//! Watching a workspace folder for changes made outside the app (a git pull,
//! an editor), so that the tree never goes stale.
//!
//! [`WorkspaceWatcher`] reports changed files in batches: a burst of changes
//! (hundreds of files from one pull) gives one notification once the folder
//! has been quiet for [`WatchOptions::quiet`], or after
//! [`WatchOptions::max_delay`] if it never goes quiet. Hidden entries (a
//! leading `.`, such as `.git` and the app's own temporary files) are not
//! reported, as the workspace ignores them.
//!
//! A notification only says that files changed; the owner of the workspace
//! then calls [`super::Workspace::refresh`], which tells whether the tree
//! actually differs. The app's own edits are reported too (they are file
//! changes like any other), but refreshing after them finds nothing new.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

/// How changes are batched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchOptions {
    /// A batch is reported once no change has come for this long.
    pub quiet: Duration,
    /// ... or at the latest this long after its first change.
    pub max_delay: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(300),
            max_delay: Duration::from_millis(1500),
        }
    }
}

/// A batch of changes on disk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    /// The changed files and folders relative to the workspace root,
    /// `/`-separated, sorted, each once. It can be empty when the system
    /// lost track of what changed (an overflow): reload all the same.
    pub files: Vec<String>,
}

/// The folder could not be watched.
#[derive(Debug, thiserror::Error)]
#[error("cannot watch {}: {reason}", path.display())]
pub struct WatchError {
    pub path: PathBuf,
    pub reason: String,
}

/// Watches a workspace folder and everything below it until dropped.
pub struct WorkspaceWatcher {
    _watcher: notify::RecommendedWatcher,
    stopped: Arc<AtomicBool>,
}

impl std::fmt::Debug for WorkspaceWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceWatcher").finish_non_exhaustive()
    }
}

impl WorkspaceWatcher {
    /// Starts watching the folder at `root`. `on_change` is called on a
    /// thread of the watcher's own with each batch of changes; once the
    /// watcher is dropped it is not called again.
    pub fn start(
        root: impl AsRef<Path>,
        options: WatchOptions,
        on_change: impl FnMut(Changes) + Send + 'static,
    ) -> Result<Self, WatchError> {
        let root = root.as_ref().to_path_buf();
        let error = |reason: String| WatchError {
            path: root.clone(),
            reason,
        };
        // Some systems report paths through the canonical form of the root
        // (macOS reports `/private/var` for `/var`).
        let prefixes: Vec<PathBuf> = [Some(root.clone()), root.canonicalize().ok()]
            .into_iter()
            .flatten()
            .collect();

        let (sender, receiver) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let files = match result {
                    Ok(event) => match relevant_files(&event, &prefixes) {
                        Some(files) => files,
                        None => return,
                    },
                    // Something changed, but not known which files.
                    Err(_) => Vec::new(),
                };
                // The batching thread is gone only once the watcher is.
                let _ = sender.send(files);
            })
            .map_err(|e| error(e.to_string()))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| error(e.to_string()))?;

        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        thread::Builder::new()
            .name("workspace-watcher".into())
            .spawn(move || report_in_batches(&receiver, options, &stop, on_change))
            .map_err(|e| error(e.to_string()))?;
        Ok(Self {
            _watcher: watcher,
            stopped,
        })
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        // Dropping the notify watcher closes the channel, which ends the
        // batching thread; this stops a batch already being gathered.
        self.stopped.store(true, Ordering::SeqCst);
    }
}

/// Gathers the changed files the watcher sends into batches and reports
/// each, until the watcher is dropped.
fn report_in_batches(
    receiver: &mpsc::Receiver<Vec<String>>,
    options: WatchOptions,
    stopped: &AtomicBool,
    mut on_change: impl FnMut(Changes),
) {
    while let Ok(mut files) = receiver.recv() {
        let deadline = Instant::now() + options.max_delay;
        let mut watcher_alive = true;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match receiver.recv_timeout(options.quiet.min(left)) {
                Ok(more) => files.extend(more),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    watcher_alive = false;
                    break;
                }
            }
        }
        if stopped.load(Ordering::SeqCst) {
            return;
        }
        files.sort();
        files.dedup();
        on_change(Changes { files });
        if !watcher_alive {
            return;
        }
    }
}

/// The workspace files an event is about, or `None` if it does not concern
/// the workspace (reads, hidden entries).
fn relevant_files(event: &notify::Event, prefixes: &[PathBuf]) -> Option<Vec<String>> {
    if matches!(event.kind, EventKind::Access(_)) {
        // Reading files, as a reload does, changes nothing.
        return None;
    }
    if event.need_rescan() {
        return Some(Vec::new());
    }
    // Windows reports a folder as modified when its timestamps catch up
    // with a change inside it, sometimes only once a reload reads it. The
    // change inside is reported by itself; renames are not affected.
    let folder_touched = matches!(
        event.kind,
        EventKind::Modify(kind) if !matches!(kind, notify::event::ModifyKind::Name(_))
    );
    let mut files: Vec<String> = event
        .paths
        .iter()
        .filter(|path| !(folder_touched && path.is_dir()))
        .filter_map(|path| relative(path, prefixes))
        .collect();
    // Some systems (Linux) start watching a new folder only once they
    // report it, missing what was put in it before (a pull that adds a
    // folder of hosts): what it holds counts as changed too.
    if matches!(event.kind, EventKind::Create(_)) {
        for path in &event.paths {
            add_contents(path, prefixes, &mut files);
        }
    }
    (!files.is_empty()).then_some(files)
}

/// Adds everything in the folder `path` (not hidden, symlinked folders not
/// followed, as the workspace reads it) to `files`; nothing if `path` is
/// not a folder.
fn add_contents(path: &Path, prefixes: &[PathBuf], files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file) = relative(&path, prefixes) else {
            continue;
        };
        files.push(file);
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            add_contents(&path, prefixes, files);
        }
    }
}

/// `path` relative to the root, `/`-separated; `None` for the root itself
/// and for anything hidden.
fn relative(path: &Path, prefixes: &[PathBuf]) -> Option<String> {
    let inside = prefixes
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix).ok())
        .unwrap_or(path);
    let mut names = Vec::new();
    for component in inside.components() {
        if let Component::Normal(name) = component {
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                return None;
            }
            names.push(name.into_owned());
        }
    }
    (!names.is_empty()).then(|| names.join("/"))
}
