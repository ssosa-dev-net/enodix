//! Session tests: real SSH sessions opened through the session manager
//! against the in-process test server, asserting on the bytes received.

mod support;

use netterm_core::session::{
    CloseReason, OpenError, SessionError, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, SshAuth, SshTarget, TerminalSize,
};
use support::{
    next_state, numbered_lines, output_ending_with, TestSshServer, PROMPT, RSA_CLIENT_KEY,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Opens a session and consumes everything up to and including the first prompt.
async fn open_shell(
    manager: &SessionManager,
    server: &TestSshServer,
) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    (id, events)
}

/// What the scripted shell sends back for a typed line: the echo, the output
/// of the command, and the next prompt.
fn transcript(typed: &str, output: &[u8]) -> Vec<u8> {
    [typed.as_bytes(), b"\r\n", output, PROMPT].concat()
}

#[tokio::test]
async fn password_login_opens_a_shell_and_the_prompt_arrives() {
    let server = TestSshServer::start().await;
    let manager = support::manager();

    let (_id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    assert_eq!(output_ending_with(&mut events, PROMPT).await, PROMPT);
}

#[tokio::test]
async fn typed_input_reaches_the_shell_and_its_output_comes_back() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"si".to_vec()).unwrap();
    manager.write(id, b"ze\r".to_vec()).unwrap();

    let expected = transcript("size", b"80x24\r\n");
    assert_eq!(output_ending_with(&mut events, PROMPT).await, expected);
}

#[tokio::test]
async fn colour_sequences_arrive_unaltered() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"colors\r".to_vec()).unwrap();

    let expected = transcript(
        "colors",
        b"\x1b[1;31mdown\x1b[0m \x1b[38;5;46mup\x1b[0m\r\n",
    );
    assert_eq!(output_ending_with(&mut events, PROMPT).await, expected);
}

#[tokio::test]
async fn full_screen_program_output_arrives_unaltered() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"fullscreen\r".to_vec()).unwrap();

    let expected = transcript(
        "fullscreen",
        b"\x1b[?1049h\x1b[2J\x1b[H\x1b[5;10H\xe2\x94\x82 top\x1b[?25l\x1b[?25h\x1b[?1049l",
    );
    assert_eq!(output_ending_with(&mut events, PROMPT).await, expected);
}

#[tokio::test]
async fn output_far_larger_than_one_packet_arrives_complete_and_in_order() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"lines 50000\r".to_vec()).unwrap();

    let received = output_ending_with(&mut events, PROMPT).await;
    let listing = numbered_lines(50000);
    assert!(listing.len() > 500_000);
    assert!(received == transcript("lines 50000", &listing));
}

#[tokio::test]
async fn input_larger_than_the_remote_window_is_delivered_while_its_echo_flows_back() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    // Several times the server's receive window, echoed back byte by byte.
    let pasted = "x".repeat(100_000);
    manager.write(id, pasted.clone().into_bytes()).unwrap();
    manager.write(id, b"\r".to_vec()).unwrap();

    let received = output_ending_with(&mut events, PROMPT).await;
    assert!(received == transcript(&pasted, b""));
}

#[tokio::test]
async fn resizing_the_session_resizes_the_remote_terminal() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    let size = TerminalSize {
        cols: 132,
        rows: 43,
    };
    manager.resize(id, size).unwrap();
    manager.write(id, b"size\r".to_vec()).unwrap();

    let expected = transcript("size", b"132x43\r\n");
    assert_eq!(output_ending_with(&mut events, PROMPT).await, expected);
}

#[tokio::test]
async fn a_resize_made_while_connecting_is_not_lost() {
    let server = TestSshServer::start().await;
    let manager = support::manager();

    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let size = TerminalSize {
        cols: 100,
        rows: 30,
    };
    manager.resize(id, size).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    manager.write(id, b"size\r".to_vec()).unwrap();

    let expected = transcript("size", b"100x30\r\n");
    assert_eq!(output_ending_with(&mut events, PROMPT).await, expected);
}

#[tokio::test]
async fn the_remote_shell_exiting_closes_the_session_with_its_exit_status() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"exit 3\r".to_vec()).unwrap();

    let reason = CloseReason::RemoteExited {
        exit_status: Some(3),
    };
    assert_eq!(next_state(&mut events).await, SessionState::Closed(reason));
    assert_eq!(events.next().await, None);
    assert_eq!(
        manager.write(id, b"x".to_vec()),
        Err(SessionError::NotFound)
    );
}

#[tokio::test]
async fn closing_a_session_ends_it() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.close(id).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
    assert_eq!(events.next().await, None);
    assert_eq!(manager.close(id), Err(SessionError::NotFound));
}

#[tokio::test]
async fn losing_the_connection_closes_the_session() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"drop\r".to_vec()).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ConnectionLost)
    );
}

#[tokio::test]
async fn dropping_the_manager_closes_its_sessions() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (_id, mut events) = open_shell(&manager, &server).await;

    drop(manager);

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
}

#[tokio::test]
async fn a_wrong_password_fails_the_session() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let target = SshTarget {
        auth: SshAuth::Password("wrong".into()),
        ..server.target()
    };

    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

/// Opens a session with `auth` and returns the state that follows
/// `Connecting`.
async fn state_after_connecting(server: &TestSshServer, auth: SshAuth) -> SessionState {
    let manager = support::manager();
    let target = SshTarget {
        auth,
        ..server.target()
    };
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let state = next_state(&mut events).await;
    if state == SessionState::Connected {
        output_ending_with(&mut events, PROMPT).await;
    }
    state
}

fn key_login(key_file: impl Into<std::path::PathBuf>, passphrase: Option<&str>) -> SshAuth {
    SshAuth::PrivateKey {
        key_file: key_file.into(),
        passphrase: passphrase.map(Into::into),
    }
}

#[tokio::test]
async fn an_ed25519_private_key_logs_in() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let key = temp.path().join("id_ed25519");
    server.write_client_key(&key, None);

    let state = state_after_connecting(&server, key_login(&key, None)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn an_rsa_private_key_logs_in() {
    let server = TestSshServer::start().await;

    let state = state_after_connecting(&server, key_login(RSA_CLIENT_KEY, None)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn an_encrypted_private_key_logs_in_with_its_passphrase() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let key = temp.path().join("id_ed25519");
    server.write_client_key(&key, Some("open sesame"));

    let state = state_after_connecting(&server, key_login(&key, Some("open sesame"))).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn an_encrypted_private_key_without_the_right_passphrase_is_reported() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let key = temp.path().join("id_ed25519");
    server.write_client_key(&key, Some("open sesame"));

    for passphrase in [None, Some("wrong")] {
        let state = state_after_connecting(&server, key_login(&key, passphrase)).await;
        assert!(
            matches!(
                &state,
                SessionState::Closed(CloseReason::Failed(OpenError::PrivateKey { file, .. }))
                    if file == &key.display().to_string()
            ),
            "passphrase {passphrase:?}: got {state:?}"
        );
    }
}

#[tokio::test]
async fn a_missing_key_file_is_reported() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();

    let state = state_after_connecting(&server, key_login(temp.path().join("nope"), None)).await;

    assert!(
        matches!(
            state,
            SessionState::Closed(CloseReason::Failed(OpenError::PrivateKey { .. }))
        ),
        "got {state:?}"
    );
}

#[tokio::test]
async fn a_key_the_server_does_not_know_fails_authentication() {
    let server = TestSshServer::start().await;
    let other = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let key = temp.path().join("id_ed25519");
    other.write_client_key(&key, None);

    let state = state_after_connecting(&server, key_login(&key, None)).await;

    assert_eq!(
        state,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

#[tokio::test]
async fn a_port_nobody_listens_on_fails_the_session() {
    // Bind a free port, then stop listening on it.
    let closed_port = {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        listener.local_addr().unwrap().port()
    };
    let manager = support::manager();
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port: closed_port,
        username: "engineer".into(),
        auth: SshAuth::Password("irrelevant".into()),
    };

    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let state = next_state(&mut events).await;
    assert!(
        matches!(
            state,
            SessionState::Closed(CloseReason::Failed(OpenError::Connect(_)))
        ),
        "expected a connect failure, got {state:?}"
    );
}

#[tokio::test]
async fn two_sessions_are_independent() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (first, mut first_events) = open_shell(&manager, &server).await;
    let (second, mut second_events) = open_shell(&manager, &server).await;

    manager.close(first).unwrap();
    manager.write(second, b"size\r".to_vec()).unwrap();

    assert_eq!(
        next_state(&mut first_events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
    let expected = transcript("size", b"80x24\r\n");
    assert_eq!(
        output_ending_with(&mut second_events, PROMPT).await,
        expected
    );
}
