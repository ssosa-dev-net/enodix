//! Multi-send (#35): one call writes the same input to several sessions,
//! as typed in a tab of the multi-send group. Observed as what each
//! in-process device echoes or receives: every connected member gets the
//! bytes in order, a closed or still connecting member is skipped and
//! reported, and a session outside the list gets nothing. Only typed
//! input fans out (#123): a paste, the terminal's replies and mouse reports
//! reach their own device, and a group paste's line reaches its member only
//! while it is one.

mod support;

use std::sync::Arc;
use std::time::Duration;

use netterm_core::layout::{InputOrigin, TabId, WindowId, Windows};
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

// What reaches the group's devices, by where the input came from (#123).

/// Tabs of the main window showing `sessions`, in order.
fn tabs_showing(windows: &mut Windows, sessions: &[SessionId]) -> Vec<TabId> {
    let mut layout = windows.layout_mut(WindowId::MAIN).unwrap();
    let tabs: Vec<TabId> = sessions.iter().map(|_| layout.open_tab()).collect();
    drop(layout);
    for (&tab, &session) in tabs.iter().zip(sessions) {
        windows
            .attach_session(WindowId::MAIN, tab, session)
            .unwrap();
    }
    tabs
}

/// Sends `data` from `session` as the shell does: to the sessions the
/// group routes input of that origin to.
fn send(
    manager: &SessionManager,
    windows: &Windows,
    session: SessionId,
    origin: InputOrigin,
    data: &[u8],
) -> Fanout {
    manager.write_to_each(&windows.input_targets(session, origin), data)
}

#[tokio::test]
async fn a_paste_begun_before_its_tab_joined_the_group_reaches_only_its_own_device() {
    let servers = [TestSshServer::start().await, TestSshServer::start().await];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;
    let mut windows = Windows::new();
    let [a_tab, b_tab] = tabs_showing(&mut windows, &[a, b])[..] else {
        unreachable!()
    };
    windows.join_group(WindowId::MAIN, b_tab).unwrap();

    // The paste's first line goes; the tab joins; its next line goes.
    send(&manager, &windows, a, InputOrigin::Pasted, b"show one\r");
    assert_eq!(
        output_ending_with(&mut a_events, PROMPT).await,
        transcript("show one", b"")
    );
    windows.join_group(WindowId::MAIN, a_tab).unwrap();
    send(&manager, &windows, a, InputOrigin::Pasted, b"show two\r");

    assert_eq!(
        output_ending_with(&mut a_events, PROMPT).await,
        transcript("show two", b"")
    );
    no_output(&mut b_events).await;
}

#[tokio::test]
async fn replies_and_mouse_reports_from_a_member_reach_only_its_own_device() {
    let servers = [TestSshServer::start().await, TestSshServer::start().await];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;
    let mut windows = Windows::new();
    for tab in tabs_showing(&mut windows, &[a, b]) {
        windows.join_group(WindowId::MAIN, tab).unwrap();
    }

    // A cursor position report, then an SGR mouse click, then Enter typed:
    // the scripted shell echoes what it received.
    send(&manager, &windows, a, InputOrigin::Reply, b"R1");
    send(&manager, &windows, a, InputOrigin::Mouse, b"M2");
    send(&manager, &windows, a, InputOrigin::Typed, b"\r");

    assert_eq!(
        output_ending_with(&mut a_events, PROMPT).await,
        transcript("R1M2", b"")
    );
    // The other member got only the typed Enter.
    assert_eq!(
        output_ending_with(&mut b_events, PROMPT).await,
        transcript("", b"")
    );
}

#[tokio::test]
async fn a_member_closing_in_the_middle_of_a_group_paste_gets_no_more_and_the_others_get_every_line(
) {
    let servers = [
        TestSshServer::start().await,
        TestSshServer::start().await,
        TestSshServer::start().await,
    ];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;
    let (c, mut c_events) = ssh_shell(&manager, &servers[2]).await;
    let mut windows = Windows::new();
    let [a_tab, b_tab, c_tab] = tabs_showing(&mut windows, &[a, b, c])[..] else {
        unreachable!()
    };
    for tab in [a_tab, b_tab, c_tab] {
        windows.join_group(WindowId::MAIN, tab).unwrap();
    }
    let planned = windows.group_paste_members(WindowId::MAIN, a_tab).unwrap();
    let sessions: Vec<SessionId> = planned.members.iter().map(|(_, s)| *s).collect();
    windows
        .group_paste_targets(WindowId::MAIN, a_tab, planned.revision, &sessions)
        .unwrap();

    // Each member's first line, through its own terminal.
    for &member in &sessions {
        send(
            &manager,
            &windows,
            member,
            InputOrigin::GroupPaste,
            b"line one\r",
        );
    }
    for events in [&mut a_events, &mut b_events, &mut c_events] {
        assert_eq!(
            output_ending_with(events, PROMPT).await,
            transcript("line one", b"")
        );
    }
    // b's tab closes (and with it its session) before the next line.
    windows
        .layout_mut(WindowId::MAIN)
        .unwrap()
        .close_tab(b_tab)
        .unwrap();
    manager.close(b).unwrap();
    assert!(matches!(
        next_state(&mut b_events).await,
        SessionState::Closed(_)
    ));
    let reports: Vec<Fanout> = sessions
        .iter()
        .map(|&member| {
            send(
                &manager,
                &windows,
                member,
                InputOrigin::GroupPaste,
                b"line two\r",
            )
        })
        .collect();

    assert_eq!(reports[1].written, Vec::<SessionId>::new());
    for events in [&mut a_events, &mut c_events] {
        assert_eq!(
            output_ending_with(events, PROMPT).await,
            transcript("line two", b"")
        );
    }
}

#[tokio::test]
async fn a_member_leaving_in_the_middle_of_a_group_paste_gets_no_more_of_it() {
    let servers = [TestSshServer::start().await, TestSshServer::start().await];
    let manager = support::manager();
    let (a, mut a_events) = ssh_shell(&manager, &servers[0]).await;
    let (b, mut b_events) = ssh_shell(&manager, &servers[1]).await;
    let mut windows = Windows::new();
    let [a_tab, b_tab] = tabs_showing(&mut windows, &[a, b])[..] else {
        unreachable!()
    };
    windows.join_group(WindowId::MAIN, a_tab).unwrap();
    windows.join_group(WindowId::MAIN, b_tab).unwrap();
    for member in [a, b] {
        send(
            &manager,
            &windows,
            member,
            InputOrigin::GroupPaste,
            b"line one\r",
        );
    }
    for events in [&mut a_events, &mut b_events] {
        output_ending_with(events, PROMPT).await;
    }

    windows.leave_group(WindowId::MAIN, b_tab).unwrap();
    for member in [a, b] {
        send(
            &manager,
            &windows,
            member,
            InputOrigin::GroupPaste,
            b"line two\r",
        );
    }

    assert_eq!(
        output_ending_with(&mut a_events, PROMPT).await,
        transcript("line two", b"")
    );
    no_output(&mut b_events).await;
}

#[tokio::test]
async fn a_serial_break_in_a_member_goes_to_its_own_line_only() {
    let ports = VirtualSerialPorts::new();
    let first = ports.add("COM3", "USB Serial Port");
    let second = ports.add("COM4", "USB Serial Port");
    let manager = SessionManager::with_serial_ports(
        known_hosts(),
        OutputOptions::default(),
        Arc::new(ports.clone()),
    );
    let serial = |port: &str| {
        SessionTarget::Serial(SerialTarget {
            port: port.into(),
            parameters: SerialParameters::default(),
        })
    };
    let (a, _a_events) = connected(&manager, serial("COM3")).await;
    let (b, _b_events) = connected(&manager, serial("COM4")).await;
    let mut windows = Windows::new();
    for tab in tabs_showing(&mut windows, &[a, b]) {
        windows.join_group(WindowId::MAIN, tab).unwrap();
    }

    manager.send_break(a).unwrap();
    // Typed after it, so that the first line has seen the break once this
    // arrives.
    send(&manager, &windows, a, InputOrigin::Typed, b"x");

    assert_eq!(first.received_ending_with(b"x").await, b"x");
    assert_eq!(second.received_ending_with(b"x").await, b"x");
    assert_eq!(first.breaks().len(), 1);
    assert_eq!(second.breaks().len(), 0);
}
