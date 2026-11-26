//! Local shell sessions (#22): a shell on this computer on a pseudo
//! terminal, observed as the bytes, states and close reasons a user would
//! see. Runs `/bin/sh`, so on Unix only; the Windows shells are covered by
//! the smoke test.

#![cfg(unix)]

mod support;

use std::time::Duration;

use enodix_core::session::{
    local_shells, CloseReason, LocalTarget, OpenError, OutputOptions, SessionEvent, SessionEvents,
    SessionId, SessionManager, SessionState, SessionTarget, TerminalSize, Transport,
};
use support::{known_hosts, next_state, unacknowledged_output};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

fn sh(folder: Option<&std::path::Path>, environment: &[(&str, &str)]) -> LocalTarget {
    LocalTarget {
        program: "/bin/sh".into(),
        args: Vec::new(),
        folder: folder.map(Into::into),
        environment: environment
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect(),
    }
}

/// Reads (and acknowledges) output until it contains `expected`: a shell
/// prints its prompt after each answer, so the answer is not the end.
async fn output_containing(events: &mut SessionEvents, expected: &[u8]) {
    let mut received = Vec::new();
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while !received
            .windows(expected.len())
            .any(|part| part == expected)
        {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    received.extend_from_slice(&bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await;
    assert!(
        found.is_ok(),
        "never saw {:?}; got {:?}",
        String::from_utf8_lossy(expected),
        String::from_utf8_lossy(&received)
    );
}

async fn open(manager: &SessionManager, target: LocalTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Local(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

#[tokio::test]
async fn a_local_shell_runs_what_is_typed() {
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, sh(None, &[])).await;

    manager
        .write(id, b"echo hello-$((6*7))\n".to_vec())
        .unwrap();

    output_containing(&mut events, b"hello-42\r\n").await;
    assert_eq!(manager.transport(id), Ok(Transport::Local));
    assert_eq!(manager.encrypted(id), Ok(true));
}

#[tokio::test]
async fn a_local_shell_starts_in_the_folder_and_gets_the_environment() {
    let folder = TempDir::new().unwrap();
    let real = std::fs::canonicalize(folder.path()).unwrap();
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(
        &manager,
        sh(Some(&real), &[("ENODIX_WORKSPACE", "/some/workspace")]),
    )
    .await;

    manager
        .write(id, b"echo [$PWD] [$ENODIX_WORKSPACE]\n".to_vec())
        .unwrap();

    let expected = format!("[{}] [/some/workspace]\r\n", real.display());
    output_containing(&mut events, expected.as_bytes()).await;
}

#[tokio::test]
async fn a_local_shell_announces_the_terminal_and_follows_its_size() {
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, sh(None, &[])).await;

    manager
        .write(id, b"echo $TERM; stty size\n".to_vec())
        .unwrap();
    output_containing(&mut events, b"xterm-256color\r\n24 80\r\n").await;

    manager
        .resize(
            id,
            TerminalSize {
                cols: 100,
                rows: 30,
            },
        )
        .unwrap();
    manager.write(id, b"stty size\n".to_vec()).unwrap();
    output_containing(&mut events, b"30 100\r\n").await;
}

#[tokio::test]
async fn exiting_the_shell_closes_the_session_with_its_status() {
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, sh(None, &[])).await;

    manager.write(id, b"exit 3\n".to_vec()).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::LocalExited {
            exit_status: Some(3)
        })
    );
}

#[tokio::test]
async fn closing_the_session_ends_the_shell() {
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, sh(None, &[])).await;

    manager.close(id).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
}

#[tokio::test]
async fn a_program_that_is_not_there_is_a_failure_that_names_it() {
    let manager = SessionManager::new(known_hosts());
    let (_, mut events) = manager.open(
        SessionTarget::Local(LocalTarget {
            program: "/no/such/shell".into(),
            args: Vec::new(),
            folder: None,
            environment: Vec::new(),
        }),
        SIZE,
    );

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    match next_state(&mut events).await {
        SessionState::Closed(CloseReason::Failed(OpenError::LocalShell { program, .. })) => {
            assert_eq!(program, "/no/such/shell");
        }
        other => panic!("expected the shell to fail to start, got {other:?}"),
    }
}

#[tokio::test]
async fn a_shell_can_be_reconnected_after_it_ended() {
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, sh(None, &[])).await;
    manager.write(id, b"exit\n".to_vec()).unwrap();
    assert!(matches!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::LocalExited { .. })
    ));

    let mut events = manager.reconnect(id, SIZE).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    manager.write(id, b"echo again\n".to_vec()).unwrap();
    output_containing(&mut events, b"again\r\n").await;
}

#[tokio::test]
async fn output_waits_for_acknowledgement_like_any_session() {
    let window = 4 * 1024;
    let manager = SessionManager::with_output_options(
        known_hosts(),
        OutputOptions {
            window,
            ..OutputOptions::default()
        },
    );
    let (id, mut events) = open(&manager, sh(None, &[])).await;

    manager
        .write(id, b"i=0; while [ $i -lt 4000 ]; do echo line-$i-xxxxxxxxxxxxxxxxxxxxxxxx; i=$((i+1)); done\n".to_vec())
        .unwrap();

    let held = unacknowledged_output(&mut events, Duration::from_millis(500)).await;
    // The shell has far more to say than the window, and reading stopped.
    assert!(!held.is_empty());
    assert!(
        held.len() < 4 * window,
        "read {} bytes unacknowledged",
        held.len()
    );
}

#[test]
fn this_computer_has_a_shell_to_open() {
    assert!(!local_shells().is_empty());
}
