//! Multi-send (#35): one call writes the same input to several sessions,
//! as typed in a tab of the multi-send group. Observed as what each
//! in-process device echoes or receives: every connected member gets the
//! bytes in order, a closed or still connecting member is skipped and
//! reported, and a session outside the list gets nothing.

mod support;

use std::sync::Arc;
use std::time::Duration;

use netterm_core::session::{
    Fanout, OutputOptions, SerialParameters, SerialTarget, SessionEvents, SessionId,
    SessionManager, SessionState, SessionTarget, TerminalSize,
};
use support::serial::VirtualSerialPorts;
use support::telnet::{TestTelnetServer, PASSWORD_PROMPT, TELNET_PROMPT, USERNAME_PROMPT};
use support::{
    known_hosts, next_state, output_ending_with, target_at, unacknowledged_output, SilentServer,
    TestSshServer, CTRL_C, INTERRUPTED, PASSWORD, PROMPT, USERNAME,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Opens a session and waits until it is connected.
async fn connected(manager: &SessionManager, target: SessionTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(target, SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

/// An SSH session to `server`, up to its first prompt.
async fn ssh_shell(manager: &SessionManager, server: &TestSshServer) -> (SessionId, SessionEvents) {
    let (id, mut events) = connected(manager, SessionTarget::Ssh(server.target())).await;
    output_ending_with(&mut events, PROMPT).await;
    (id, events)
}

/// A telnet session to `server`, logged in up to its prompt.
async fn telnet_shell(
    manager: &SessionManager,
    server: &TestTelnetServer,
) -> (SessionId, SessionEvents) {
    let (id, mut events) = connected(manager, SessionTarget::Telnet(server.target())).await;
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

/// Types `text` into `members` one key at a time, as multi-send does, and
/// returns what each call reported.
fn type_keys(manager: &SessionManager, members: &[SessionId], text: &[u8]) -> Vec<Fanout> {
    text.iter()
        .map(|&key| manager.write_to_each(members, &[key]))
        .collect()
}

/// What the scripted SSH shell sends back for a typed line.
fn transcript(typed: &str, output: &[u8]) -> Vec<u8> {
    [typed.as_bytes(), b"\r\n", output, PROMPT].concat()
}

/// Waits long enough to see that `events` brings no output.
async fn no_output(events: &mut SessionEvents) {
    let received = unacknowledged_output(events, Duration::from_millis(300)).await;
    assert_eq!(String::from_utf8_lossy(&received), "");
}

#[tokio::test]
async fn keys_typed_in_the_group_reach_every_member_in_order() {
    let servers = [
        TestSshServer::start().await,
        TestSshServer::start().await,
        TestSshServer::start().await,
    ];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;
    let (c, mut c_events) = ssh_shell(&manager, &servers[2]).await;
    let members = [a, b, c];

    let reports = type_keys(&manager, &members, b"size\r");

    for report in reports {
        assert_eq!(report.written, members);
        assert_eq!(report.skipped, Vec::<SessionId>::new());
    }
    let expected = transcript("size", b"80x24\r\n");
    for events in [&mut a_events, &mut b_events, &mut c_events] {
        assert_eq!(output_ending_with(events, PROMPT).await, expected);
    }
}

#[tokio::test]
async fn control_keys_reach_every_member() {
    let servers = [TestSshServer::start().await, TestSshServer::start().await];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;

    type_keys(&manager, &[a, b], &[b'x', b'y', CTRL_C]);

    let expected = [b"xy".as_slice(), INTERRUPTED, PROMPT].concat();
    assert_eq!(output_ending_with(&mut a_events, PROMPT).await, expected);
    assert_eq!(output_ending_with(&mut b_events, PROMPT).await, expected);
}

#[tokio::test]
async fn a_session_outside_the_members_receives_nothing() {
    let servers = [TestSshServer::start().await, TestSshServer::start().await];
    let manager = support::manager();
    let (member, mut member_events) = ssh_shell(&manager, &servers[0]).await;
    let (_outside, mut outside_events) = ssh_shell(&manager, &servers[1]).await;

    type_keys(&manager, &[member], b"size\r");

    output_ending_with(&mut member_events, PROMPT).await;
    no_output(&mut outside_events).await;
}

#[tokio::test]
async fn a_closed_member_is_skipped_and_reported_and_receives_again_after_reconnecting() {
    let servers = [TestSshServer::start().await, TestSshServer::start().await];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;
    manager.close(b).unwrap();
    assert!(matches!(
        next_state(&mut b_events).await,
        SessionState::Closed(_)
    ));

    let report = manager.write_to_each(&[a, b], b"size\r");

    assert_eq!(report.written, vec![a]);
    assert_eq!(report.skipped, vec![b]);
    assert_eq!(
        output_ending_with(&mut a_events, PROMPT).await,
        transcript("size", b"80x24\r\n")
    );

    let mut b_events = manager.reconnect(b, SIZE).unwrap();
    assert_eq!(next_state(&mut b_events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut b_events).await, SessionState::Connected);
    output_ending_with(&mut b_events, PROMPT).await;

    let report = manager.write_to_each(&[a, b], b"size\r");

    assert_eq!(report.written, vec![a, b]);
    for events in [&mut a_events, &mut b_events] {
        assert_eq!(
            output_ending_with(events, PROMPT).await,
            transcript("size", b"80x24\r\n")
        );
    }
}

#[tokio::test]
async fn a_member_still_connecting_or_unknown_is_skipped_and_the_others_still_receive() {
    let server = TestSshServer::start().await;
    let silent = SilentServer::start().await;
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &server).await;
    let (connecting, mut connecting_events) = manager.open(
        SessionTarget::Ssh(target_at("127.0.0.1", silent.port())),
        SIZE,
    );
    assert_eq!(
        next_state(&mut connecting_events).await,
        SessionState::Connecting
    );
    let unknown = SessionId(9_999);

    let report = manager.write_to_each(&[connecting, unknown, a], b"size\r");

    assert_eq!(report.written, vec![a]);
    assert_eq!(report.skipped, vec![connecting, unknown]);
    assert_eq!(
        output_ending_with(&mut a_events, PROMPT).await,
        transcript("size", b"80x24\r\n")
    );
}

#[tokio::test]
async fn ssh_telnet_and_serial_sessions_can_all_be_members() {
    let ssh = TestSshServer::start().await;
    let telnet = TestTelnetServer::start().await;
    let ports = VirtualSerialPorts::new();
    let console = ports.add("COM3", "USB Serial Port");
    let manager = SessionManager::with_serial_ports(
        known_hosts(),
        OutputOptions::default(),
        Arc::new(ports.clone()),
    );
    let (ssh_id, mut ssh_events) = ssh_shell(&manager, &ssh).await;
    let (telnet_id, mut telnet_events) = telnet_shell(&manager, &telnet).await;
    let (serial_id, _serial_events) = connected(
        &manager,
        SessionTarget::Serial(SerialTarget {
            port: "COM3".into(),
            parameters: SerialParameters::default(),
        }),
    )
    .await;
    let members = [ssh_id, telnet_id, serial_id];

    for report in type_keys(&manager, &members, b"size\r") {
        assert_eq!(report.written, members);
    }

    assert_eq!(
        output_ending_with(&mut ssh_events, PROMPT).await,
        transcript("size", b"80x24\r\n")
    );
    let telnet_output = output_ending_with(&mut telnet_events, TELNET_PROMPT).await;
    assert!(
        String::from_utf8_lossy(&telnet_output).starts_with("size\r\n80x24\r\n"),
        "telnet device showed {:?}",
        String::from_utf8_lossy(&telnet_output)
    );
    assert_eq!(console.received_ending_with(b"size\r").await, b"size\r");
}
