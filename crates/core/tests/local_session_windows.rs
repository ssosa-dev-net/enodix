//! Local shell sessions on Windows (#22), on ConPTY. ConPTY's output does
//! not end when its child exits, so a shell that ends must be noticed by
//! watching the process itself: the tab then closes by itself.

#![cfg(windows)]

mod support;

use enodix_core::session::{
    CloseReason, LocalTarget, SessionManager, SessionState, SessionTarget, TerminalSize,
};
use support::{known_hosts, next_state};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

#[tokio::test]
async fn a_shell_that_exits_closes_the_session_with_its_status() {
    let manager = SessionManager::new(known_hosts());
    let (_, mut events) = manager.open(
        SessionTarget::Local(LocalTarget {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), "exit 3".into()],
            folder: None,
            environment: Vec::new(),
        }),
        SIZE,
    );

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::LocalExited {
            exit_status: Some(3)
        })
    );
}

#[tokio::test]
async fn typing_exit_in_an_interactive_shell_closes_the_session() {
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = manager.open(
        SessionTarget::Local(LocalTarget {
            program: "cmd.exe".into(),
            args: Vec::new(),
            folder: None,
            environment: Vec::new(),
        }),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);

    manager.write(id, b"exit 5\r".to_vec()).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::LocalExited {
            exit_status: Some(5)
        })
    );
}
