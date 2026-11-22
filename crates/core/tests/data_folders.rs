//! The data folders (#128): config backups, kept job results and the
//! Python environment move once from the settings folder (roaming on
//! Windows) to the local one, whole, and nothing is lost whatever cuts the
//! move short. Played on temporary folders only: a settings folder and a
//! local folder of the test's own, never the user's.
//!
//! The copy across drives is played on one drive (`SettleOptions::copy`);
//! the processes that are cut short or start at once are this test binary
//! run again (`support::processes`), killed by the test from their spawn.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use enodix_core::configs::ConfigStore;
use enodix_core::data_folders::{
    remove_left_environment, settle, settle_with, DataFolders, NoticeKind, SettleOptions, Step,
    CONFIGS, JOBS, MOVED, MOVING_LOCK, PYTHON, RUNNING_LOCK,
};
use enodix_core::job::results::RunStore;
use enodix_core::python::{environment_interpreter, PythonChoice, Setup};
use enodix_core::workspace::HostId;
use support::processes;
use tempfile::TempDir;

/// The role of a helper that settles the folders the variables name.
const SETTLE: &str = "settle";
const SETTINGS_VARIABLE: &str = "ENODIX_TEST_SETTINGS";
const LOCAL_VARIABLE: &str = "ENODIX_TEST_LOCAL";
/// Set: copy, as across drives.
const COPY_VARIABLE: &str = "ENODIX_TEST_COPY";
/// The step (as `said` writes it) the helper stops at, printing
/// [`STOPPED`] and waiting to be killed.
const STOP_VARIABLE: &str = "ENODIX_TEST_STOP";
/// A file the helper waits for before it starts (two start at once).
const GO_VARIABLE: &str = "ENODIX_TEST_GO";
const STOPPED: &str = "stopped at the step";
const SETTLED: &str = "settled ";
const WAITING: &str = "waiting for the other copy";

const WORKSPACE: &str = "a1b2c3";
const RUN: &str = "2026-01-02_10-00-00";

#[test]
fn helper() {
    processes::play();
    if processes::role().as_deref() == Some(SETTLE) {
        play_settle();
    }
}

fn said(step: &Step) -> String {
    match step {
        Step::Waiting => "waiting".into(),
        Step::Copied { what, files } => format!("copied {what} {files}"),
        Step::CopiedAll { what } => format!("copied all {what}"),
        Step::InPlace { what } => format!("in place {what}"),
        Step::Committed { what } => format!("committed {what}"),
        Step::Moved { what } => format!("moved {what}"),
    }
}

fn play_settle() -> ! {
    let folder = |name| PathBuf::from(std::env::var_os(name).expect("the helper is told"));
    let (settings, local) = (folder(SETTINGS_VARIABLE), folder(LOCAL_VARIABLE));
    if let Some(go) = std::env::var_os(GO_VARIABLE) {
        while !Path::new(&go).exists() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let stop = std::env::var(STOP_VARIABLE).ok();
    let settled = settle_with(
        &settings,
        &local,
        SettleOptions {
            copy: std::env::var_os(COPY_VARIABLE).is_some(),
            watch: &mut |step| {
                if *step == Step::Waiting {
                    println!("{WAITING}");
                }
                if stop.as_deref() == Some(said(step).as_str()) {
                    println!("{STOPPED}");
                    std::thread::sleep(Duration::from_secs(60));
                    std::process::exit(0);
                }
            },
        },
    );
    println!(
        "{SETTLED}{}",
        serde_json::json!({ "folders": settled.folders, "moved": settled.moved })
    );
    std::process::exit(0)
}

/// A settings folder and a local folder, as an older version left them:
/// everything in the settings folder.
struct Folders {
    _temp: TempDir,
    settings: PathBuf,
    local: PathBuf,
}

impl Folders {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let settings = temp.path().join("Roaming").join("dev.enodix.app");
        let local = temp.path().join("Local").join("dev.enodix.app");
        fs::create_dir_all(&settings).unwrap();
        fs::create_dir_all(local.join("logs")).unwrap();
        fs::write(local.join("logs").join("session.log"), "a session\n").unwrap();
        for (name, text) in [
            ("preferences.yaml", "theme: dark\n"),
            ("state.yaml", "workspaces: []\n"),
            ("known_hosts", "192.0.2.1 ssh-ed25519 AAAA\n"),
            ("credentials.vault", "sealed\n"),
        ] {
            fs::write(settings.join(name), text).unwrap();
        }
        Self {
            _temp: temp,
            settings,
            local,
        }
    }

    /// Config backups, a kept run and a Python environment (a stand-in:
    /// files where a virtual environment has them, one read-only) in the
    /// settings folder, and the Python choice naming its interpreter.
    fn with_old_data(self, files: usize) -> Self {
        let configs = self.settings.join(CONFIGS).join(WORKSPACE).join("Site");
        for n in 0..files {
            let host = configs.join(format!("r{n}"));
            fs::create_dir_all(&host).unwrap();
            fs::write(
                host.join("2026-01-01_10-00-00.txt"),
                format!("hostname r{n}\n").repeat(50),
            )
            .unwrap();
        }
        let run = self.settings.join(JOBS).join(WORKSPACE).join(RUN);
        fs::create_dir_all(run.join("output")).unwrap();
        fs::write(run.join("output").join("r1.txt"), "r1# show version\n").unwrap();
        fs::write(
            run.join("run.yaml"),
            format!(
                "id: {RUN}\nworkspace: {WORKSPACE}\ntarget:\n  kind: folder\n  path: Site\nstep:\n  \
                 kind: snippet\n  name: Show\nstarted: 2026-01-02 10:00:00\nended: 2026-01-02 \
                 10:00:05\nhosts:\n- id: Site/r1.yaml\n  name: r1\n  address: 192.0.2.1\n  \
                 state:\n    kind: done\n  file: r1.txt\nskipped: []\n"
            ),
        )
        .unwrap();
        let environment = self.settings.join(PYTHON);
        let interpreter = environment_interpreter(&environment);
        fs::create_dir_all(interpreter.parent().unwrap()).unwrap();
        fs::write(&interpreter, "interpreter").unwrap();
        fs::write(environment.join("pyvenv.cfg"), "home = C:\\Python\n").unwrap();
        let package = environment.join("Lib").join("site-packages").join("enodix");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("__init__.py"), "__version__ = '0.5.0'\n").unwrap();
        let locked = package.join("RECORD");
        fs::write(&locked, "enodix/__init__.py\n").unwrap();
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&locked, permissions).unwrap();
        PythonChoice {
            interpreter,
            setup: Setup::Managed,
        }
        .save(&self.settings)
        .unwrap();
        self
    }

    fn settle(&self, copy: bool) -> enodix_core::data_folders::Settled {
        settle_with(
            &self.settings,
            &self.local,
            SettleOptions {
                copy,
                watch: &mut |_| {},
            },
        )
    }

    /// This test binary as a helper settling these folders.
    fn helper(&self, copy: bool, stop: Option<&str>, go: Option<&Path>) -> Child {
        let pids = self.local.parent().unwrap().join("pids");
        fs::create_dir_all(&pids).unwrap();
        let mut command = processes::helper(SETTLE, &pids);
        command
            .env(SETTINGS_VARIABLE, &self.settings)
            .env(LOCAL_VARIABLE, &self.local)
            .stdout(Stdio::piped())
            .stdin(Stdio::null());
        if copy {
            command.env(COPY_VARIABLE, "1");
        }
        if let Some(stop) = stop {
            command.env(STOP_VARIABLE, stop);
        }
        if let Some(go) = go {
            command.env(GO_VARIABLE, go);
        }
        command.spawn().unwrap()
    }
}

/// Every file under `root` (relative `/` path → bytes), the lock files left
/// out (they are no data).
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                out.insert(format!("{name}/"), Vec::new());
                walk(&entry.path(), &format!("{name}/"), out);
            } else if name != MOVING_LOCK && name != RUNNING_LOCK {
                out.insert(name, fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
}

/// The part of `tree` under `prefix/`, with `prefix/` taken off.
fn under(tree: &BTreeMap<String, Vec<u8>>, prefix: &str) -> BTreeMap<String, Vec<u8>> {
    let prefix = format!("{prefix}/");
    tree.iter()
        .filter_map(|(path, bytes)| Some((path.strip_prefix(&prefix)?.to_string(), bytes.clone())))
        .collect()
}

/// What is in `tree` but the three moved folders (and the Python choice,
/// which follows the environment).
fn the_rest(tree: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    tree.iter()
        .filter(|(path, _)| {
            !MOVED
                .iter()
                .any(|what| path.starts_with(&format!("{what}/")))
                && path.as_str() != "python.yaml"
        })
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}

fn moved_to_local(folders: &DataFolders, local: &Path) -> bool {
    folders.configs == local.join(CONFIGS)
        && folders.jobs == local.join(JOBS)
        && folders.python_environment == local.join(PYTHON)
}

/// Checks the move is complete: each moved folder in the local folder
/// exactly as it was (`before`, the settings folder's tree), nothing of
/// them left in the settings folder (no folder set aside either) or half
/// made in the local one, the rest of the settings folder as it was, and
/// the Python choice naming the moved interpreter. When the move was a
/// copy, the Python environment is not copied: it is whole where it was,
/// and the choice still names it there.
fn check_moved_whole(folders: &Folders, before: &BTreeMap<String, Vec<u8>>, copied: bool) {
    let settings = tree(&folders.settings);
    let local = tree(&folders.local);
    let moved: &[&str] = if copied { &[CONFIGS, JOBS] } else { &MOVED };
    if copied {
        assert_eq!(under(&settings, PYTHON), under(before, PYTHON));
        assert!(under(&local, PYTHON).is_empty());
        assert!(!local.keys().any(|path| path.starts_with(".python.")));
        let choice = PythonChoice::load(&folders.settings).unwrap();
        assert_eq!(
            choice.interpreter,
            environment_interpreter(&folders.settings.join(PYTHON))
        );
    }
    for &what in moved {
        assert_eq!(
            under(&local, what),
            under(before, what),
            "{what} in the local folder"
        );
        assert!(
            !settings.keys().any(|path| {
                path.starts_with(&format!("{what}/")) || path.starts_with(&format!(".{what}."))
            }),
            "{what} left in the settings folder: {:?}",
            settings.keys().collect::<Vec<_>>()
        );
        assert!(
            !local
                .keys()
                .any(|path| path.starts_with(&format!(".{what}"))),
            "a staging folder or marker of {what} left: {:?}",
            local.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(the_rest(&settings), the_rest(before));
    assert_eq!(local["logs/session.log"], b"a session\n");
    let choice = PythonChoice::load(&folders.settings).unwrap();
    if !copied {
        assert_eq!(
            choice.interpreter,
            environment_interpreter(&folders.local.join(PYTHON))
        );
    }
    assert_eq!(choice.setup, Setup::Managed);
}

/// The settled folders after a copy: the environment left where it was,
/// its successor to be made in the local folder.
fn check_environment_left(settled: &DataFolders, folders: &Folders) {
    assert_eq!(settled.python_environment, folders.local.join(PYTHON));
    assert_eq!(
        settled.python_left.as_deref(),
        Some(folders.settings.join(PYTHON).as_path())
    );
}

/// What the stores show once moved: the history and the run, read where
/// the folders say.
fn check_stores_read(folders: &DataFolders) {
    let configs = ConfigStore::new(&folders.configs);
    let versions = configs
        .versions(WORKSPACE, &HostId::new("Site/r1.yaml"))
        .unwrap();
    assert_eq!(versions.len(), 1);
    let runs = RunStore::new(&folders.jobs).list(WORKSPACE).unwrap();
    assert_eq!(
        runs.runs
            .iter()
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>(),
        [RUN],
        "{:?}",
        runs.damaged
    );
}

/// The settings folder holds none of the three and the local folder only
/// its logs: nothing changes, nothing is said, and the stores go to the
/// local folder. One folder for both (`ENODIX_DATA_DIR`) is used as it
/// is, with no lock file written.
#[test]
fn nothing_to_move_changes_nothing() {
    let folders = Folders::new();
    let (settings, local) = (tree(&folders.settings), tree(&folders.local));
    let settled = folders.settle(false);
    assert!(settled.moved.is_empty());
    assert!(settled.notices.is_empty(), "{:?}", settled.notices);
    assert!(moved_to_local(&settled.folders, &folders.local));
    assert_eq!(settled.folders.settings, folders.settings);
    assert_eq!(tree(&folders.settings), settings);
    assert_eq!(tree(&folders.local), local);

    let one = folders.local.parent().unwrap().join("one");
    fs::create_dir_all(&one).unwrap();
    let settled = settle(&one, &one);
    assert_eq!(settled.folders, DataFolders::in_one(&one));
    assert_eq!(settled.folders.configs, one.join(CONFIGS));
    assert!(
        fs::read_dir(&one).unwrap().next().is_none(),
        "wrote into it"
    );
}

/// Everything an older version kept moves by a rename: whole, the
/// read-only file included, the old folders gone, the settings and logs
/// untouched, the Python choice pointed at the moved interpreter, and the
/// stores read it in the new place. The next start finds nothing to do.
#[test]
fn everything_moves_whole_and_what_stays_stays() {
    let folders = Folders::new().with_old_data(5);
    let before = tree(&folders.settings);
    let settled = folders.settle(false);
    assert_eq!(settled.moved, MOVED);
    assert!(settled.notices.is_empty(), "{:?}", settled.notices);
    assert!(moved_to_local(&settled.folders, &folders.local));
    assert_eq!(settled.folders.python_left, None);
    check_moved_whole(&folders, &before, false);
    check_stores_read(&settled.folders);
    let read_only = folders
        .local
        .join(PYTHON)
        .join("Lib/site-packages/enodix/RECORD");
    assert!(fs::metadata(read_only).unwrap().permissions().readonly());
    drop(settled);

    let after = (tree(&folders.settings), tree(&folders.local));
    let again = folders.settle(false);
    assert!(again.moved.is_empty() && again.notices.is_empty());
    assert!(moved_to_local(&again.folders, &folders.local));
    assert_eq!((tree(&folders.settings), tree(&folders.local)), after);
}

/// Across drives (played as a copy) the config backups and job results are
/// copied, checked and the old folders removed, the read-only file kept
/// read-only. The Python environment is not copied: it goes on working
/// where it is, said at every start, until it is set up again in the local
/// folder; then the old one is removed, and nothing more is said.
#[test]
fn across_drives_the_data_is_copied_and_the_environment_set_up_again() {
    let folders = Folders::new().with_old_data(5);
    let before = tree(&folders.settings);
    let read_only = Path::new("Lib/site-packages/enodix/RECORD");
    // Read-only in the jobs too, to be copied.
    let run_file = folders
        .settings
        .join(JOBS)
        .join(WORKSPACE)
        .join(RUN)
        .join("output/r1.txt");
    let mut permissions = fs::metadata(&run_file).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&run_file, permissions).unwrap();

    for start in 0..2 {
        let settled = folders.settle(true);
        let expected: &[&str] = if start == 0 { &[CONFIGS, JOBS] } else { &[] };
        assert_eq!(settled.moved, expected);
        let kinds: Vec<_> = settled
            .notices
            .iter()
            .map(|notice| (notice.what, notice.kind))
            .collect();
        assert_eq!(kinds, [(PYTHON, NoticeKind::SetUpAgain)], "start {start}");
        assert!(settled.notices[0].message.contains("set it up again"));
        check_moved_whole(&folders, &before, true);
        check_environment_left(&settled.folders, &folders);
        check_stores_read(&settled.folders);
        let copied = folders
            .local
            .join(JOBS)
            .join(WORKSPACE)
            .join(RUN)
            .join("output/r1.txt");
        assert!(fs::metadata(copied).unwrap().permissions().readonly());
        assert!(folders.settings.join(PYTHON).join(read_only).exists());
        // While the choice names it, the old environment is not removed.
        assert!(!remove_left_environment(&settled.folders).unwrap());
    }

    // Set up again in the local folder (as the setup does: the
    // environment, then the choice), the old one goes.
    let settled = folders.settle(true);
    let new_interpreter = environment_interpreter(&settled.folders.python_environment);
    fs::create_dir_all(new_interpreter.parent().unwrap()).unwrap();
    fs::write(&new_interpreter, "interpreter").unwrap();
    PythonChoice {
        interpreter: new_interpreter,
        setup: Setup::Managed,
    }
    .save(&folders.settings)
    .unwrap();
    assert!(remove_left_environment(&settled.folders).unwrap());
    assert!(!folders.settings.join(PYTHON).exists());
    drop(settled);
    let after = folders.settle(true);
    assert!(after.notices.is_empty(), "{:?}", after.notices);
    assert_eq!(after.folders.python_left, None);
    assert!(moved_to_local(&after.folders, &folders.local));

    // An old environment that a removal missed, beside the new one the
    // choice names, goes at the next start.
    fs::create_dir_all(folders.settings.join(PYTHON).join("Lib")).unwrap();
    drop(after);
    let again = folders.settle(true);
    assert!(again.notices.is_empty(), "{:?}", again.notices);
    assert!(!folders.settings.join(PYTHON).exists());
}

/// Reads the helper's lines until one starts with `prefix` (`None` if it
/// ends first or `within` passes).
fn line_from(child: &mut Child, prefix: &str, within: Duration) -> Option<String> {
    let stdout = child.stdout.take().expect("the helper's output is piped");
    let (send, receive) = std::sync::mpsc::channel();
    let wanted = prefix.to_string();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.starts_with(&wanted) {
                let _ = send.send(line);
                return;
            }
        }
    });
    receive.recv_timeout(within).ok()
}

/// A move cut short at any point (the process killed: power lost, a full
/// disk, the app ended in Task Manager) leaves the data whole in one place
/// that the next start recognises: before the commit the old folders are
/// the data and the copy is thrown away; after it the copy is, and what is
/// left of the old folder is removed. Either way the move then completes,
/// with nothing lost.
#[test]
fn a_move_cut_short_anywhere_is_finished_at_the_next_start() {
    for stop in [
        "copied configs 2",
        "copied all configs",
        "in place jobs",
        "committed jobs",
        "moved jobs",
    ] {
        let folders = Folders::new().with_old_data(4);
        let before = tree(&folders.settings);
        let mut child = folders.helper(true, Some(stop), None);
        let stopped = line_from(&mut child, STOPPED, Duration::from_secs(60));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(stopped.is_some(), "the helper never reached {stop:?}");

        // Whatever the moment, every piece is whole somewhere.
        let (settings, local) = (tree(&folders.settings), tree(&folders.local));
        for what in MOVED {
            let old = under(&before, what);
            let in_settings = under(&settings, what);
            let aside = under(&settings, &format!(".{what}.moved"));
            let in_local = under(&local, what);
            assert!(
                in_settings == old || aside == old || in_local == old,
                "{stop}: {what} is whole nowhere"
            );
        }

        let settled = folders.settle(true);
        let kinds: Vec<_> = settled.notices.iter().map(|n| (n.what, n.kind)).collect();
        assert_eq!(kinds, [(PYTHON, NoticeKind::SetUpAgain)], "{stop}");
        assert_eq!(settled.folders.configs, folders.local.join(CONFIGS));
        assert_eq!(settled.folders.jobs, folders.local.join(JOBS));
        check_environment_left(&settled.folders, &folders);
        check_moved_whole(&folders, &before, true);
        check_stores_read(&settled.folders);
    }
}

/// When the local folder already holds one of the three (an older version
/// run after the move kept new data in the old place), neither is changed
/// or merged: the local one is used and the old one is reported, at every
/// start. The others move as ever.
#[test]
fn a_local_folder_already_holding_data_is_used_and_the_old_left_alone() {
    let folders = Folders::new().with_old_data(2);
    let held = folders.local.join(CONFIGS).join(WORKSPACE).join("Site/r9");
    fs::create_dir_all(&held).unwrap();
    fs::write(held.join("2026-02-01_10-00-00.txt"), "hostname r9\n").unwrap();
    let before = tree(&folders.settings);
    let local_configs = under(&tree(&folders.local), CONFIGS);

    for start in 0..2 {
        let settled = folders.settle(false);
        let kinds: Vec<_> = settled
            .notices
            .iter()
            .map(|notice| (notice.what, notice.kind))
            .collect();
        assert_eq!(kinds, [(CONFIGS, NoticeKind::BothHold)], "start {start}");
        let message = &settled.notices[0].message;
        assert!(
            message.contains(&folders.settings.join(CONFIGS).display().to_string())
                && message.contains(&folders.local.join(CONFIGS).display().to_string()),
            "{message}"
        );
        assert_eq!(settled.folders.configs, folders.local.join(CONFIGS));
        assert_eq!(
            under(&tree(&folders.settings), CONFIGS),
            under(&before, CONFIGS)
        );
        assert_eq!(under(&tree(&folders.local), CONFIGS), local_configs);
        assert_eq!(settled.folders.jobs, folders.local.join(JOBS));
        assert_eq!(
            under(&tree(&folders.local), JOBS),
            under(&before, JOBS),
            "start {start}"
        );
    }
}

/// Holds a file of the settings folder so that it cannot be read (and, on
/// Windows, so that its folder cannot be renamed): opened without sharing
/// on Windows, made unreadable elsewhere. `None` where that is not
/// possible (a user who reads everything).
fn hold_unreadable(path: &Path) -> Option<Box<dyn FnOnce()>> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .unwrap();
        Some(Box::new(move || drop(held)))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let before = fs::metadata(path).unwrap().permissions();
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(path).is_ok() {
            fs::set_permissions(path, before).unwrap();
            return None;
        }
        let path = path.to_path_buf();
        Some(Box::new(move || fs::set_permissions(path, before).unwrap()))
    }
}

/// A file of the old folder that cannot be read stops that folder's move
/// before anything is removed: it is said, the app goes on using it where
/// it is, nothing half copied stays in the local folder, and the others
/// move. Once the file can be read, the next start moves it.
#[test]
fn an_unreadable_file_stops_its_move_and_nothing_is_lost() {
    let modes: &[bool] = if cfg!(windows) {
        &[true, false]
    } else {
        &[true]
    };
    for &copy in modes {
        let folders = Folders::new().with_old_data(2);
        let before = tree(&folders.settings);
        let output = folders
            .settings
            .join(JOBS)
            .join(WORKSPACE)
            .join(RUN)
            .join("output/r1.txt");
        let Some(release) = hold_unreadable(&output) else {
            println!("an unreadable file SKIPPED: this user reads every file");
            return;
        };
        let settled = folders.settle(copy);
        release();
        let kinds: Vec<_> = settled
            .notices
            .iter()
            .map(|notice| (notice.what, notice.kind))
            .collect();
        let mut expected = vec![(JOBS, NoticeKind::NotMoved)];
        if copy {
            expected.push((PYTHON, NoticeKind::SetUpAgain));
        }
        assert_eq!(kinds, expected, "copy: {copy}");
        assert!(
            settled.notices[0].message.contains("Nothing was lost"),
            "{}",
            settled.notices[0].message
        );
        assert_eq!(settled.folders.jobs, folders.settings.join(JOBS));
        assert_eq!(settled.folders.configs, folders.local.join(CONFIGS));
        let moved: &[&str] = if copy { &[CONFIGS] } else { &[CONFIGS, PYTHON] };
        assert_eq!(settled.moved, moved);
        assert_eq!(under(&tree(&folders.settings), JOBS), under(&before, JOBS));
        let local = tree(&folders.local);
        assert!(
            !local
                .keys()
                .any(|path| path.starts_with("jobs/") || path.starts_with(".jobs.")),
            "copy: {copy}: {:?}",
            local.keys().collect::<Vec<_>>()
        );
        // The old place is the one the stores read meanwhile.
        assert_eq!(
            RunStore::new(&settled.folders.jobs)
                .list(WORKSPACE)
                .unwrap()
                .runs
                .len(),
            1
        );
        drop(settled);

        let again = folders.settle(copy);
        assert_eq!(again.moved, [JOBS]);
        assert_eq!(
            again.notices.len(),
            usize::from(copy),
            "{:?}",
            again.notices
        );
        check_moved_whole(&folders, &before, copy);
    }
}

/// Two copies of the app starting at the same moment move the data once:
/// one moves while the other waits, then finds it moved; both use the
/// local folder, and every file is there once, as it was.
#[test]
fn two_copies_starting_at_once_move_it_once() {
    let folders = Folders::new().with_old_data(200);
    let before = tree(&folders.settings);
    let go = folders.local.parent().unwrap().join("go");
    let mut children: Vec<Child> = (0..2)
        .map(|_| folders.helper(true, None, Some(&go)))
        .collect();
    // Both are started and waiting for the same moment.
    std::thread::sleep(Duration::from_millis(500));
    fs::write(&go, "").unwrap();
    let mut results = Vec::new();
    for child in &mut children {
        let line = line_from(child, SETTLED, Duration::from_secs(120));
        child.wait().unwrap();
        let line = line.expect("a copy did not settle");
        let value: serde_json::Value =
            serde_json::from_str(line.strip_prefix(SETTLED).unwrap()).unwrap();
        results.push(value);
    }
    let local = folders.local.display().to_string();
    for result in &results {
        for key in ["configs", "jobs"] {
            assert!(
                result["folders"][key]
                    .as_str()
                    .is_some_and(|folder| folder.starts_with(&local)),
                "{result}"
            );
        }
    }
    let moved: Vec<usize> = results
        .iter()
        .map(|result| result["moved"].as_array().unwrap().len())
        .collect();
    assert!(
        moved == [2, 0] || moved == [0, 2],
        "both moved, or neither: {results:?}"
    );
    // Copied, so the environment stays where it was, whole.
    check_moved_whole(&folders, &before, true);
}

/// A copy of the app that starts while another runs (and uses the data
/// where it is) moves nothing: it uses the data where it is too, and says
/// that the move waits. Once it runs alone, a start moves it.
#[test]
fn a_copy_started_while_another_runs_leaves_the_data_where_it_is() {
    let folders = Folders::new();
    let running = folders.settle(false);
    assert!(running.notices.is_empty());
    // Data the running copy does not move (an older version beside it).
    let folders = folders.with_old_data(1);
    let before = tree(&folders.settings);
    let second = folders.settle(false);
    assert!(second.moved.is_empty());
    let kinds: Vec<_> = second
        .notices
        .iter()
        .map(|notice| (notice.what, notice.kind))
        .collect();
    assert_eq!(
        kinds,
        MOVED.map(|what| (what, NoticeKind::OtherCopyRunning)),
        "{:?}",
        second.notices
    );
    assert_eq!(second.folders.configs, folders.settings.join(CONFIGS));
    assert_eq!(
        second.folders.python_environment,
        folders.settings.join(PYTHON)
    );
    assert_eq!(tree(&folders.settings), before);
    drop(second);
    drop(running);

    let alone = folders.settle(false);
    assert_eq!(alone.moved, MOVED);
    check_moved_whole(&folders, &before, false);
}

/// A real Python environment works from its new folder once renamed (the
/// decision of #128: moved, not set up again, on one drive): the app's
/// check finds it ready, with the app's package. Across drives (a copy) it
/// is left where it is and still ready there. SKIPPED without Python 3.10+
/// or Node.
#[test]
fn a_moved_python_environment_still_works() {
    use enodix_core::python::{
        create_environment, find_python, package_version, status, system_candidates,
        SetupCanceller, SetupOffer,
    };
    let Ok(base) = find_python(&system_candidates()) else {
        println!("a moved environment SKIPPED: no Python 3.10 or newer found");
        return;
    };
    let wheels = TempDir::new().unwrap();
    let Some(wheel) = support::python::build_wheel(&wheels.path().join("wheel")) else {
        println!("a moved environment SKIPPED: Node cannot build the wheel");
        return;
    };
    let version = env!("CARGO_PKG_VERSION");
    let expected = package_version(version);
    for copy in [false, true] {
        let folders = Folders::new();
        let offer = SetupOffer::find(
            &[enodix_core::python::Candidate::new(&base.executable)],
            &folders.settings.join(PYTHON),
            wheel.parent().unwrap(),
            version,
        )
        .unwrap();
        create_environment(
            &offer.plan(&[]),
            &folders.settings,
            &expected,
            &SetupCanceller::new(),
            &mut |_| {},
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let started = Instant::now();
        let settled = folders.settle(copy);
        println!("settled (copy: {copy}) in {:?}", started.elapsed());
        let choice = PythonChoice::load(&folders.settings).unwrap();
        if copy {
            check_environment_left(&settled.folders, &folders);
            assert!(choice
                .interpreter
                .starts_with(folders.settings.join(PYTHON)));
        } else {
            assert!(settled.notices.is_empty(), "{:?}", settled.notices);
            assert!(!folders.settings.join(PYTHON).exists());
            assert!(choice.interpreter.starts_with(folders.local.join(PYTHON)));
        }
        let now = status(Some(&choice), &expected, None);
        assert!(now.ready(), "copy: {copy}: {now:?}");
    }
}
