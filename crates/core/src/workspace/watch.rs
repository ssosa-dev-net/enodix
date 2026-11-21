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
//!
//! The folder itself can go and come back: a network drive that
//! disconnects and reconnects, a folder deleted and restored, or replaced by
//! another one at the same path. The system's watch does not survive that
//! (it watches the folder that was there), so every
//! [`WatchOptions::check_every`] the watcher looks at the folder: when it is
//! gone, it says so once (an empty [`Changes`]: the owner's refresh then
//! fails, and the workspace says it cannot be reached); when it is back, or
//! another folder stands at the path, it watches it anew and says so (an
//! empty [`Changes`] again: the refresh loads what is there now). A system
//! watch can also end without a word (a network share's connection drops):
//! [`WorkspaceWatcher::check_again`] lets the owner, whose refresh found the
//! folder unreachable, have it watched anew at the next check.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

/// How changes are batched, and how often the folder itself is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchOptions {
    /// A batch is reported once no change has come for this long.
    pub quiet: Duration,
    /// ... or at the latest this long after its first change.
    pub max_delay: Duration,
    /// How often the watcher checks that the folder is still the one it
    /// watches, and looks for it while it is gone.
    pub check_every: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(300),
            max_delay: Duration::from_millis(1500),
            check_every: Duration::from_secs(2),
        }
    }
}

/// A batch of changes on disk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    /// The changed files and folders relative to the workspace root,
    /// `/`-separated, sorted, each once. It can be empty when the system
    /// lost track of what changed (an overflow), or when the folder itself
    /// went or came back: reload all the same.
    pub files: Vec<String>,
}

/// The folder could not be watched.
#[derive(Debug, thiserror::Error)]
#[error("cannot watch {}: {reason}", path.display())]
pub struct WatchError {
    pub path: PathBuf,
    pub reason: String,
}

/// What the watcher's thread is sent: changed files from the system's
/// watch, or a word from the owner.
enum Message {
    Files(Vec<String>),
    CheckAgain,
    Stop,
}

/// The system's watch, shared between the owner (who drops it at once when
/// dropping the watcher) and the thread (which replaces it when the folder
/// comes back).
type SystemWatch = Arc<Mutex<Option<notify::RecommendedWatcher>>>;

/// Watches a workspace folder and everything below it until dropped.
pub struct WorkspaceWatcher {
    system: SystemWatch,
    control: mpsc::Sender<Message>,
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
        let (sender, receiver) = mpsc::channel();
        let identity = folder_identity(&root).ok();
        let watch = system_watch(&root, &sender).map_err(error)?;
        let system: SystemWatch = Arc::new(Mutex::new(Some(watch)));
        let stopped = Arc::new(AtomicBool::new(false));
        let mut thread = WatchThread {
            root: root.clone(),
            options,
            system: Arc::clone(&system),
            sender: sender.clone(),
            stopped: Arc::clone(&stopped),
            identity,
            check_again: false,
        };
        thread::Builder::new()
            .name("workspace-watcher".into())
            .spawn(move || thread.run(&receiver, on_change))
            .map_err(|e| error(e.to_string()))?;
        Ok(Self {
            system,
            control: sender,
            stopped,
        })
    }

    /// The owner could not reload the folder (it could not be reached):
    /// at the next check, once the folder is there, it is watched anew and
    /// a notification follows. A system watch that ended without a word
    /// (a network share whose connection dropped) is so replaced.
    pub fn check_again(&self) {
        let _ = self.control.send(Message::CheckAgain);
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        // This stops a batch already being gathered; the system's watch
        // ends here, the thread at its next message.
        self.stopped.store(true, Ordering::SeqCst);
        let watch = self
            .system
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(watch);
        let _ = self.control.send(Message::Stop);
    }
}

/// Starts the system's watch on `root`, sending what it sees to `sender`.
fn system_watch(
    root: &Path,
    sender: &mpsc::Sender<Message>,
) -> Result<notify::RecommendedWatcher, String> {
    // Some systems report paths through the canonical form of the root
    // (macOS reports `/private/var` for `/var`).
    let prefixes: Vec<PathBuf> = [Some(root.to_path_buf()), root.canonicalize().ok()]
        .into_iter()
        .flatten()
        .collect();
    let sender = sender.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let files = match result {
            Ok(event) => match relevant_files(&event, &prefixes) {
                Some(files) => files,
                None => return,
            },
            // Something changed, but not known which files.
            Err(_) => Vec::new(),
        };
        // The thread is gone only once the watcher is.
        let _ = sender.send(Message::Files(files));
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;
    Ok(watcher)
}

/// The watcher's own thread: batches what the system's watch reports, and
/// checks the folder itself.
struct WatchThread {
    root: PathBuf,
    options: WatchOptions,
    system: SystemWatch,
    sender: mpsc::Sender<Message>,
    stopped: Arc<AtomicBool>,
    /// The folder watched (see [`folder_identity`]); `None` while it is
    /// gone.
    identity: Option<FolderIdentity>,
    /// The owner asked for the folder to be watched anew.
    check_again: bool,
}

impl WatchThread {
    fn run(&mut self, receiver: &mpsc::Receiver<Message>, mut on_change: impl FnMut(Changes)) {
        let mut next_check = Instant::now() + self.options.check_every;
        loop {
            let wait = next_check.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(wait) {
                Ok(Message::Files(files)) => {
                    let Some(files) = self.gather(receiver, files) else {
                        return;
                    };
                    if self.stopped.load(Ordering::SeqCst) {
                        return;
                    }
                    on_change(Changes { files });
                }
                Ok(Message::CheckAgain) => self.check_again = true,
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= next_check {
                next_check = Instant::now() + self.options.check_every;
                if self.folder_changed() {
                    if self.stopped.load(Ordering::SeqCst) {
                        return;
                    }
                    on_change(Changes::default());
                }
            }
        }
    }

    /// Gathers the batch `files` begins, until the folder is quiet or the
    /// batch is due; `None` once the watcher is stopped.
    fn gather(
        &mut self,
        receiver: &mpsc::Receiver<Message>,
        mut files: Vec<String>,
    ) -> Option<Vec<String>> {
        let deadline = Instant::now() + self.options.max_delay;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match receiver.recv_timeout(self.options.quiet.min(left)) {
                Ok(Message::Files(more)) => files.extend(more),
                Ok(Message::CheckAgain) => self.check_again = true,
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        files.sort();
        files.dedup();
        Some(files)
    }

    /// Checks the folder: whether it went, or came back (or another stands
    /// at its path, or the owner asked) and is now watched anew. Either is
    /// worth a notification.
    fn folder_changed(&mut self) -> bool {
        let Ok(identity) = folder_identity(&self.root) else {
            // Gone (or unreadable): the system's watch is of no use now.
            let was_watched = self.identity.take().is_some();
            self.system
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            return was_watched;
        };
        let watched = self
            .system
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if watched && self.identity == Some(identity) && !self.check_again {
            return false;
        }
        // The old watch first, so its events cannot mix with the new one's.
        self.system
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Ok(watch) = system_watch(&self.root, &self.sender) else {
            // Tried again at the next check.
            self.identity = None;
            return false;
        };
        let mut system = self.system.lock().unwrap_or_else(PoisonError::into_inner);
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        *system = Some(watch);
        self.identity = Some(identity);
        self.check_again = false;
        true
    }
}

/// What tells one folder from another made at the same path: the device and
/// the file's number on it.
type FolderIdentity = (u64, u64);

#[cfg(unix)]
fn folder_identity(path: &Path) -> io::Result<FolderIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("not a folder"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn folder_identity(path: &Path) -> io::Result<FolderIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_FLAG_BACKUP_SEMANTICS,
    };
    // A folder opens only with backup semantics; the default sharing lets
    // it be renamed and deleted meanwhile.
    let folder = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    // SAFETY: the structure is plain data, filled in by the call; the
    // handle is open for as long as `folder` lives.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(folder.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::other("not a folder"));
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(not(any(unix, windows)))]
fn folder_identity(path: &Path) -> io::Result<FolderIdentity> {
    // No file numbers: only whether the folder is there.
    if std::fs::metadata(path)?.is_dir() {
        Ok((0, 0))
    } else {
        Err(io::Error::other("not a folder"))
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
