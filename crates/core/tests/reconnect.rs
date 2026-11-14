//! Reconnecting a session that ended, in place, and duplicating a session.

mod support;

use std::time::{Duration, Instant};

use netterm_core::session::{
    CloseReason, OpenError, SessionError, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, SshTarget, TerminalSize,
};
use support::{closed_port, next_state, output_ending_with, target_at, TestSshServer, PROMPT};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Waits for `Connecting`, `Connected` and the first prompt.
async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

async fn open_shell(
    manager: &SessionManager,
    server: &TestSshServer,
) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    connected(&mut events).await;
    (id, events)
}

/// Runs `size` in the session and returns what it printed.
async fn size_of(manager: &SessionManager, id: SessionId, events: &mut SessionEvents) -> String {
    manager.write(id, b"size\r".to_vec()).unwrap();
    let output = output_ending_with(events, PROMPT).await;
    let output = String::from_utf8(output).unwrap();
    output
        .trim_start_matches("size\r\n")
        .trim_end_matches(std::str::from_utf8(PROMPT).unwrap())
        .trim()
        .to_owned()
}

#[tokio::test]
async fn a_session_whose_connection_the_server_dropped_reconnects_in_place() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;
    manager.write(id, b"drop\r".to_vec()).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ConnectionLost)
    );
    assert_eq!(events.next().await, None);

    let mut events = manager.reconnect(id, SIZE).unwrap();

    connected(&mut events).await;
    assert_eq!(size_of(&manager, id, &mut events).await, "80x24");
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn a_connection_that_goes_silent_is_lost_and_reconnects() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let target = SshTarget {
        connect_timeout: Duration::from_secs(1),
        ..server.target()
    };
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    connected(&mut events).await;

    manager.write(id, b"freeze\r".to_vec()).unwrap();

    // Unanswered keepalives: lost after about three connect timeouts.
    let started = Instant::now();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ConnectionLost)
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    let mut events = manager.reconnect(id, SIZE).unwrap();
    connected(&mut events).await;
}

#[tokio::test]
async fn a_session_that_failed_reconnects_to_the_same_target() {
    let port = closed_port().await;
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Ssh(target_at("127.0.0.1", port)), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert!(matches!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::ConnectionRefused { .. }))
    ));

    // The device comes up on that port.
    let server = TestSshServer::start_on(port).await;
    let mut events = manager.reconnect(id, SIZE).unwrap();

    connected(&mut events).await;
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn a_reconnected_session_gets_the_size_the_terminal_has_now() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;
    manager.write(id, b"exit\r".to_vec()).unwrap();
    assert!(matches!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::RemoteExited { .. })
    ));

    let size = TerminalSize {
        cols: 120,
        rows: 40,
    };
    let mut events = manager.reconnect(id, size).unwrap();

    connected(&mut events).await;
    assert_eq!(size_of(&manager, id, &mut events).await, "120x40");
}

#[tokio::test]
async fn an_open_session_is_not_reconnected() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    assert_eq!(
        manager.reconnect(id, SIZE).err(),
        Some(SessionError::StillOpen)
    );
    // It goes on as before.
    assert_eq!(size_of(&manager, id, &mut events).await, "80x24");
}

#[tokio::test]
async fn a_discarded_session_is_gone() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;
    manager.close(id).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );

    manager.discard(id);

    assert_eq!(
        manager.reconnect(id, SIZE).err(),
        Some(SessionError::NotFound)
    );
    assert_eq!(
        manager.duplicate(id, SIZE).err(),
        Some(SessionError::NotFound)
    );
}

#[tokio::test]
async fn discarding_an_open_session_closes_it() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.discard(id);

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
    assert_eq!(
        manager.reconnect(id, SIZE).err(),
        Some(SessionError::NotFound)
    );
}

#[tokio::test]
async fn duplicating_a_session_opens_a_second_connection_to_the_same_host() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    let (copy, mut copy_events) = manager.duplicate(id, SIZE).unwrap();

    assert_ne!(copy, id);
    connected(&mut copy_events).await;
    assert_eq!(server.connections(), 2);
    // Two sessions, each on its own.
    manager.close(id).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
    assert_eq!(size_of(&manager, copy, &mut copy_events).await, "80x24");
}

#[tokio::test]
async fn a_session_that_ended_can_be_duplicated() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;
    manager.write(id, b"drop\r".to_vec()).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ConnectionLost)
    );

    let (copy, mut copy_events) = manager.duplicate(id, SIZE).unwrap();

    connected(&mut copy_events).await;
    assert_eq!(size_of(&manager, copy, &mut copy_events).await, "80x24");
}
