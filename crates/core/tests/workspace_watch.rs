//! Live reload: a temporary workspace is changed on disk the way git or a
//! text editor would, and the watcher's change notifications are checked,
//! then what the reloaded workspace shows.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use enodix_core::session::{SessionState, SessionTarget, TerminalSize};
use enodix_core::workspace::{
    Changes, HostId, WatchOptions, Workspace, WorkspaceWatcher, FOLDER_SETTINGS_FILE,
};
use support::workspaces::fixture_copy;
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

/// The longest a change may take to be reported ("within a couple of
/// seconds").
const REPORTED_WITHIN: Duration = Duration::from_secs(2);

/// How long a test waits to be sure no further notification comes.
const SETTLE: Duration = Duration::from_secs(1);

/// A writable copy of the `edit` fixture, opened and watched.
struct Watched {
    // Dropped last: the watcher goes before its folder.
    _temp: TempDir,
    root: PathBuf,
    workspace: Workspace,
    changes: Receiver<Changes>,
    _watcher: WorkspaceWatcher,
}

fn watched() -> Watched {
    watched_with(WatchOptions::default())
}

fn watched_with(options: WatchOptions) -> Watched {
    let (temp, root) = fixture_copy("edit");
    let workspace = Workspace::open(&root).unwrap();
    let (sender, changes) = mpsc::channel();
    let watcher = WorkspaceWatcher::start(&root, options, move |change| {
        let _ = sender.send(change);
    })
    .unwrap();
    Watched {
        _temp: temp,
        root,
        workspace,
        changes,
        _watcher: watcher,
    }
}

impl Watched {
    /// The next notification; fails if none comes in time.
    fn next_change(&self) -> Changes {
        let started = Instant::now();
        let change = self
            .changes
            .recv_timeout(REPORTED_WITHIN)
            .expect("no change notification in time");
        assert!(started.elapsed() <= REPORTED_WITHIN);
        change
    }

    /// Waits for the next notification, which must name every file in
    /// `files`, and reloads the workspace, which must report a change.
    fn reload_after_change(&mut self, files: &[&str]) -> Changes {
        let change = self.next_change();
        for file in files {
            assert!(
                change.files.iter().any(|changed| changed == file),
                "{file} not in {:?}",
                change.files
            );
        }
        assert!(
            self.workspace.refresh().unwrap(),
            "the reload saw no change"
        );
        change
    }

    /// Fails if a notification arrives within [`SETTLE`].
    fn assert_no_change(&self) {
        match self.changes.recv_timeout(SETTLE) {
            Err(RecvTimeoutError::Timeout) => {}
            other => panic!("unexpected notification: {other:?}"),
        }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.root.join(file)
    }

    fn has_host(&self, id: &str) -> bool {
        self.workspace.host(&HostId::new(id)).is_some()
    }
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
}

#[test]
fn a_host_file_added_on_disk_appears() {
    let mut w = watched();

    write(&w.path("Core/core-r3.yaml"), "address: 10.0.0.3\n");

    w.reload_after_change(&["Core/core-r3.yaml"]);
    let host = w.workspace.host(&HostId::new("Core/core-r3.yaml")).unwrap();
    assert_eq!(host.address, "10.0.0.3");
}

#[test]
fn a_host_file_edited_on_disk_shows_its_new_fields() {
    let mut w = watched();

    write(
        &w.path("Access/sw1.yaml"),
        "address: 10.9.9.9\nvendor: Juniper\n",
    );

    w.reload_after_change(&["Access/sw1.yaml"]);
    let host = w.workspace.host(&HostId::new("Access/sw1.yaml")).unwrap();
    assert_eq!(host.address, "10.9.9.9");
    assert_eq!(host.vendor.as_deref(), Some("Juniper"));
}

#[test]
fn a_host_file_moved_on_disk_moves_in_the_tree() {
    let mut w = watched();

    fs::rename(w.path("Access/sw1.yaml"), w.path("Core/sw1.yaml")).unwrap();

    w.reload_after_change(&["Access/sw1.yaml", "Core/sw1.yaml"]);
    assert!(!w.has_host("Access/sw1.yaml"));
    assert!(w.has_host("Core/sw1.yaml"));
}

#[test]
fn a_host_file_deleted_on_disk_disappears() {
    let mut w = watched();

    fs::remove_file(w.path("Core/core-r1.yaml")).unwrap();

    w.reload_after_change(&["Core/core-r1.yaml"]);
    assert!(!w.has_host("Core/core-r1.yaml"));
    assert!(w.has_host("Core/core-r2.yml"));
}

#[test]
fn folders_added_moved_and_deleted_on_disk_follow() {
    let mut w = watched();

    fs::create_dir(w.path("Edge")).unwrap();
    w.reload_after_change(&["Edge"]);
    assert!(w.workspace.folder("Edge").is_some());

    fs::rename(w.path("Edge"), w.path("Core/Edge")).unwrap();
    w.reload_after_change(&["Edge", "Core/Edge"]);
    assert!(w.workspace.folder("Edge").is_none());
    assert!(w.workspace.folder("Core/Edge").is_some());

    fs::remove_dir_all(w.path("Access")).unwrap();
    w.reload_after_change(&["Access"]);
    assert!(w.workspace.folder("Access").is_none());
    assert!(!w.has_host("Access/sw1.yaml"));
}

#[test]
fn a_folder_settings_file_edited_on_disk_changes_what_is_inherited() {
    let mut w = watched();

    write(
        &w.path(&format!("Access/{FOLDER_SETTINGS_FILE}")),
        "description: Access layer\nport: 2222\n",
    );

    w.reload_after_change(&["Access/_folder.yaml"]);
    assert_eq!(
        w.workspace.folder("Access").unwrap().description.as_deref(),
        Some("Access layer")
    );
    let resolved = w
        .workspace
        .resolve_host(&HostId::new("Access/sw1.yaml"))
        .unwrap();
    assert_eq!(resolved.settings.port.unwrap().value, 2222);
}

#[test]
fn a_burst_of_changes_is_reported_once() {
    let mut w = watched();

    // What a git pull does: many files at once, in several folders.
    fs::create_dir(w.path("Pulled")).unwrap();
    for n in 0..200 {
        let folder = if n % 2 == 0 { "Pulled" } else { "Core" };
        write(
            &w.path(&format!("{folder}/host-{n}.yaml")),
            &format!("address: 10.1.{}.{}\n", n / 250, n % 250),
        );
    }
    fs::remove_file(w.path("Access/sw1.yaml")).unwrap();

    w.reload_after_change(&["Pulled/host-0.yaml", "Core/host-199.yaml"]);
    w.assert_no_change();
    assert_eq!(w.workspace.folder("Pulled").unwrap().hosts.len(), 100);
    assert!(w.has_host("Core/host-1.yaml"));
    assert!(!w.has_host("Access/sw1.yaml"));
}

#[test]
fn a_change_that_never_stops_is_still_reported_in_time() {
    let w = watched_with(WatchOptions {
        quiet: Duration::from_millis(300),
        max_delay: Duration::from_millis(800),
        ..WatchOptions::default()
    });

    // A change every 100 ms never leaves the 300 ms of quiet; the notification
    // still comes, after at most `max_delay`.
    let started = Instant::now();
    let path = w.path("Core/busy.yaml");
    let mut received = None;
    for n in 0..30 {
        write(&path, &format!("address: 10.2.0.{n}\n"));
        if let Ok(change) = w.changes.recv_timeout(Duration::from_millis(100)) {
            received = Some(change);
            break;
        }
    }
    let change = received.expect("no notification while changes kept coming");
    assert!(started.elapsed() < REPORTED_WITHIN);
    assert!(change.files.iter().any(|file| file == "Core/busy.yaml"));
}

#[test]
fn changes_inside_hidden_folders_such_as_git_are_not_reported() {
    let w = watched();

    fs::create_dir_all(w.path(".git/objects/ab")).unwrap();
    write(&w.path(".git/objects/ab/cdef"), "blob");
    write(&w.path(".git/HEAD"), "ref: refs/heads/main\n");
    write(&w.path("Core/.core-r1.yaml.swp"), "editor swap file");

    w.assert_no_change();
}

#[test]
fn the_apps_own_edits_need_no_reload() {
    let mut w = watched();

    let id = w
        .workspace
        .create_host("Core", "core-r9", "10.0.0.9")
        .unwrap();
    let mut host = w.workspace.host(&id).unwrap().clone();
    host.vendor = Some("Cisco".into());
    w.workspace.write_host(&host).unwrap();
    w.workspace.rename_folder("Empty", "Spare").unwrap();
    w.workspace.move_host(&id, "Spare").unwrap();

    // The files did change, so the watcher reports them; but the workspace
    // already shows what is on disk, so reloading finds nothing new.
    let change = w.next_change();
    assert!(
        change.files.iter().any(|file| file.starts_with("Spare")),
        "{:?}",
        change.files
    );
    let revision = w.workspace.revision();
    assert!(!w.workspace.refresh().unwrap());
    assert_eq!(w.workspace.revision(), revision);
    assert!(w.has_host("Spare/core-r9.yaml"));
}

#[test]
fn each_reload_that_changes_something_is_a_newer_revision() {
    let mut w = watched();
    assert_eq!(w.workspace.revision(), 0);

    w.workspace
        .create_host("Core", "core-r9", "10.0.0.9")
        .unwrap();
    assert_eq!(w.workspace.revision(), 1);

    write(&w.path("Core/core-r8.yaml"), "address: 10.0.0.8\n");
    w.reload_after_change(&["Core/core-r8.yaml"]);
    assert_eq!(w.workspace.revision(), 2);

    // An edit that changes nothing writes nothing, and is no new revision.
    let host = w
        .workspace
        .host(&HostId::new("Core/core-r8.yaml"))
        .unwrap()
        .clone();
    w.workspace.write_host(&host).unwrap();
    assert_eq!(w.workspace.revision(), 2);
}

#[test]
fn a_reload_keeps_what_the_app_saves_next_in_step_with_the_file() {
    let mut w = watched();

    // Someone adds a key the app does not know; the app then saves the host.
    write(
        &w.path("Access/sw1.yaml"),
        "address: 10.0.1.1\nmonitoring: zabbix\n",
    );
    w.reload_after_change(&["Access/sw1.yaml"]);
    let mut host = w
        .workspace
        .host(&HostId::new("Access/sw1.yaml"))
        .unwrap()
        .clone();
    host.vendor = Some("Arista".into());
    w.workspace.write_host(&host).unwrap();

    assert_eq!(
        fs::read_to_string(w.path("Access/sw1.yaml")).unwrap(),
        "address: 10.0.1.1\nvendor: Arista\nmonitoring: zabbix\n"
    );
}

#[test]
fn no_notification_comes_once_the_watcher_is_dropped() {
    let w = watched();
    let Watched {
        _temp,
        root,
        changes,
        _watcher,
        ..
    } = w;
    drop(_watcher);

    write(&root.join("Core/late.yaml"), "address: 10.0.0.50\n");

    match changes.recv_timeout(SETTLE) {
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        Ok(change) => panic!("notified after being dropped: {change:?}"),
    }
}

// Multi-threaded: waiting for a notification blocks this thread, and the
// test server and session keep running meanwhile.
#[tokio::test(flavor = "multi_thread")]
async fn an_open_session_survives_its_host_file_being_deleted() {
    let server = TestSshServer::start().await;
    let mut w = watched();
    write(
        &w.path("Core/lab.yaml"),
        &format!("address: 127.0.0.1\nport: {}\n", server.port()),
    );
    w.reload_after_change(&["Core/lab.yaml"]);

    let manager = support::manager();
    let target = w
        .workspace
        .resolve_host(&HostId::new("Core/lab.yaml"))
        .unwrap()
        .ssh_target(USERNAME.into(), PASSWORD.into());
    let (id, mut events) = manager.open(
        SessionTarget::Ssh(target),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;

    fs::remove_file(w.path("Core/lab.yaml")).unwrap();
    w.reload_after_change(&["Core/lab.yaml"]);
    assert!(!w.has_host("Core/lab.yaml"));

    manager.write(id, b"size\r".to_vec()).unwrap();
    let expected = [b"size\r\n80x24\r\n".as_slice(), PROMPT].concat();
    assert_eq!(output_ending_with(&mut events, PROMPT).await, expected);
}
