//! Process trees for the tests that check what ends with what: helper
//! processes that are this test binary itself, run again in a role.
//!
//! A test binary that uses them has a `#[test] fn helper()` that calls
//! [`play`] (and plays roles of its own when [`role`] names one): run
//! normally it does nothing; run by [`helper`] with [`ROLE_VARIABLE`] set,
//! it plays the role and never returns.
//!
//! - [`TREE`]: a process that writes its id to `child.pid` in the folder
//!   [`PIDS_VARIABLE`] names, starts a [`LEAF`] (so that a stand-in
//!   interpreter running it has a child and a grandchild), prints
//!   [`TREE_STARTED`] and waits for a minute.
//! - [`LEAF`]: writes its id to `grandchild.pid` and waits for a minute.
//!
//! The minute is a bound on what a failed test leaves behind; every check
//! waits on the processes being gone, never on a time. The tests end only
//! processes they started themselves (from the handle of the spawn).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The role a helper plays.
pub const ROLE_VARIABLE: &str = "ENODIX_TEST_ROLE";
/// The folder the helpers write their process ids to.
pub const PIDS_VARIABLE: &str = "ENODIX_TEST_PIDS";
pub const TREE: &str = "tree";
pub const LEAF: &str = "leaf";
/// What a [`TREE`] prints once its leaf is started.
pub const TREE_STARTED: &str = "tree started";
/// The pid files of a tree's two processes.
pub const CHILD: &str = "child.pid";
pub const GRANDCHILD: &str = "grandchild.pid";

/// How long a helper waits before it ends on its own.
const LIFETIME: Duration = Duration::from_secs(60);

/// The role this process was started to play, if any.
pub fn role() -> Option<String> {
    std::env::var(ROLE_VARIABLE)
        .ok()
        .filter(|role| !role.is_empty())
}

/// This test binary, run as a helper (`--exact helper`) in `role`, writing
/// its ids to `pids`.
pub fn helper(role: &str, pids: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "helper", "--nocapture", "--test-threads=1", "-q"])
        .env(ROLE_VARIABLE, role)
        .env(PIDS_VARIABLE, pids);
    command
}

/// The command line that runs a [`TREE`] (for a stand-in interpreter's
/// script: the variables come from the environment it is given).
pub fn tree_line() -> String {
    format!(
        "\"{}\" --exact helper --nocapture --test-threads=1 -q",
        std::env::current_exe().unwrap().display()
    )
}

/// A stand-in interpreter (`support::python`) that runs a [`TREE`]
/// writing to `pids`, whatever environment it is started with (a Python
/// setup's steps get none of their own).
pub fn tree_python(folder: &Path, name: &str, pids: &Path) -> super::python::FakePython {
    let line = tree_line();
    super::python::FakePython::running(
        folder,
        name,
        &format!(
            "set \"{ROLE_VARIABLE}={TREE}\"\r\nset \"{PIDS_VARIABLE}={}\"\r\n{line}",
            pids.display()
        ),
        &format!(
            "{ROLE_VARIABLE}={TREE} {PIDS_VARIABLE}='{}' {line}",
            pids.display()
        ),
    )
}

/// The environment a stand-in interpreter running [`tree_line`] needs.
pub fn tree_environment(pids: &Path) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    vec![
        (ROLE_VARIABLE.into(), TREE.into()),
        (PIDS_VARIABLE.into(), pids.into()),
    ]
}

/// Plays [`TREE`] or [`LEAF`] if this process was started as one (never
/// returning); else returns.
pub fn play() {
    match role().as_deref() {
        Some(TREE) => tree(),
        Some(LEAF) => leaf(),
        _ => {}
    }
}

fn pids() -> PathBuf {
    PathBuf::from(std::env::var_os(PIDS_VARIABLE).expect("the helper knows where to write"))
}

/// Writes this process's id to `name` in `folder`, whole.
pub fn write_pid(folder: &Path, name: &str) {
    let temporary = folder.join(format!(".{name}.tmp"));
    fs::write(&temporary, std::process::id().to_string()).unwrap();
    fs::rename(&temporary, folder.join(name)).unwrap();
}

fn tree() -> ! {
    let folder = pids();
    write_pid(&folder, CHILD);
    // Started plainly, as a script starts a program: it inherits what the
    // tree has (its process group, its job object, its output).
    let _leaf = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "helper", "--nocapture", "--test-threads=1", "-q"])
        .env(ROLE_VARIABLE, LEAF)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    while !folder.join(GRANDCHILD).exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("{TREE_STARTED}");
    std::thread::sleep(LIFETIME);
    std::process::exit(0)
}

fn leaf() -> ! {
    write_pid(&pids(), GRANDCHILD);
    std::thread::sleep(LIFETIME);
    std::process::exit(0)
}

/// The ids in `names` under `folder`, once they are all written (`None`
/// after `within`).
pub fn wait_for_pids(folder: &Path, names: &[&str], within: Duration) -> Option<Vec<u32>> {
    let until = Instant::now() + within;
    loop {
        let read: Option<Vec<u32>> = names
            .iter()
            .map(|name| {
                fs::read_to_string(folder.join(name))
                    .ok()?
                    .trim()
                    .parse()
                    .ok()
            })
            .collect();
        if read.is_some() {
            return read;
        }
        if Instant::now() > until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits up to `within` for every process in `pids` to be gone; the ones
/// still running then.
pub fn wait_gone(pids: &[u32], within: Duration) -> Vec<u32> {
    let until = Instant::now() + within;
    loop {
        let alive: Vec<u32> = pids.iter().copied().filter(|pid| running(*pid)).collect();
        if alive.is_empty() || Instant::now() > until {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether the process `pid` is running (a process that has ended but
/// not been reaped is not).
#[cfg(windows)]
pub fn running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    // SAFETY: plain calls; the handle is closed here.
    unsafe {
        let process = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if process.is_null() {
            return false;
        }
        let still = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
        CloseHandle(process);
        still
    }
}

#[cfg(unix)]
pub fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    // A zombie has ended.
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_none_or(|state| state != "Z"),
        Err(_) => !Path::new("/proc").is_dir(),
    }
}
