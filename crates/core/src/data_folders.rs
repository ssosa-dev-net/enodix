//! Where the app keeps its data on this computer (#128), and the one-time
//! move of what is large and bound to the machine out of the settings
//! folder.
//!
//! The app has two folders of its own. The **settings folder** holds what
//! is small and the user's own: preferences, the list of workspaces, the
//! vault, trusted host keys, the Python choice. The **local folder** holds
//! what is large or only makes sense on this computer: session logs, config
//! backups ([`CONFIGS`]), kept job results ([`JOBS`]) and the app's Python
//! environment ([`PYTHON`]). On Windows the first is in the roaming part of
//! the profile (`%APPDATA%`), which follows the user to other computers and
//! is copied at every logon; the second is `%LOCALAPPDATA%`. Elsewhere they
//! are one folder, and nothing moves.
//!
//! Before 0.5.0 the three [`MOVED`] lived in the settings folder.
//! [`settle`], run at start before anything reads them, moves each one the
//! local folder lacks:
//!
//! - On one drive a move is a rename: done whole or not at all.
//! - Across drives (a roaming folder redirected to a server) it is a copy:
//!   every file is copied into a staging folder beside the target, written
//!   to disk and read back to compare, the staging folder renamed into
//!   place, and only then is the old folder renamed aside (the commit) and
//!   deleted. A marker file beside the target says a copy is under way: a
//!   start that finds one and no old folder renamed aside knows the copy
//!   was never committed, throws it away and starts again; one that finds
//!   the old folder renamed aside knows the copy is the data and finishes
//!   the deletion. So an interruption at any point (the power, a full disk,
//!   the app killed) leaves the data whole in exactly one place.
//! - A move that fails leaves the old folder as it was and the app uses it
//!   until a later start tries again; it says so ([`Notice`]).
//! - When both folders hold one of them (an older version of the app run
//!   after the move, or a copy put back), neither is touched: the local
//!   one is used and the other is reported, at every start, never merged.
//!
//! Two copies of the app never move at once, and a copy never moves data
//! another running copy is using: every running copy holds a shared lock on
//! [`RUNNING_LOCK`] in the local folder for as long as it runs (kept by
//! [`Settled`]); the move needs that lock alone, taken under a second one,
//! [`MOVING_LOCK`], that a copy holds while it decides. A copy that starts
//! while another runs uses the data where it is and leaves the move to a
//! start when it runs alone. The locks are the system's (`File::lock`), let
//! go when a process ends however it ends.
//!
//! The Python environment is renamed like the rest: a virtual environment
//! works from its new folder (its interpreter finds the base Python by its
//! `pyvenv.cfg` and its packages from where it is; checked with Python 3.15
//! on Windows, renamed and copied, and in the tests). Only its own
//! launchers (`Scripts\pip.exe`) and activation scripts keep the old path,
//! and the app uses neither: it runs `<environment>\Scripts\python.exe -m
//! ...`. The Python choice, which stays in the settings folder, is pointed
//! at the moved interpreter when it named the old one. Across drives it is
//! **not copied** (thousands of files, each written to disk and checked:
//! 26 s for a bare environment on a local disk, at a start): it stays and
//! goes on working where it is ([`DataFolders::python_left`],
//! [`NoticeKind::SetUpAgain`]); setting it up again makes it in the local
//! folder, and the old one is then removed ([`remove_left_environment`]).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::python::{self, PythonChoice};

/// Config backups, under both folders.
pub const CONFIGS: &str = "configs";
/// Kept job results.
pub const JOBS: &str = "jobs";
/// The app's Python environment.
pub const PYTHON: &str = python::ENVIRONMENT_FOLDER;
/// What moves from the settings folder to the local one, in this order.
pub const MOVED: [&str; 3] = [CONFIGS, JOBS, PYTHON];

/// Held exclusively while a copy of the app decides where its data is (and
/// moves it), in the local folder.
pub const MOVING_LOCK: &str = ".data-moving.lock";
/// Held shared by every running copy of the app, in the local folder.
pub const RUNNING_LOCK: &str = ".data-running.lock";

/// Where each piece of the app's data is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataFolders {
    /// Preferences, workspaces, the vault, host keys, the Python choice.
    pub settings: PathBuf,
    /// Config backups: a `ConfigStore`'s root.
    pub configs: PathBuf,
    /// Kept job results: a `RunStore`'s root.
    pub jobs: PathBuf,
    /// The app's Python environment: where it is, or where it is made
    /// when it is set up again.
    pub python_environment: PathBuf,
    /// An environment left in the settings folder because it could not be
    /// renamed (another drive): still the one in use until the environment
    /// is set up again in `python_environment`, then removed
    /// ([`remove_left_environment`]).
    pub python_left: Option<PathBuf>,
}

impl DataFolders {
    /// Every piece in one folder (the tests' and the smoke's
    /// `NETTERM_DATA_DIR`, and systems with a single data folder).
    pub fn in_one(folder: &Path) -> Self {
        Self::placed(folder, [folder, folder, folder])
    }

    fn placed(settings: &Path, [configs, jobs, environment]: [&Path; 3]) -> Self {
        Self {
            settings: settings.to_path_buf(),
            configs: configs.join(CONFIGS),
            jobs: jobs.join(JOBS),
            python_environment: environment.join(PYTHON),
            python_left: None,
        }
    }
}

/// What a start found, beyond moving what was there to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    /// The move failed; the data stays in the settings folder, which is
    /// used until a later start moves it.
    NotMoved,
    /// Both folders hold it: the local one is used, the other left alone.
    BothHold,
    /// Another copy of the app is running: the move waits for a start when
    /// none is.
    OtherCopyRunning,
    /// The move is done, but what is left of the old folder could not be
    /// removed (it is tried again at the next start).
    LeftBehind,
    /// The Python environment could not be renamed into the local folder
    /// (another drive) and is not copied: it goes on working where it is
    /// until it is set up again.
    SetUpAgain,
}

/// Something about where the data is that the user should be told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// Which of [`MOVED`].
    pub what: &'static str,
    pub kind: NoticeKind,
    /// The words to show.
    pub message: String,
}

/// Where a move stands, as [`settle_with`] goes (for a watcher; the tests
/// stop the process at each of them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Another copy of the app is deciding (or moving): waiting for it.
    Waiting,
    /// One file more is copied and checked (`files` so far).
    Copied { what: &'static str, files: usize },
    /// Every file is copied and checked, in the staging folder.
    CopiedAll { what: &'static str },
    /// The copy is in place in the local folder; the old one is still
    /// where it was.
    InPlace { what: &'static str },
    /// The old folder is renamed aside: the copy is the data now.
    Committed { what: &'static str },
    /// The move of `what` is over (the old folder deleted, or renamed into
    /// place).
    Moved { what: &'static str },
}

/// How [`settle_with`] moves.
pub struct SettleOptions<'a> {
    /// Copy, as across drives, even where a rename would do (the tests, or
    /// a folder that refuses to be renamed).
    pub copy: bool,
    pub watch: &'a mut dyn FnMut(&Step),
}

/// Where the data is now, what moved, and what to tell the user. Holds the
/// running copy's shared lock: keep it for as long as the app runs.
#[derive(Debug)]
pub struct Settled {
    pub folders: DataFolders,
    /// What moved at this start.
    pub moved: Vec<&'static str>,
    pub notices: Vec<Notice>,
    _running: Option<File>,
}

/// [`settle_with`], renaming where it can, unwatched.
pub fn settle(settings: &Path, local: &Path) -> Settled {
    settle_with(
        settings,
        local,
        SettleOptions {
            copy: false,
            watch: &mut |_| {},
        },
    )
}

/// Decides where the data is, `settings` being the settings folder and
/// `local` the local one, moving what is still in the settings folder
/// alone (module documentation). The same folder twice moves nothing and
/// takes no lock.
pub fn settle_with(settings: &Path, local: &Path, options: SettleOptions<'_>) -> Settled {
    let SettleOptions { copy, watch } = options;
    if settings == local {
        return Settled {
            folders: DataFolders::in_one(settings),
            moved: Vec::new(),
            notices: Vec::new(),
            _running: None,
        };
    }
    let mut notices = Vec::new();
    let mut moved = Vec::new();
    // Without the locks, nothing can be moved safely: the data stays where
    // it is found.
    let locks = fs::create_dir_all(local).and_then(|()| {
        let moving = open_lock(&local.join(MOVING_LOCK))?;
        match moving.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                watch(&Step::Waiting);
                moving.lock()?;
            }
            Err(fs::TryLockError::Error(error)) => return Err(error),
        }
        let running = open_lock(&local.join(RUNNING_LOCK))?;
        let alone = alone_on(&running)?;
        Ok((moving, running, alone))
    });
    let (moving, running, alone) = match locks {
        Ok((moving, running, alone)) => (Some(moving), Some(running), alone),
        Err(error) => {
            for what in MOVED {
                if has_entries(&settings.join(what)) {
                    notices.push(Notice {
                        what,
                        kind: NoticeKind::NotMoved,
                        message: format!(
                            "{} was not moved to {}: the folder cannot be used ({error}); the \
                             app goes on using it where it is",
                            settings.join(what).display(),
                            local.display()
                        ),
                    });
                }
            }
            (None, None, false)
        }
    };
    let mut places = [local, local, local];
    let mut left = None;
    for (index, what) in MOVED.into_iter().enumerate() {
        let item = Item::new(settings, local, what);
        let place = if moving.is_none() {
            item.where_it_is()
        } else if alone {
            item.settle(copy, watch, &mut notices, &mut moved)
        } else {
            let place = item.where_it_is();
            if place == Place::Settings && !item.dst_has_entries() {
                notices.push(Notice {
                    what,
                    kind: NoticeKind::OtherCopyRunning,
                    message: format!(
                        "{} stays where it is for now: another copy of the app is running; it \
                         moves to {} at a start while no other copy runs",
                        item.src.display(),
                        local.display()
                    ),
                });
            }
            place
        };
        match place {
            Place::Settings => places[index] = settings,
            Place::LeftBehind => left = Some(item.src.clone()),
            Place::Local => {}
        }
    }
    let mut folders = DataFolders::placed(settings, places);
    folders.python_left = left;
    if let Some(running) = &running {
        point_choice_at_moved_environment(settings, &folders);
        // From exclusive (if it was) to shared, still under the moving
        // lock: no other copy decides in between.
        let _ = running.unlock();
        if let Err(error) = running.lock_shared() {
            eprintln!("cannot hold the data folder's running lock: {error}");
        }
    }
    drop(moving);
    Settled {
        folders,
        moved,
        notices,
        _running: running,
    }
}

/// How long a start waits for the running lock before it takes another
/// copy to be running.
const ENDING_COPY_WAIT: Duration = Duration::from_secs(1);

/// Whether no other copy holds the running lock, which is then held alone.
/// A copy that is just ending (the app starting again after an update)
/// holds it a moment longer, and so, on Linux, does a process that is just
/// being started, until it runs its program (it has copies of the starting
/// process's open files till then; seen in the tests): the lock is tried
/// for [`ENDING_COPY_WAIT`] before another copy is taken to be running.
fn alone_on(running: &File) -> io::Result<bool> {
    let deadline = Instant::now() + ENDING_COPY_WAIT;
    loop {
        match running.try_lock() {
            Ok(()) => return Ok(true),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(fs::TryLockError::WouldBlock) => return Ok(false),
            Err(fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

fn open_lock(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Which folder holds an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Settings,
    Local,
    /// The Python environment, left in the settings folder until it is set
    /// up again in the local one.
    LeftBehind,
}

/// One of [`MOVED`], with the names its move uses.
struct Item {
    what: &'static str,
    /// The settings folder (the Python choice is there).
    settings: PathBuf,
    /// In the settings folder: where it was before 0.5.0.
    src: PathBuf,
    /// In the local folder: where it goes.
    dst: PathBuf,
    /// Beside `dst`: the copy being made.
    staging: PathBuf,
    /// Beside `dst`: a copy is under way, not committed yet.
    marker: PathBuf,
    /// Beside `src`: the old folder once a copy is committed.
    aside: PathBuf,
}

impl Item {
    fn new(settings: &Path, local: &Path, what: &'static str) -> Self {
        Self {
            what,
            settings: settings.to_path_buf(),
            src: settings.join(what),
            dst: local.join(what),
            staging: local.join(format!(".{what}.incoming")),
            marker: local.join(format!(".{what}.moving")),
            aside: settings.join(format!(".{what}.moved")),
        }
    }

    /// The local folder.
    fn local(&self) -> &Path {
        self.dst.parent().unwrap_or(&self.dst)
    }

    fn dst_has_entries(&self) -> bool {
        has_entries(&self.dst)
    }

    /// Where the item is, looking only (another copy may be using it).
    fn where_it_is(&self) -> Place {
        // A copy under way and not committed is not the data.
        let uncommitted = present(&self.marker) && !present(&self.aside);
        if uncommitted || (has_entries(&self.src) && !self.dst_has_entries()) {
            Place::Settings
        } else {
            Place::Local
        }
    }

    /// Finishes or throws away what an interrupted copy left, then moves
    /// the item if it is only in the settings folder. Under both locks.
    fn settle(
        &self,
        copy: bool,
        watch: &mut dyn FnMut(&Step),
        notices: &mut Vec<Notice>,
        moved: &mut Vec<&'static str>,
    ) -> Place {
        if present(&self.marker) {
            if present(&self.aside) {
                // Committed, cut before the marker went: the copy is it.
                if let Err(error) = fs::remove_file(&self.marker) {
                    return self.left_behind(notices, &self.marker, &error);
                }
            } else {
                // Never committed: the old folder is the data.
                if let Err(error) = self.throw_away_copy() {
                    notices.push(self.not_moved(&format!(
                        "an earlier copy of it that was cut short could not be removed from {} \
                         ({error})",
                        self.dst.display()
                    )));
                    return Place::Settings;
                }
            }
        }
        if present(&self.aside) {
            if let Err(error) = remove_all(&self.aside) {
                self.left_behind(notices, &self.aside, &error);
            }
        }
        if !has_entries(&self.src) {
            // Nothing (or an empty folder) to move.
            if present(&self.src) && !self.dst_has_entries() {
                let _ = fs::remove_dir(&self.src);
            }
            return Place::Local;
        }
        if self.dst_has_entries() {
            if self.what == PYTHON && choice_inside(&self.settings, &self.dst) {
                // An environment set up again in the local folder, the old
                // one not removed yet: nothing uses it.
                if let Err(error) = remove_all(&self.src) {
                    return self.left_behind(notices, &self.src, &error);
                }
                return Place::Local;
            }
            notices.push(Notice {
                what: self.what,
                kind: NoticeKind::BothHold,
                message: format!(
                    "{} was not moved: {} holds {} already, which the app uses; nothing of \
                     either was changed: look in the first for anything you need, then \
                     remove it",
                    self.src.display(),
                    self.dst.display(),
                    self.description()
                ),
            });
            return Place::Local;
        }
        // An empty folder in the way is no data.
        if present(&self.dst) {
            if let Err(error) = fs::remove_dir(&self.dst) {
                notices.push(
                    self.not_moved(&format!("{} is in the way ({error})", self.dst.display())),
                );
                return Place::Settings;
            }
        }
        if !copy {
            match fs::rename(&self.src, &self.dst) {
                Ok(()) => {
                    moved.push(self.what);
                    watch(&Step::Moved { what: self.what });
                    return Place::Local;
                }
                Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
                Err(error) => {
                    notices.push(self.not_moved(&error.to_string()));
                    return Place::Settings;
                }
            }
        }
        if self.what == PYTHON {
            // Thousands of files, each written to disk and checked, at the
            // start (26 s for a bare environment on a local disk): setting
            // it up again is quicker, and it works where it is meanwhile.
            notices.push(Notice {
                what: PYTHON,
                kind: NoticeKind::SetUpAgain,
                message: format!(
                    "the app's Python environment stays in {} for now: it is on another drive \
                     than {}, and an environment is not copied. It goes on working there. To \
                     have it in the local folder, set it up again (Preferences, Python): it is \
                     made in {} and the old one is then removed",
                    self.src.display(),
                    self.local().display(),
                    self.dst.display()
                ),
            });
            return Place::LeftBehind;
        }
        match self.copy(watch) {
            Ok(()) => {
                moved.push(self.what);
                if let Err(error) = self.finish(watch) {
                    self.left_behind(notices, &self.aside, &error);
                }
                Place::Local
            }
            Err(reason) => {
                // Not committed: back to where it was.
                let cleared = self.throw_away_copy();
                notices.push(self.not_moved(&match cleared {
                    Ok(()) => reason,
                    Err(error) => format!(
                        "{reason}; what was copied could not be removed from {} ({error})",
                        self.dst.display()
                    ),
                }));
                Place::Settings
            }
        }
    }

    /// The copy, up to and including the commit. `Err` before the commit
    /// (nothing changed but the copy, which the caller throws away).
    fn copy(&self, watch: &mut dyn FnMut(&Step)) -> Result<(), String> {
        let what = self.what;
        if present(&self.staging) {
            remove_all(&self.staging).map_err(|error| {
                format!("{} could not be emptied ({error})", self.staging.display())
            })?;
        }
        write_marker(&self.marker)
            .map_err(|error| format!("{} could not be written ({error})", self.marker.display()))?;
        let mut files = 0;
        copy_tree(&self.src, &self.staging, &mut |_| {
            files += 1;
            watch(&Step::Copied { what, files });
        })?;
        watch(&Step::CopiedAll { what });
        fs::rename(&self.staging, &self.dst).map_err(|error| {
            format!(
                "the copy could not be put in place as {} ({error})",
                self.dst.display()
            )
        })?;
        watch(&Step::InPlace { what });
        // The commit: from here on the copy is the data.
        fs::rename(&self.src, &self.aside).map_err(|error| {
            format!("the old folder could not be set aside after it was copied ({error})")
        })?;
        watch(&Step::Committed { what });
        Ok(())
    }

    /// After the commit: the marker goes, then the old folder.
    fn finish(&self, watch: &mut dyn FnMut(&Step)) -> io::Result<()> {
        fs::remove_file(&self.marker)?;
        remove_all(&self.aside)?;
        watch(&Step::Moved { what: self.what });
        Ok(())
    }

    /// Removes an uncommitted copy (in place or still staged) and its
    /// marker.
    fn throw_away_copy(&self) -> io::Result<()> {
        if present(&self.dst) {
            remove_all(&self.dst)?;
        }
        if present(&self.staging) {
            remove_all(&self.staging)?;
        }
        match fs::remove_file(&self.marker) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn description(&self) -> &'static str {
        match self.what {
            CONFIGS => "config backups",
            JOBS => "kept job results",
            _ => "a Python environment",
        }
    }

    fn not_moved(&self, reason: &str) -> Notice {
        Notice {
            what: self.what,
            kind: NoticeKind::NotMoved,
            message: format!(
                "the {} in {} could not be moved to {}: {reason}. Nothing was lost: the app \
                 uses them where they are and tries again at its next start",
                self.description(),
                self.src.display(),
                self.dst.display()
            ),
        }
    }

    fn left_behind(&self, notices: &mut Vec<Notice>, path: &Path, error: &io::Error) -> Place {
        notices.push(Notice {
            what: self.what,
            kind: NoticeKind::LeftBehind,
            message: format!(
                "the {} were moved to {}, but {} could not be removed ({error}); it is tried \
                 again at the next start",
                self.description(),
                self.dst.display(),
                path.display()
            ),
        });
        Place::Local
    }
}

/// Whether the name `path` is there (a file, a folder or a link).
fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Whether `path` is a folder with something in it, or anything but a
/// folder (which is not nothing either).
fn has_entries(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Err(_) => false,
        Ok(meta) if meta.is_dir() => fs::read_dir(path).map_or(true, |mut e| e.next().is_some()),
        Ok(_) => true,
    }
}

fn remove_all(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    }
}

fn write_marker(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    file.sync_all()
}

/// Copies the folder `from` to `to` (which must not exist), every file
/// written to disk and read back to compare; `each` after every file.
/// Links are refused: what they point to is not the app's to copy.
fn copy_tree(from: &Path, to: &Path, each: &mut dyn FnMut(&Path)) -> Result<(), String> {
    let meta = fs::symlink_metadata(from)
        .map_err(|error| format!("{} cannot be read ({error})", from.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", from.display()));
    }
    fs::create_dir(to).map_err(|error| format!("{} cannot be created ({error})", to.display()))?;
    let entries = fs::read_dir(from)
        .map_err(|error| format!("{} cannot be read ({error})", from.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("{} cannot be read ({error})", from.display()))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| format!("{} cannot be read ({error})", source.display()))?;
        if kind.is_symlink() {
            return Err(format!(
                "{} is a link, which is not copied",
                source.display()
            ));
        } else if kind.is_dir() {
            copy_tree(&source, &target, each)?;
        } else {
            copy_file(&source, &target).map_err(|error| {
                format!(
                    "{} could not be copied to {} ({error})",
                    source.display(),
                    target.display()
                )
            })?;
            each(&target);
        }
    }
    Ok(())
}

/// Copies one file, on disk before it returns, then reads both back to
/// compare. The permissions follow (a read-only file stays read-only).
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut source = File::open(from)?;
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    io::copy(&mut source, &mut target)?;
    target.sync_all()?;
    drop(target);
    if !same_bytes(from, to)? {
        return Err(io::Error::other("the copy read back differs"));
    }
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

fn same_bytes(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (File::open(a)?, File::open(b)?);
    let (mut x, mut y) = (vec![0; 64 * 1024], vec![0; 64 * 1024]);
    loop {
        let n = read_full(&mut a, &mut x)?;
        let m = read_full(&mut b, &mut y)?;
        if n != m || x[..n] != y[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// Reads until `buffer` is full or the file ends.
fn read_full(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// Whether the Python choice kept in `settings` names an interpreter in
/// `folder`.
fn choice_inside(settings: &Path, folder: &Path) -> bool {
    PythonChoice::load(settings).is_some_and(|choice| choice.interpreter.starts_with(folder))
}

/// Removes the Python environment left in the settings folder
/// ([`DataFolders::python_left`]) once the choice no longer names it (the
/// environment was set up again in the local folder); `Ok(false)` while it
/// does, or when there is none.
pub fn remove_left_environment(folders: &DataFolders) -> io::Result<bool> {
    let Some(left) = &folders.python_left else {
        return Ok(false);
    };
    if choice_inside(&folders.settings, left) || !present(left) {
        return Ok(false);
    }
    remove_all(left)?;
    Ok(true)
}

/// Points the Python choice (in the settings folder) at the environment's
/// interpreter in its new folder, when it named one in the old folder that
/// is gone and the new one is there.
fn point_choice_at_moved_environment(settings: &Path, folders: &DataFolders) {
    let Some(choice) = PythonChoice::load(settings) else {
        return;
    };
    let old = settings.join(PYTHON);
    if folders.python_environment == old {
        return;
    }
    let Ok(inside) = choice.interpreter.strip_prefix(&old) else {
        return;
    };
    let moved = folders.python_environment.join(inside);
    if choice.interpreter.exists() || !moved.exists() {
        return;
    }
    let pointed = PythonChoice {
        interpreter: moved,
        ..choice
    };
    if let Err(error) = pointed.save(settings) {
        eprintln!("the Python choice still names the environment's old folder: {error}");
    }
}
