//! Telnet sessions against the in-process telnet server
//! (`support::telnet`): login prompts, echo and password masking, window
//! size and terminal type, `IAC` escaping, flow control, close and
//! reconnect, all observed as the bytes and states a user would see.

mod support;

use std::fs;
use std::time::Duration;

use netterm_core::credentials::{HostLogin, MemoryStore, Profiles};
use netterm_core::session::{
    CloseReason, OpenError, OutputOptions, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, TelnetTarget, TerminalSize, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TELNET_PORT,
};
use netterm_core::workspace::{HostId, Protocol, Workspace};
use support::telnet::{
    EchoMode, TestTelnetServer, BANNER, LOGIN_INCORRECT, PASSWORD_PROMPT, TELNET_PROMPT,
    UNKNOWN_COMMAND, USERNAME_PROMPT,
};
use support::{
    closed_port, next_state, output_ending_with, unacknowledged_output, CTRL_C, FLOOD_LINE,
    INTERRUPTED, PASSWORD, USERNAME,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

async fn open(manager: &SessionManager, target: TelnetTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Telnet(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

/// Types `text` and returns the output up to and including `until`.
async fn type_until(
    manager: &SessionManager,
    id: SessionId,
    events: &mut SessionEvents,
    text: &[u8],
    until: &[u8],
) -> Vec<u8> {
    manager.write(id, text.to_vec()).unwrap();
    output_ending_with(events, until).await
}

/// Opens a session and logs in, up to the shell prompt.
async fn logged_in(
    manager: &SessionManager,
    server: &TestTelnetServer,
) -> (SessionId, SessionEvents) {
    let (id, mut events) = open(manager, server.target()).await;
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    let username = format!("{USERNAME}\r");
    type_until(
        manager,
        id,
        &mut events,
        username.as_bytes(),
        PASSWORD_PROMPT,
    )
    .await;
    let password = format!("{PASSWORD}\r");
    type_until(manager, id, &mut events, password.as_bytes(), TELNET_PROMPT).await;
    (id, events)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn a_telnet_session_shows_the_login_prompt_and_logs_in() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;

    // The device's own bytes, with the option negotiation taken out.
    let greeting = output_ending_with(&mut events, USERNAME_PROMPT).await;
    assert_eq!(greeting, [BANNER, USERNAME_PROMPT].concat());

    let username = format!("{USERNAME}\r");
    let echoed = type_until(
        &manager,
        id,
        &mut events,
        username.as_bytes(),
        PASSWORD_PROMPT,
    )
    .await;
    assert_eq!(
        echoed,
        [USERNAME.as_bytes(), b"\r\n", PASSWORD_PROMPT].concat()
    );

    let password = format!("{PASSWORD}\r");
    let after = type_until(
        &manager,
        id,
        &mut events,
        password.as_bytes(),
        TELNET_PROMPT,
    )
    .await;
    assert_eq!(after, [b"\r\n".as_slice(), TELNET_PROMPT].concat());
}

#[tokio::test]
async fn a_wrong_password_is_refused_by_the_device_and_asked_again() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    let username = format!("{USERNAME}\r");
    type_until(
        &manager,
        id,
        &mut events,
        username.as_bytes(),
        PASSWORD_PROMPT,
    )
    .await;

    let refused = type_until(&manager, id, &mut events, b"wrong\r", USERNAME_PROMPT).await;

    assert_eq!(refused, [LOGIN_INCORRECT, USERNAME_PROMPT].concat());
}

/// Most devices echo themselves: the client must not echo as well.
#[tokio::test]
async fn with_a_device_that_echoes_typed_input_shows_once() {
    let server = TestTelnetServer::start_with(EchoMode::Remote).await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    let output = type_until(&manager, id, &mut events, b"hello\r", TELNET_PROMPT).await;

    assert_eq!(
        output,
        [b"hello\r\n", UNKNOWN_COMMAND, b"hello\r\n", TELNET_PROMPT].concat()
    );
}

/// Old devices leave echoing to the client, and take it over (`WILL
/// ECHO`) only while asking for the password, so that it is not shown.
#[tokio::test]
async fn with_a_line_mode_device_the_client_echoes_except_while_the_device_masks_the_password() {
    let server = TestTelnetServer::start_with(EchoMode::Line).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    output_ending_with(&mut events, USERNAME_PROMPT).await;

    // Echoed by the client, once.
    let username = format!("{USERNAME}\r");
    let echoed = type_until(
        &manager,
        id,
        &mut events,
        username.as_bytes(),
        PASSWORD_PROMPT,
    )
    .await;
    assert_eq!(
        echoed,
        [USERNAME.as_bytes(), b"\r\n", PASSWORD_PROMPT].concat()
    );

    let password = format!("{PASSWORD}\r");
    let after = type_until(
        &manager,
        id,
        &mut events,
        password.as_bytes(),
        TELNET_PROMPT,
    )
    .await;
    assert!(!contains(&after, PASSWORD.as_bytes()), "{after:?}");
    assert_eq!(after, [b"\r\n".as_slice(), TELNET_PROMPT].concat());

    // The device gave the echo back after the password.
    let output = type_until(&manager, id, &mut events, b"hello\r", TELNET_PROMPT).await;
    assert_eq!(
        output,
        [b"hello\r\n", UNKNOWN_COMMAND, b"hello\r\n", TELNET_PROMPT].concat()
    );
}

#[tokio::test]
async fn the_client_follows_the_device_switching_its_echo_off_and_on() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    type_until(&manager, id, &mut events, b"echo off\r", TELNET_PROMPT).await;
    let local = type_until(&manager, id, &mut events, b"one\r", TELNET_PROMPT).await;
    assert_eq!(
        local,
        [b"one\r\n", UNKNOWN_COMMAND, b"one\r\n", TELNET_PROMPT].concat()
    );

    // Echoed locally while the device does not echo.
    type_until(&manager, id, &mut events, b"echo on\r", TELNET_PROMPT).await;
    let remote = type_until(&manager, id, &mut events, b"two\r", TELNET_PROMPT).await;
    assert_eq!(
        remote,
        [b"two\r\n", UNKNOWN_COMMAND, b"two\r\n", TELNET_PROMPT].concat()
    );
}

#[tokio::test]
async fn the_device_learns_the_window_size_and_every_resize() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    let size = type_until(&manager, id, &mut events, b"size\r", TELNET_PROMPT).await;
    assert_eq!(
        size,
        [b"size\r\n80x24\r\n".as_slice(), TELNET_PROMPT].concat()
    );

    manager
        .resize(
            id,
            TerminalSize {
                cols: 132,
                rows: 50,
            },
        )
        .unwrap();
    let resized = type_until(&manager, id, &mut events, b"size\r", TELNET_PROMPT).await;
    assert_eq!(
        resized,
        [b"size\r\n132x50\r\n".as_slice(), TELNET_PROMPT].concat()
    );
}

/// Window sizes are sent with any `0xFF` byte doubled, as telnet requires.
#[tokio::test]
async fn a_window_size_holding_the_byte_255_reaches_the_device_intact() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    manager
        .resize(
            id,
            TerminalSize {
                cols: 255,
                rows: 511,
            },
        )
        .unwrap();
    let resized = type_until(&manager, id, &mut events, b"size\r", TELNET_PROMPT).await;

    assert_eq!(
        resized,
        [b"size\r\n255x511\r\n".as_slice(), TELNET_PROMPT].concat()
    );
}

#[tokio::test]
async fn a_resize_while_connecting_reaches_the_device() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Telnet(server.target()), SIZE);
    manager
        .resize(
            id,
            TerminalSize {
                cols: 100,
                rows: 30,
            },
        )
        .unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    let username = format!("{USERNAME}\r");
    type_until(
        &manager,
        id,
        &mut events,
        username.as_bytes(),
        PASSWORD_PROMPT,
    )
    .await;
    let password = format!("{PASSWORD}\r");
    type_until(
        &manager,
        id,
        &mut events,
        password.as_bytes(),
        TELNET_PROMPT,
    )
    .await;

    let size = type_until(&manager, id, &mut events, b"size\r", TELNET_PROMPT).await;

    assert_eq!(
        size,
        [b"size\r\n100x30\r\n".as_slice(), TELNET_PROMPT].concat()
    );
}

#[tokio::test]
async fn the_device_is_told_the_terminal_type() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    let term = type_until(&manager, id, &mut events, b"term\r", TELNET_PROMPT).await;

    assert_eq!(
        term,
        [b"term\r\nxterm-256color\r\n".as_slice(), TELNET_PROMPT].concat()
    );
}

/// Some devices never ask for the window size or terminal type: the
/// session offers them, as telnet clients do.
#[tokio::test]
async fn a_device_that_waits_to_be_offered_learns_the_size_and_terminal_type() {
    let server = TestTelnetServer::start_waiting_for_offers().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    let size = type_until(&manager, id, &mut events, b"size\r", TELNET_PROMPT).await;
    assert_eq!(
        size,
        [b"size\r\n80x24\r\n".as_slice(), TELNET_PROMPT].concat()
    );
    manager
        .resize(id, TerminalSize { cols: 90, rows: 33 })
        .unwrap();
    let resized = type_until(&manager, id, &mut events, b"size\r", TELNET_PROMPT).await;
    assert_eq!(
        resized,
        [b"size\r\n90x33\r\n".as_slice(), TELNET_PROMPT].concat()
    );
    let term = type_until(&manager, id, &mut events, b"term\r", TELNET_PROMPT).await;
    assert_eq!(
        term,
        [b"term\r\nxterm-256color\r\n".as_slice(), TELNET_PROMPT].concat()
    );
}

/// While the session echoes, an arrow key reaches the device but is not
/// echoed: on screen it would move the cursor.
#[tokio::test]
async fn local_echo_leaves_out_escape_sequences_and_control_characters() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;
    type_until(&manager, id, &mut events, b"echo off\r", TELNET_PROMPT).await;

    let output = type_until(
        &manager,
        id,
        &mut events,
        b"ab\x1b[D\x1bOA\x01c\r",
        TELNET_PROMPT,
    )
    .await;

    assert_eq!(
        output,
        [
            b"abc\r\n".as_slice(),
            UNKNOWN_COMMAND,
            b"ab\x1b[D\x1bOA\x01c\r\n",
            TELNET_PROMPT
        ]
        .concat()
    );
}

/// The byte 255 is telnet's command marker, so it travels doubled; the user
/// sees and sends it as one byte.
#[tokio::test]
async fn the_byte_255_passes_both_ways_as_data() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    let from_device = type_until(&manager, id, &mut events, b"binary\r", TELNET_PROMPT).await;
    assert_eq!(
        from_device,
        [b"binary\r\nA\xffB\r\n".as_slice(), TELNET_PROMPT].concat()
    );

    let to_device = type_until(&manager, id, &mut events, b"\xff\r", TELNET_PROMPT).await;
    assert_eq!(
        to_device,
        [b"\xff\r\n", UNKNOWN_COMMAND, b"\xff\r\n", TELNET_PROMPT].concat()
    );
}

/// Enter arrives as CR LF in one write (a paste of Windows text, say): the
/// device still sees one end of line.
#[tokio::test]
async fn a_carriage_return_and_line_feed_typed_together_end_one_line() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    let second = [UNKNOWN_COMMAND, b"two\r\n", TELNET_PROMPT].concat();
    let output = type_until(&manager, id, &mut events, b"one\r\ntwo\r", &second).await;

    // No empty line between them, as a second end of line would give.
    assert_eq!(
        output,
        [
            b"one\r\n",
            UNKNOWN_COMMAND,
            b"one\r\n",
            TELNET_PROMPT,
            b"two\r\n",
            &second
        ]
        .concat()
    );
}

#[tokio::test]
async fn reading_stops_while_output_is_unacknowledged_and_input_still_reaches_the_device() {
    let server = TestTelnetServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, mut events) = logged_in(&manager, &server).await;

    manager.write(id, b"flood\r".to_vec()).unwrap();
    let stalled = unacknowledged_output(&mut events, Duration::from_millis(500)).await;
    // One read past the window at most.
    assert!(
        stalled.len() >= window && stalled.len() <= window + 16 * 1024,
        "read {} bytes with a window of {window}",
        stalled.len()
    );
    assert!(contains(&stalled, FLOOD_LINE));

    // Ctrl+C goes out while reading is paused; acknowledging lets the rest
    // of the flood and the prompt in.
    manager.write(id, vec![CTRL_C]).unwrap();
    events.acknowledge(stalled.len());
    let rest = output_ending_with(
        &mut events,
        [INTERRUPTED, TELNET_PROMPT].concat().as_slice(),
    )
    .await;
    assert!(rest.ends_with(&[INTERRUPTED, TELNET_PROMPT].concat()));
}

#[tokio::test]
async fn the_device_closing_the_connection_ends_the_session() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    manager.write(id, b"exit\r".to_vec()).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::RemoteExited { exit_status: None })
    );
}

#[tokio::test]
async fn closing_a_telnet_session_ends_it() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;

    manager.close(id).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
}

#[tokio::test]
async fn a_closed_telnet_session_reconnects_to_a_new_login_prompt() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server).await;
    manager.write(id, b"exit\r".to_vec()).unwrap();
    next_state(&mut events).await;

    let mut events = manager.reconnect(id, SIZE).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, USERNAME_PROMPT).await;
}

#[tokio::test]
async fn a_port_nothing_listens_on_is_reported() {
    let port = closed_port().await;
    let manager = support::manager();
    let target = TelnetTarget {
        host: "127.0.0.1".into(),
        port,
        connect_timeout: Duration::from_secs(5),
    };
    let (_id, mut events) = manager.open(SessionTarget::Telnet(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::ConnectionRefused {
            host: "127.0.0.1".into(),
            port
        }))
    );
}

#[tokio::test]
async fn telnet_sessions_are_unencrypted_and_ssh_sessions_are_not() {
    let telnet = TestTelnetServer::start().await;
    let ssh = support::TestSshServer::start().await;
    let manager = support::manager();

    let (telnet_id, _telnet_events) = manager.open(SessionTarget::Telnet(telnet.target()), SIZE);
    let (ssh_id, _ssh_events) = manager.open(SessionTarget::Ssh(ssh.target()), SIZE);

    assert_eq!(manager.encrypted(telnet_id), Ok(false));
    assert_eq!(manager.encrypted(ssh_id), Ok(true));
    assert!(!SessionTarget::Telnet(telnet.target()).encrypted());
}

/// A workspace made of `files` (relative path, text).
fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("telnet");
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[test]
fn a_host_whose_resolved_protocol_is_telnet_logs_in_in_the_terminal() {
    let (_temp, workspace) = workspace(&[
        (
            "Old/_folder.yaml",
            "protocol: telnet\ncredential_profile: noc\n".into(),
        ),
        ("Old/switch.yaml", "address: 10.0.0.9\n".into()),
        ("router.yaml", "address: 10.0.0.1\n".into()),
    ]);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let switch = HostId::new("Old/switch.yaml");

    assert_eq!(profiles.login(&switch).unwrap(), HostLogin::Telnet);
    assert_eq!(
        profiles.telnet_target(&switch).unwrap(),
        TelnetTarget {
            host: "10.0.0.9".into(),
            port: DEFAULT_TELNET_PORT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    );
    // Reached over SSH instead (from the side panel), it logs in as SSH does.
    assert_eq!(
        profiles.login_with(&switch, Protocol::Ssh).unwrap(),
        HostLogin::NotSetUp {
            profile: "noc".into()
        }
    );
    assert_eq!(
        profiles.login(&HostId::new("router.yaml")).unwrap(),
        HostLogin::Prompt
    );
}

#[test]
fn any_host_can_be_reached_by_telnet_on_its_port_with_its_connect_timeout() {
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "connect_timeout: 30\n".into()),
        ("console.yaml", "address: 10.0.0.2\nport: 2001\n".into()),
        ("router.yaml", "address: 10.0.0.1\n".into()),
    ]);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);

    let console = profiles
        .telnet_target(&HostId::new("console.yaml"))
        .unwrap();
    assert_eq!(
        (console.port, console.connect_timeout),
        (2001, Duration::from_secs(30))
    );
    // An SSH host, reached by telnet from the side panel: telnet's port.
    let router = profiles.telnet_target(&HostId::new("router.yaml")).unwrap();
    assert_eq!(router.port, DEFAULT_TELNET_PORT);
    assert_eq!(
        profiles
            .login_with(&HostId::new("router.yaml"), Protocol::Telnet)
            .unwrap(),
        HostLogin::Telnet
    );
}

/// Telnet is not tunnelled through jump hosts (yet): rather than reaching
/// the device directly, around the bastion, it is refused.
#[test]
fn a_telnet_host_behind_a_jump_host_is_refused() {
    let (_temp, workspace) = workspace(&[
        ("bastion.yaml", "address: 10.0.0.1\n".into()),
        (
            "Site/_folder.yaml",
            "jump_host: bastion.yaml\nprotocol: telnet\n".into(),
        ),
        ("Site/switch.yaml", "address: 10.1.0.9\n".into()),
    ]);
    let store = MemoryStore::new();
    let error = Profiles::new(&store, &workspace)
        .telnet_target(&HostId::new("Site/switch.yaml"))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Site/switch.yaml is reached through the jump host bastion.yaml, \
         and telnet sessions cannot go through jump hosts"
    );
}

#[tokio::test]
async fn an_inventory_host_set_to_telnet_opens_a_telnet_session() {
    let server = TestTelnetServer::start().await;
    let (_temp, workspace) = workspace(&[(
        "Lab/old-switch.yaml",
        format!(
            "address: 127.0.0.1\nport: {}\nprotocol: telnet\n",
            server.port()
        ),
    )]);
    let store = MemoryStore::new();
    let target = Profiles::new(&store, &workspace)
        .telnet_target(&HostId::new("Lab/old-switch.yaml"))
        .unwrap();

    let manager = support::manager();
    let (_id, mut events) = open(&manager, target).await;

    output_ending_with(&mut events, USERNAME_PROMPT).await;
}
