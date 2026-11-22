//! Reconnecting a session to a workspace host (#130): the target is worked
//! out again from the host's settings and credential profile as they are
//! now, as a new connection from the tree would; a typed login is kept
//! unless the host has another usable profile now; a host that is gone or
//! can no longer be reached that way is said so; a quick connection, or a
//! session of a workspace that is not open, reconnects as it was made.

mod support;

use std::fs;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::job::SessionLogin;
use enodix_core::session::{
    CloseReason, HostSession, HostWay, OpenError, OpenOptions, ReconnectRefused, Reconnection,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAuth, TerminalSize,
    TypedLogin,
};
use enodix_core::workspace::{HostId, Workspace};
use support::{
    closed_port, next_prompt, next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT,
    USERNAME,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A workspace made of `files` (relative path, text), with a store for its
/// profiles.
struct Lab {
    _temp: TempDir,
    workspace: Workspace,
    store: MemoryStore,
}

impl Lab {
    fn new(files: &[(&str, String)]) -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("lab");
        fs::create_dir_all(&root).unwrap();
        for (path, text) in files {
            write(&root, path, text);
        }
        let mut workspace = Workspace::open(&root).unwrap();
        workspace.ensure_id().unwrap();
        assert!(
            workspace.problems().is_empty(),
            "{:?}",
            workspace.problems()
        );
        Self {
            _temp: temp,
            workspace,
            store: MemoryStore::new(),
        }
    }

    fn profiles(&self) -> Profiles<'_> {
        Profiles::new(&self.store, &self.workspace)
    }

    /// Stores the profile `name` (in place of any of that name).
    fn profile(&self, name: &str, password: &str) {
        self.store
            .put(
                &self.workspace.credential_scope(),
                &CredentialProfile {
                    name: name.into(),
                    username: USERNAME.into(),
                    auth: ProfileAuth::Password {
                        password: password.into(),
                    },
                    enable_secret: None,
                },
            )
            .unwrap();
    }

    /// Changes a file outside the app, and reloads (as live reload does).
    fn rewrite(&mut self, path: &str, text: &str) {
        write(self.workspace.root(), path, text);
        assert!(self.workspace.refresh().unwrap());
    }

    fn open(&self, manager: &SessionManager, id: &str, way: HostWay) -> (SessionId, SessionEvents) {
        let session = self.profiles().host_session(&HostId::new(id), way).unwrap();
        manager.open_host(session, SIZE, OpenOptions::default())
    }

    /// What reconnecting the session `id` works out now.
    fn reconnection(
        &self,
        manager: &SessionManager,
        id: SessionId,
    ) -> Result<Reconnection, ReconnectRefused> {
        let origin = manager.origin(id).unwrap().expect("a session to a host");
        Reconnection::work_out(&origin, Some(&self.profiles()))
    }

    /// Reconnects the session `id` as worked out now; the session worked
    /// out (its target, login and note).
    fn reconnect(&self, manager: &SessionManager, id: SessionId) -> (HostSession, SessionEvents) {
        let Reconnection::Again(session) = self.reconnection(manager, id).unwrap() else {
            panic!("the host is in the open workspace");
        };
        let events = manager
            .reconnect_host(id, session.clone(), SIZE, OpenOptions::default())
            .unwrap();
        (session, events)
    }
}

fn write(root: &std::path::Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

/// A host file for a server on the loopback address.
fn host(port: u16, extra: &str) -> String {
    format!("address: 127.0.0.1\nport: {port}\n{extra}")
}

fn typed(password: &str) -> HostWay {
    HostWay::Ssh {
        typed: Some(TypedLogin {
            username: USERNAME.into(),
            password: password.into(),
        }),
    }
}

const PROFILE: HostWay = HostWay::Ssh { typed: None };

async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

async fn failed(events: &mut SessionEvents) -> OpenError {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    match next_state(events).await {
        SessionState::Closed(CloseReason::Failed(error)) => error,
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// Ends a connected session from the device's side.
async fn exit(manager: &SessionManager, id: SessionId, events: &mut SessionEvents) {
    manager.write(id, b"exit\r".to_vec()).unwrap();
    assert!(matches!(
        next_state(events).await,
        SessionState::Closed(CloseReason::RemoteExited { .. })
    ));
}

fn ssh(session: &HostSession) -> &enodix_core::session::SshTarget {
    match &session.target {
        SessionTarget::Ssh(target) => target,
        _ => panic!("an SSH session"),
    }
}

#[tokio::test]
async fn a_password_corrected_in_the_profile_is_used_by_the_reconnect() {
    let server = TestSshServer::start().await;
    let lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", "wrong password");
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    assert_eq!(failed(&mut events).await, OpenError::AuthenticationFailed);

    // The user corrects it in the Credentials view, then presses Enter.
    lab.profile("lab", PASSWORD);
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(ssh(&session).auth, SshAuth::Password(PASSWORD.into()));
    assert_eq!(session.login, SessionLogin::Profile);
    assert_eq!(session.note, None);
    connected(&mut events).await;
}

#[tokio::test]
async fn a_port_corrected_in_the_host_is_used_by_the_reconnect() {
    let server = TestSshServer::start().await;
    let wrong = closed_port().await;
    let mut lab = Lab::new(&[("r1.yaml", host(wrong, "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    assert!(matches!(
        failed(&mut events).await,
        OpenError::ConnectionRefused { .. }
    ));

    lab.rewrite("r1.yaml", &host(server.port(), "credential_profile: lab\n"));
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(ssh(&session).port, server.port());
    connected(&mut events).await;
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn a_jump_host_set_meanwhile_is_gone_through_by_the_reconnect() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let mut lab = Lab::new(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("router.yaml", host(router.port(), "")),
    ]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "router.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;
    assert_eq!(bastion.forwarded(), 0);

    lab.rewrite(
        "router.yaml",
        &host(router.port(), "jump_host: bastion.yaml\n"),
    );
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(ssh(&session).jump_hosts.len(), 1);
    connected(&mut events).await;
    assert_eq!(bastion.forwarded(), 1);
}

#[tokio::test]
async fn a_host_deleted_meanwhile_is_said_so() {
    let server = TestSshServer::start().await;
    let mut lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    lab.workspace.delete_host(&HostId::new("r1.yaml")).unwrap();
    let refused = lab.reconnection(&manager, id).err().unwrap();

    assert!(matches!(refused, ReconnectRefused::Gone { .. }));
    assert_eq!(refused.to_string(), "r1 is no longer in the workspace");
    assert!(refused.advice().unwrap().contains("Hosts view"));
    // Nothing was tried.
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn a_host_switched_to_another_protocol_is_said_so() {
    let server = TestSshServer::start().await;
    let mut lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    lab.rewrite(
        "r1.yaml",
        "address: 127.0.0.1\nprotocol: serial\nserial:\n  port: COM250\n",
    );
    let refused = lab.reconnection(&manager, id).err().unwrap();

    assert_eq!(
        refused.to_string(),
        "r1 is set to connect over serial now, and this is an SSH session"
    );
}

/// An SSH session opened on a telnet host (the side panel's SSH button)
/// stays SSH while the host's protocol is the one it had.
#[tokio::test]
async fn ssh_to_a_telnet_host_reconnects_over_ssh() {
    let server = TestSshServer::start().await;
    let lab = Lab::new(&[(
        "r1.yaml",
        host(server.port(), "protocol: telnet\ncredential_profile: lab\n"),
    )]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    let (_, mut events) = lab.reconnect(&manager, id);

    connected(&mut events).await;
}

#[tokio::test]
async fn a_typed_login_is_kept_with_the_settings_of_now() {
    let server = TestSshServer::start().await;
    let wrong = closed_port().await;
    let mut lab = Lab::new(&[("r1.yaml", host(wrong, ""))]);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", typed(PASSWORD));
    assert!(matches!(
        failed(&mut events).await,
        OpenError::ConnectionRefused { .. }
    ));

    lab.rewrite("r1.yaml", &host(server.port(), ""));
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(ssh(&session).auth, SshAuth::Password(PASSWORD.into()));
    assert_eq!(ssh(&session).port, server.port());
    assert_eq!(session.login, SessionLogin::Typed);
    assert_eq!(session.note, None);
    connected(&mut events).await;
    // A duplicate reconnects the same way.
    let (copy, _) = manager.duplicate(id, SIZE).unwrap();
    assert_eq!(manager.origin(copy).unwrap(), manager.origin(id).unwrap());
}

/// The password fall back after the profile's SSH agent failed: the host
/// still has that profile, so the typed password is what reconnects.
#[tokio::test]
async fn a_typed_login_beside_the_profile_it_replaced_is_kept() {
    let server = TestSshServer::start().await;
    let lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", "the agent's");
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", typed(PASSWORD));
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(ssh(&session).auth, SshAuth::Password(PASSWORD.into()));
    connected(&mut events).await;
}

/// The user typed a login, it failed, and they gave the host a profile.
#[tokio::test]
async fn a_typed_login_gives_way_to_a_profile_the_host_has_now() {
    let server = TestSshServer::start().await;
    let mut lab = Lab::new(&[("r1.yaml", host(server.port(), ""))]);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", typed("mistyped"));
    assert_eq!(failed(&mut events).await, OpenError::AuthenticationFailed);

    lab.profile("lab", PASSWORD);
    lab.rewrite("r1.yaml", &host(server.port(), "credential_profile: lab\n"));
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(session.login, SessionLogin::Profile);
    assert_eq!(
        session.note.as_deref(),
        Some("Logging in with the credential profile lab, which r1 has now.")
    );
    connected(&mut events).await;
    // From now on it is a profile login.
    exit(&manager, id, &mut events).await;
    let (session, _) = lab.reconnect(&manager, id);
    assert_eq!(session.note, None);
}

/// A host that named a profile not set up here: the user typed a login,
/// then set the profile up.
#[tokio::test]
async fn a_typed_login_gives_way_to_the_profile_set_up_meanwhile() {
    let server = TestSshServer::start().await;
    let lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", typed("mistyped"));
    assert_eq!(failed(&mut events).await, OpenError::AuthenticationFailed);

    lab.profile("lab", PASSWORD);
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(ssh(&session).auth, SshAuth::Password(PASSWORD.into()));
    connected(&mut events).await;
}

#[tokio::test]
async fn a_host_whose_profile_went_asks_for_the_login() {
    let server = TestSshServer::start().await;
    let mut lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    // The profile is removed from the Credentials view: not set up now.
    lab.profiles().delete("lab").unwrap();
    let Reconnection::Again(session) = lab.reconnection(&manager, id).unwrap() else {
        panic!("the host is in the open workspace");
    };
    assert_eq!(ssh(&session).auth, SshAuth::Prompt);
    assert_eq!(
        session.note.as_deref(),
        Some(
            "The credential profile lab is not set up on this computer: type the username and \
             password asked for, or set the profile up in the Credentials view."
        )
    );

    // The host no longer names one.
    lab.rewrite("r1.yaml", &host(server.port(), ""));
    let (session, mut events) = lab.reconnect(&manager, id);
    assert_eq!(
        session.note.as_deref(),
        Some("r1 has no credential profile now: type the username and password asked for.")
    );
    assert_eq!(session.login, SessionLogin::Typed);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let prompt = next_prompt(&mut events).await;
    let questions: Vec<_> = prompt.questions.iter().map(|q| q.text.as_str()).collect();
    assert_eq!(questions, ["Username: ", "Password: "]);
    manager
        .answer(id, vec![USERNAME.into(), PASSWORD.into()])
        .unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn a_profile_renamed_on_the_host_is_used_and_named() {
    let server = TestSshServer::start().await;
    let mut lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: old\n"))]);
    lab.profile("old", "wrong password");
    lab.profile("new", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    assert_eq!(failed(&mut events).await, OpenError::AuthenticationFailed);

    lab.rewrite("r1.yaml", &host(server.port(), "credential_profile: new\n"));
    let (session, mut events) = lab.reconnect(&manager, id);

    assert_eq!(
        session.note.as_deref(),
        Some("Logging in with the credential profile new, which r1 has now.")
    );
    connected(&mut events).await;
}

/// A host renamed in the app keeps its sessions (#99), and they reconnect
/// to it under its new id.
#[tokio::test]
async fn a_host_renamed_in_the_app_reconnects_under_its_new_id() {
    let server = TestSshServer::start().await;
    let mut lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    let moved = lab
        .workspace
        .rename_host(&HostId::new("r1.yaml"), "core1")
        .unwrap();
    assert_eq!(
        manager.move_hosts(lab.workspace.root(), &moved.hosts),
        1,
        "the session names the new id"
    );
    let (_, mut events) = lab.reconnect(&manager, id);

    connected(&mut events).await;
    let origin = manager.origin(id).unwrap().unwrap();
    assert_eq!(origin.host(), &HostId::new("core1.yaml"));
}

#[tokio::test]
async fn a_quick_connection_reconnects_as_made() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    // It belongs to no host: nothing to work out again.
    assert_eq!(manager.origin(id).unwrap(), None);
    let mut events = manager.reconnect(id, SIZE).unwrap();
    connected(&mut events).await;
}

/// A session kept open after switching to another workspace: its host's
/// settings are not at hand, so it reconnects as it was made, and says so.
#[tokio::test]
async fn a_session_of_a_workspace_not_open_reconnects_as_made() {
    let server = TestSshServer::start().await;
    let lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let other = Lab::new(&[("r1.yaml", host(1, "credential_profile: lab\n"))]);
    other.profile("lab", "another");
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;
    let origin = manager.origin(id).unwrap().unwrap();

    let note = "r1 is a host of a workspace that is not open: reconnecting with the settings \
                and login it was opened with.";
    for open in [None, Some(&other.profiles())] {
        match Reconnection::work_out(&origin, open).unwrap() {
            Reconnection::AsMade { note: said } => assert_eq!(said, note),
            Reconnection::Again(_) => panic!("not the open workspace's host"),
        }
    }
    let mut events = manager.reconnect(id, SIZE).unwrap();
    connected(&mut events).await;
}

/// A reconnect worked out again replaces what the session logs to and
/// does when connected, and starts at the terminal's size of now.
#[tokio::test]
async fn a_reconnect_worked_out_again_gets_the_size_of_now() {
    let server = TestSshServer::start().await;
    let lab = Lab::new(&[("r1.yaml", host(server.port(), "credential_profile: lab\n"))]);
    lab.profile("lab", PASSWORD);
    let manager = support::manager();
    let (id, mut events) = lab.open(&manager, "r1.yaml", PROFILE);
    connected(&mut events).await;
    exit(&manager, id, &mut events).await;

    let Reconnection::Again(session) = lab.reconnection(&manager, id).unwrap() else {
        panic!("the host is in the open workspace");
    };
    let size = TerminalSize {
        cols: 132,
        rows: 50,
    };
    let mut events = manager
        .reconnect_host(id, session, size, OpenOptions::default())
        .unwrap();
    connected(&mut events).await;
    manager.write(id, b"size\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, PROMPT).await;
    assert!(
        String::from_utf8_lossy(&output).contains("132x50"),
        "{output:?}"
    );
}
