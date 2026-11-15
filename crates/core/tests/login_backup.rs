//! Back up on login: a session to a host with `backup_on_login` on saves
//! the host's configuration once connected, over a second channel on the
//! same connection (or, for a profile login on a device that refuses one,
//! a second connection), without anything in the terminal.

mod support;

use std::fs;
use std::sync::Arc;

use netterm_core::configs::ConfigStore;
use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::job::{LoginBackup, LoginBackupOutcome, SessionLogin};
use netterm_core::layout::WindowId;
use netterm_core::session::{
    CloseReason, OpenError, OpenOptions, SessionError, SessionEvent, SessionEvents, SessionId,
    SessionManager, SessionRelays, SessionState, SessionTarget, SshTarget, TerminalSize,
};
use netterm_core::workspace::{HostId, Workspace};
use support::{next_state, TestSshServer, PASSWORD, USERNAME, WAIT};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A workspace made of `files` (relative path, text), with an id.
fn workspace(temp: &TempDir, files: &[(&str, String)]) -> Workspace {
    let root = temp.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let mut workspace = Workspace::open(&root).unwrap();
    workspace.ensure_id().unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    workspace
}

/// A host for the device-mode server `server`, driven by EOS (whose
/// ignored-lines pattern matches the device's time line), with `more`.
fn host(server: &TestSshServer, more: &str) -> String {
    format!(
        "address: 127.0.0.1\nport: {}\nplatform: eos\n{more}",
        server.port()
    )
}

fn profiles_store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: USERNAME.into(),
                auth: ProfileAuth::Password {
                    password: PASSWORD.into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    store
}

fn r1() -> HostId {
    HostId::new("Site/r1.yaml")
}

/// A site whose folder turns the backup on, with one device.
struct Site {
    temp: TempDir,
    workspace: Workspace,
    store: MemoryStore,
    configs: ConfigStore,
}

impl Site {
    fn new(server: &TestSshServer) -> Self {
        let temp = TempDir::new().unwrap();
        let workspace = workspace(
            &temp,
            &[
                (
                    "Site/_folder.yaml",
                    "credential_profile: lab\nbackup_on_login: true\n".into(),
                ),
                ("Site/r1.yaml", host(server, "")),
            ],
        );
        let store = profiles_store(&workspace);
        let configs = ConfigStore::new(temp.path().join("data").join("configs"));
        Self {
            temp,
            workspace,
            store,
            configs,
        }
    }

    fn profiles(&self) -> Profiles<'_> {
        Profiles::new(&self.store, &self.workspace)
    }

    fn backup(&self, login: SessionLogin) -> Option<LoginBackup> {
        LoginBackup::plan(
            &self.workspace,
            &self.profiles(),
            &r1(),
            login,
            self.configs.clone(),
        )
    }

    /// The target a terminal session logs in with.
    fn target(&self, login: SessionLogin) -> SshTarget {
        match login {
            SessionLogin::Profile => self.profiles().profile_target(&r1()).unwrap(),
            SessionLogin::Typed => self
                .profiles()
                .prompted_target(&r1(), USERNAME.into(), PASSWORD.into())
                .unwrap(),
        }
    }

    /// Opens a terminal session to r1 that backs it up once connected.
    fn open(&self, manager: &SessionManager, login: SessionLogin) -> (SessionId, SessionEvents) {
        let backup = self.backup(login).expect("the backup applies");
        manager.open_with(
            SessionTarget::Ssh(self.target(login)),
            SIZE,
            OpenOptions {
                when_connected: Some(Arc::new(backup)),
                ..OpenOptions::default()
            },
        )
    }

    fn id(&self) -> &str {
        self.workspace.id().unwrap()
    }

    fn versions(&self) -> Vec<String> {
        match self.configs.versions(self.id(), &r1()) {
            Ok(versions) => versions.into_iter().map(|v| v.id).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// What a terminal shows and the backup outcomes that arrived meanwhile.
#[derive(Default)]
struct Seen {
    output: Vec<u8>,
    backups: Vec<LoginBackupOutcome>,
}

/// Collects the terminal's output until it ends with `expected`, keeping
/// any backup outcome on the way.
async fn output_ending_with(events: &mut SessionEvents, expected: &[u8], seen: &mut Seen) {
    let start = seen.output.len();
    tokio::time::timeout(WAIT, async {
        while !seen.output[start..].ends_with(expected) {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    seen.output.extend_from_slice(&bytes);
                }
                Some(SessionEvent::Backup(outcome)) => seen.backups.push(outcome),
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for {:?}; the terminal showed {:?}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&seen.output)
        )
    });
}

/// Waits for the backup's outcome (unless one already arrived), keeping
/// the terminal's output.
async fn backup_outcome(events: &mut SessionEvents, seen: &mut Seen) -> LoginBackupOutcome {
    if let Some(outcome) = seen.backups.pop() {
        return outcome;
    }
    tokio::time::timeout(WAIT * 3, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    seen.output.extend_from_slice(&bytes);
                }
                Some(SessionEvent::Backup(outcome)) => return outcome,
                other => panic!("expected the backup outcome, got {other:?}"),
            }
        }
    })
    .await
    .expect("no backup outcome arrived")
}

/// Waits until the session (just opened) is connected and its prompt shows.
async fn connected(events: &mut SessionEvents, server: &TestSshServer, seen: &mut Seen) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, &server.prompt(), seen).await;
}

/// Types `show hostname` in the terminal and checks the answer at once.
async fn terminal_answers(
    manager: &SessionManager,
    id: SessionId,
    events: &mut SessionEvents,
    server: &TestSshServer,
    seen: &mut Seen,
) {
    manager.write(id, b"show hostname\r".to_vec()).unwrap();
    output_ending_with(events, &server.prompt(), seen).await;
    let shown = String::from_utf8_lossy(&seen.output).into_owned();
    assert!(shown.contains("show hostname\r\nr1\r\n"), "{shown:?}");
}

#[tokio::test]
async fn a_session_backs_up_over_a_second_channel_with_a_single_login() {
    let server = TestSshServer::start_device("r1").await;
    let site = Site::new(&server);
    let manager = support::manager();
    let (id, mut events) = site.open(&manager, SessionLogin::Profile);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;

    // The terminal is usable at once, whatever the backup is doing.
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;

    let LoginBackupOutcome::Saved { version } = backup_outcome(&mut events, &mut seen).await else {
        panic!("the backup did not save");
    };
    assert_eq!(site.versions(), std::slice::from_ref(&version));
    assert_eq!(
        site.configs.read(site.id(), &r1(), &version).unwrap(),
        server.running_config(1)
    );
    // One connection, one login: the backup used a second channel.
    assert_eq!(server.connections(), 1);
    assert_eq!(server.logins(), 1);
    // The terminal showed nothing of it.
    let shown = String::from_utf8_lossy(&seen.output).into_owned();
    assert!(!shown.contains("running-config"), "{shown:?}");
    assert!(!shown.contains("terminal length"), "{shown:?}");

    // The terminal goes on after it.
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;
    assert_eq!(manager.encrypted(id), Ok(true));
}

#[tokio::test]
async fn it_runs_again_after_a_reconnect() {
    let server = TestSshServer::start_device("r1").await;
    let site = Site::new(&server);
    let manager = support::manager();
    let (id, mut events) = site.open(&manager, SessionLogin::Profile);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;
    let first = backup_outcome(&mut events, &mut seen).await;
    assert!(
        matches!(first, LoginBackupOutcome::Saved { .. }),
        "{first:?}"
    );

    // Unchanged on the next connection (only the ignored time line differs).
    manager.close(id).unwrap();
    assert!(matches!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    ));
    let mut events = manager.reconnect(id, SIZE).unwrap();
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;
    let LoginBackupOutcome::Saved { version } = first else {
        unreachable!()
    };
    assert_eq!(
        backup_outcome(&mut events, &mut seen).await,
        LoginBackupOutcome::Unchanged {
            version: version.clone()
        }
    );

    // Changed by the user: the next connection keeps a second version.
    server.configure("interface Loopback1");
    manager.close(id).unwrap();
    next_state(&mut events).await;
    let mut events = manager.reconnect(id, SIZE).unwrap();
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;
    let LoginBackupOutcome::Saved { version: second } =
        backup_outcome(&mut events, &mut seen).await
    else {
        panic!("the changed configuration was not saved");
    };
    assert_ne!(second, version);
    assert_eq!(site.versions().len(), 2);
    assert!(site
        .configs
        .read(site.id(), &r1(), &second)
        .unwrap()
        .contains("interface Loopback1\n"));
    assert_eq!(server.logins(), 3);
}

#[tokio::test]
async fn a_device_refusing_a_second_channel_gets_a_second_connection_for_a_profile_login() {
    let server = TestSshServer::start_device("r1").await;
    server.refuse_second_channel();
    let site = Site::new(&server);
    let manager = support::manager();
    let (id, mut events) = site.open(&manager, SessionLogin::Profile);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;

    let outcome = backup_outcome(&mut events, &mut seen).await;
    assert!(
        matches!(outcome, LoginBackupOutcome::Saved { .. }),
        "{outcome:?}"
    );
    assert_eq!(site.versions().len(), 1);
    assert_eq!(server.connections(), 2);
    assert_eq!(server.logins(), 2);
    // The terminal's connection is the one that stays.
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;
}

#[tokio::test]
async fn a_typed_login_on_a_device_refusing_a_second_channel_gives_up_and_says_why() {
    let server = TestSshServer::start_device("r1").await;
    server.refuse_second_channel();
    let site = Site::new(&server);
    let manager = support::manager();
    let (id, mut events) = site.open(&manager, SessionLogin::Typed);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;

    let LoginBackupOutcome::Failed { reason } = backup_outcome(&mut events, &mut seen).await else {
        panic!("the backup did not fail");
    };
    assert!(reason.contains("refused a second channel"), "{reason}");
    assert!(reason.contains("typed"), "{reason}");
    assert!(site.versions().is_empty());
    assert_eq!(server.connections(), 1);
    assert_eq!(server.logins(), 1);
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;
}

#[tokio::test]
async fn a_typed_login_still_backs_up_over_a_second_channel() {
    let server = TestSshServer::start_device("r1").await;
    let site = Site::new(&server);
    let manager = support::manager();
    let (_id, mut events) = site.open(&manager, SessionLogin::Typed);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;
    let outcome = backup_outcome(&mut events, &mut seen).await;
    assert!(
        matches!(outcome, LoginBackupOutcome::Saved { .. }),
        "{outcome:?}"
    );
    assert_eq!(server.logins(), 1);
}

#[tokio::test]
async fn a_failed_backup_is_reported_with_its_reason_and_the_session_goes_on() {
    let server = TestSshServer::start_device("r1").await;
    let mut site = Site::new(&server);
    // The history cannot be written: its folder is a file.
    let blocked = site.temp.path().join("blocked");
    fs::write(&blocked, "not a folder").unwrap();
    site.configs = ConfigStore::new(&blocked);
    let manager = support::manager();
    let (id, mut events) = site.open(&manager, SessionLogin::Profile);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;

    let LoginBackupOutcome::Failed { reason } = backup_outcome(&mut events, &mut seen).await else {
        panic!("the backup did not fail");
    };
    assert!(
        reason.contains("the configuration could not be kept"),
        "{reason}"
    );
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;
}

#[tokio::test]
async fn no_backup_applies_when_the_setting_is_off_or_cannot_work() {
    let server = TestSshServer::start_device("r1").await;
    let temp = TempDir::new().unwrap();
    let workspace = workspace(
        &temp,
        &[
            ("_folder.yaml", "credential_profile: lab\n".into()),
            ("_platforms.yaml", "plain:\n  prompt: '[>#]'\n".into()),
            ("on.yaml", host(&server, "backup_on_login: true\n")),
            ("off.yaml", host(&server, "")),
            (
                "telnet.yaml",
                host(&server, "backup_on_login: true\nprotocol: telnet\n"),
            ),
            (
                "bare.yaml",
                format!(
                    "address: 127.0.0.1\nport: {}\nbackup_on_login: true\n",
                    server.port()
                ),
            ),
            (
                "plain.yaml",
                format!(
                    "address: 127.0.0.1\nport: {}\nplatform: plain\nbackup_on_login: true\n",
                    server.port()
                ),
            ),
        ],
    );
    let store = profiles_store(&workspace);
    let profiles = Profiles::new(&store, &workspace);
    let configs = ConfigStore::new(temp.path().join("configs"));
    let plan = |id: &str| {
        LoginBackup::plan(
            &workspace,
            &profiles,
            &HostId::new(id),
            SessionLogin::Profile,
            configs.clone(),
        )
    };
    assert_eq!(
        plan("on.yaml").map(|b| b.host().clone()),
        Some(HostId::new("on.yaml"))
    );
    // Off by default.
    assert!(plan("off.yaml").is_none());
    // Not for telnet hosts, hosts without a platform, or a platform
    // without a show-config command.
    assert!(plan("telnet.yaml").is_none());
    assert!(plan("bare.yaml").is_none());
    assert!(plan("plain.yaml").is_none());
    assert!(plan("gone.yaml").is_none());
}

#[tokio::test]
async fn a_second_channel_is_a_session_of_its_own_on_the_same_connection() {
    let server = TestSshServer::start_device("r1").await;
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;

    let (channel, mut channel_events) = manager.open_channel(id, SIZE).unwrap();
    let mut on_channel = Seen::default();
    connected(&mut channel_events, &server, &mut on_channel).await;
    manager.write(channel, b"echo beside\r".to_vec()).unwrap();
    output_ending_with(&mut channel_events, &server.prompt(), &mut on_channel).await;
    assert!(String::from_utf8_lossy(&on_channel.output).contains("beside\r\n"));
    assert_eq!(server.connections(), 1);
    assert_eq!(server.logins(), 1);

    // Closing it leaves the session open, and nothing of it showed there.
    manager.close(channel).unwrap();
    assert_eq!(
        next_state(&mut channel_events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;
    assert!(!String::from_utf8_lossy(&seen.output).contains("beside"));

    // Closing the session ends a channel still open on it.
    let (_, mut second) = manager.open_channel(id, SIZE).unwrap();
    assert_eq!(next_state(&mut second).await, SessionState::Connecting);
    assert_eq!(next_state(&mut second).await, SessionState::Connected);
    manager.close(id).unwrap();
    assert!(matches!(
        next_state(&mut second).await,
        SessionState::Closed(_)
    ));
}

#[tokio::test]
async fn a_refused_second_channel_closes_saying_so() {
    let server = TestSshServer::start_device("r1").await;
    server.refuse_second_channel();
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let mut seen = Seen::default();
    connected(&mut events, &server, &mut seen).await;

    let (_, mut channel_events) = manager.open_channel(id, SIZE).unwrap();
    assert_eq!(
        next_state(&mut channel_events).await,
        SessionState::Connecting
    );
    let SessionState::Closed(CloseReason::Failed(OpenError::ChannelRefused { .. })) =
        next_state(&mut channel_events).await
    else {
        panic!("the channel was not refused");
    };
    terminal_answers(&manager, id, &mut events, &server, &mut seen).await;

    // Only a connected SSH session has channels to open.
    manager.close(id).unwrap();
    next_state(&mut events).await;
    assert_eq!(
        manager.open_channel(id, SIZE).unwrap_err(),
        SessionError::NotFound
    );
}

#[tokio::test]
async fn the_outcome_is_kept_for_a_window_taking_the_session_later() {
    let server = TestSshServer::start_device("r1").await;
    let site = Site::new(&server);
    let manager = support::manager();
    let relays = SessionRelays::new();
    let (id, events) = site.open(&manager, SessionLogin::Profile);
    let (sender, mut received) = tokio::sync::mpsc::unbounded_channel();
    let key = relays.start(id, WindowId::MAIN, events, support::ToChannel(sender));
    let outcome = tokio::time::timeout(WAIT * 3, async {
        loop {
            match received.recv().await {
                Some(SessionEvent::Backup(outcome)) => return outcome,
                Some(SessionEvent::Output(bytes)) => relays.acknowledge(id, key, bytes.len()),
                Some(_) => {}
                None => panic!("the session ended first"),
            }
        }
    })
    .await
    .expect("no backup outcome arrived");

    relays.release(id, key).unwrap();

    assert!(
        matches!(outcome, LoginBackupOutcome::Saved { .. }),
        "{outcome:?}"
    );
    assert_eq!(relays.reported(id).unwrap().backup, Some(outcome));
}
