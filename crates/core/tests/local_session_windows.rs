//! Local shell sessions on Windows (#22), on ConPTY. ConPTY's output does
//! not end when its child exits, so a shell that ends must be noticed by
//! watching the process itself: the tab then closes by itself.
//!
//! portable-pty makes the pseudo console inherit the cursor position, so
//! ConPTY first asks the terminal where the cursor is (`ESC [ 6 n`) and holds
//! the program until it is answered. xterm.js answers in the app; the tests
//! answer as a terminal would, in `until_closed`.

#![cfg(windows)]

mod support;

use std::time::Duration;

use enodix_core::session::{
    CloseReason, LocalTarget, SessionEvent, SessionId, SessionManager, SessionState, SessionTarget,
    TerminalSize,
};
use support::known_hosts;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// The cursor position request ConPTY starts with.
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

/// What a terminal answers it with.
const CURSOR_REPORT: &[u8] = b"\x1b[1;1R";

fn cmd(args: &[&str]) -> SessionTarget {
    SessionTarget::Local(LocalTarget {
        program: "cmd.exe".into(),
        args: args.iter().map(|arg| (*arg).into()).collect(),
        folder: None,
        environment: Vec::new(),
    })
}

/// Reads the session's events like a terminal does (acknowledging output
/// and answering cursor position requests) until it closes; `after_connected`
/// is typed once it is connected. Returns why it closed.
async fn until_closed(
    manager: &SessionManager,
    id: SessionId,
    mut events: enodix_core::session::SessionEvents,
    after_connected: Option<&[u8]>,
) -> (CloseReason, String) {
    let mut seen = Vec::new();
    let mut answered = 0;
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    seen.extend_from_slice(&bytes);
                    let asked = seen
                        .windows(CURSOR_QUERY.len())
                        .filter(|part| *part == CURSOR_QUERY)
                        .count();
                    while answered < asked {
                        manager.write(id, CURSOR_REPORT.to_vec()).unwrap();
                        answered += 1;
                    }
                }
                Some(SessionEvent::State(SessionState::Connected)) => {
                    if let Some(typed) = after_connected {
                        manager.write(id, typed.to_vec()).unwrap();
                    }
                }
                Some(SessionEvent::State(SessionState::Closed(reason))) => return reason,
                Some(_) => {}
                None => panic!("the session's events ended without a close"),
            }
        }
    })
    .await;
    let output = String::from_utf8_lossy(&seen).into_owned();
    match result {
        Ok(reason) => (reason, output),
        Err(_) => panic!("the session never closed; its output was {output:?}"),
    }
}

#[tokio::test]
async fn a_shell_that_exits_closes_the_session_with_its_status() {
    let manager = SessionManager::new(known_hosts());
    let (id, events) = manager.open(cmd(&["/c", "exit 3"]), SIZE);

    let (reason, _) = until_closed(&manager, id, events, None).await;

    assert_eq!(
        reason,
        CloseReason::LocalExited {
            exit_status: Some(3)
        }
    );
}

#[tokio::test]
async fn typing_exit_in_an_interactive_shell_closes_the_session() {
    let manager = SessionManager::new(known_hosts());
    let (id, events) = manager.open(cmd(&[]), SIZE);

    let (reason, _) = until_closed(&manager, id, events, Some(b"exit 5\r")).await;

    assert_eq!(
        reason,
        CloseReason::LocalExited {
            exit_status: Some(5)
        }
    );
}
