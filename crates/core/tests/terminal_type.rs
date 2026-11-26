//! The terminal type a session announces (#24): a setting inherited like
//! the others, used for the SSH pty request and for telnet's terminal type.

mod support;

use std::fs;

use enodix_core::session::{
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, TerminalSize,
    TerminalType,
};
use enodix_core::workspace::{Effective, HostId, Origin, Settings, Workspace};
use support::telnet::{TestTelnetServer, PASSWORD_PROMPT, TELNET_PROMPT, USERNAME_PROMPT};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

fn workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let write = |path: &str, text: &str| {
        let path = temp.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("Old/_folder.yaml", "terminal_type: vt100\n");
    write("Old/sw1.yaml", "address: 10.0.0.1\n");
    write("Old/sw2.yaml", "address: 10.0.0.2\nterminal_type: linux\n");
    write("new.yaml", "address: 10.0.0.3\n");
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

fn terminal_type(workspace: &Workspace, id: &str) -> Effective<TerminalType> {
    workspace
        .resolve_host(&HostId::new(id))
        .unwrap()
        .settings
        .terminal_type
}

#[test]
fn the_terminal_type_inherits_like_the_other_settings() {
    let (_temp, workspace) = workspace();

    assert_eq!(
        terminal_type(&workspace, "new.yaml"),
        Effective {
            value: TerminalType::Xterm256Color,
            origin: Origin::Default
        }
    );
    assert_eq!(
        terminal_type(&workspace, "Old/sw1.yaml"),
        Effective {
            value: TerminalType::Vt100,
            origin: Origin::Folder { path: "Old".into() }
        }
    );
    assert_eq!(
        terminal_type(&workspace, "Old/sw2.yaml"),
        Effective {
            value: TerminalType::Linux,
            origin: Origin::Host
        }
    );
}

#[test]
fn the_terminal_type_is_written_and_checked() {
    let (_temp, mut workspace) = workspace();

    let mut host = workspace.host(&HostId::new("new.yaml")).unwrap().clone();
    host.settings.terminal_type = Some(TerminalType::Ansi);
    workspace.write_host(&host).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.root().join("new.yaml")).unwrap(),
        "address: 10.0.0.3\nterminal_type: ansi\n"
    );

    host.settings.terminal_type = None;
    workspace.write_host(&host).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.root().join("new.yaml")).unwrap(),
        "address: 10.0.0.3\n"
    );

    let folder = Settings {
        terminal_type: Some(TerminalType::Xterm),
        ..Settings::default()
    };
    workspace.write_folder_settings("", &folder).unwrap();
    assert_eq!(
        fs::read_to_string(workspace.root().join("_folder.yaml")).unwrap(),
        "terminal_type: xterm\n"
    );
}

#[test]
fn an_unknown_terminal_type_is_reported_for_that_file() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("a.yaml"),
        "address: 10.0.0.1\nterminal_type: wyse60\n",
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    let problems = workspace.problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].message.contains("terminal_type"),
        "{}",
        problems[0].message
    );
}

#[test]
fn the_targets_of_a_host_carry_its_terminal_type() {
    let (_temp, workspace) = workspace();
    let resolved = workspace
        .resolve_host(&HostId::new("Old/sw1.yaml"))
        .unwrap();

    assert_eq!(
        resolved.ssh_target("u".into(), "p".into()).terminal_type,
        TerminalType::Vt100
    );
    assert_eq!(resolved.telnet_target().terminal_type, TerminalType::Vt100);
}

async fn ssh_term(terminal_type: Option<TerminalType>) -> Vec<u8> {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let mut target = server.target();
    if let Some(terminal_type) = terminal_type {
        target.terminal_type = terminal_type;
    }
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    manager.write(id, b"term\r".to_vec()).unwrap();
    output_ending_with(&mut events, PROMPT).await
}

#[tokio::test]
async fn an_ssh_session_announces_xterm_256color_unless_told_otherwise() {
    assert_eq!(
        ssh_term(None).await,
        [b"term\r\nxterm-256color\r\n".as_slice(), PROMPT].concat()
    );
}

#[tokio::test]
async fn an_ssh_session_announces_the_terminal_type_of_its_target() {
    assert_eq!(
        ssh_term(Some(TerminalType::Vt100)).await,
        [b"term\r\nvt100\r\n".as_slice(), PROMPT].concat()
    );
}

async fn telnet_login(
    manager: &SessionManager,
    server: &TestTelnetServer,
    kind: TerminalType,
) -> (SessionId, SessionEvents) {
    let mut target = server.target();
    target.terminal_type = kind;
    let (id, mut events) = manager.open(SessionTarget::Telnet(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    manager
        .write(id, format!("{USERNAME}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, PASSWORD_PROMPT).await;
    manager
        .write(id, format!("{PASSWORD}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, TELNET_PROMPT).await;
    (id, events)
}

#[tokio::test]
async fn a_telnet_session_tells_the_device_the_terminal_type_of_its_target() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = telnet_login(&manager, &server, TerminalType::Ansi).await;

    manager.write(id, b"term\r".to_vec()).unwrap();
    let term = output_ending_with(&mut events, TELNET_PROMPT).await;

    assert_eq!(
        term,
        [b"term\r\nansi\r\n".as_slice(), TELNET_PROMPT].concat()
    );
}
