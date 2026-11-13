//! Output pipeline tests: batching and flow control, observed through the
//! session manager against the in-process test server.

mod support;

use std::time::{Duration, Instant};

use netterm_core::session::{
    CloseReason, OutputOptions, SessionEvent, SessionEvents, SessionId, SessionManager,
    SessionState, SessionTarget, TerminalSize,
};
use support::{
    next_state, numbered_lines, output_batches_ending_with, output_ending_with,
    unacknowledged_output, TestSshServer, CTRL_C, FLOOD_LINE, INTERRUPTED, PROMPT, WAIT,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// The most an SSH data message carries to the client (russh's default
/// maximum packet size): how far one read can overshoot the window.
const MAX_MESSAGE: usize = 32 * 1024;

/// How long without output before a test decides none is coming.
const QUIET: Duration = Duration::from_millis(500);

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

fn transcript(typed: &str, output: &[u8]) -> Vec<u8> {
    [typed.as_bytes(), b"\r\n", output, PROMPT].concat()
}

#[tokio::test]
async fn a_keystroke_after_a_quiet_spell_is_echoed_without_waiting_for_the_batch_timer() {
    let server = TestSshServer::start().await;
    // A batch timer far longer than any echo takes on loopback.
    let manager = SessionManager::with_output_options(OutputOptions {
        batch_interval: Duration::from_secs(1),
        ..OutputOptions::default()
    });
    let (id, mut events) = open_shell(&manager, &server).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let typed = Instant::now();
    manager.write(id, b"x".to_vec()).unwrap();
    let echo = output_ending_with(&mut events, b"x").await;

    assert_eq!(echo, b"x");
    let latency = typed.elapsed();
    assert!(
        latency < Duration::from_millis(500),
        "echo took {latency:?}"
    );
}

#[tokio::test]
async fn output_under_load_arrives_in_full_batches_complete_and_in_order() {
    let server = TestSshServer::start().await;
    let batch_size = 32 * 1024;
    let manager = SessionManager::with_output_options(OutputOptions {
        batch_size,
        batch_interval: Duration::from_secs(1),
        ..OutputOptions::default()
    });
    let (id, mut events) = open_shell(&manager, &server).await;

    // The echo arrives a byte at a time, the listing in packets of at most
    // 32 KiB; both are gathered into batches.
    manager.write(id, b"lines 50000\r".to_vec()).unwrap();
    let batches = output_batches_ending_with(&mut events, PROMPT).await;

    assert!(batches.concat() == transcript("lines 50000", &numbered_lines(50000)));
    let (last, full) = batches.split_last().unwrap();
    assert!(!full.is_empty());
    for batch in full {
        assert!(
            batch.len() >= batch_size,
            "a batch of {} bytes",
            batch.len()
        );
    }
    assert!(!last.is_empty());
}

#[tokio::test]
async fn unacknowledged_output_pauses_reading_and_acknowledging_resumes_it() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = SessionManager::with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, mut events) = open_shell(&manager, &server).await;
    let listing = numbered_lines(200_000);
    assert!(listing.len() > 20 * window);

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let before_pause = unacknowledged_output(&mut events, QUIET).await;

    assert!(!before_pause.is_empty());
    assert!(
        before_pause.len() <= window + MAX_MESSAGE,
        "{} bytes arrived unacknowledged",
        before_pause.len()
    );

    events.acknowledge(before_pause.len());
    let after_pause = output_ending_with(&mut events, PROMPT).await;

    let received = [before_pause, after_pause].concat();
    assert!(received == transcript("lines 200000", &listing));
}

#[tokio::test]
async fn acknowledging_through_the_manager_resumes_reading() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = SessionManager::with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"lines 50000\r".to_vec()).unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(WAIT, async {
        while !received.ends_with(PROMPT) {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    manager.acknowledge(id, bytes.len()).unwrap();
                    received.extend_from_slice(&bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await
    .expect("output stalled");

    assert!(received == transcript("lines 50000", &numbered_lines(50000)));
}

#[tokio::test]
async fn a_device_flooding_a_paused_session_cannot_make_it_buffer_without_bound() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = SessionManager::with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"flood\r".to_vec()).unwrap();
    let received = unacknowledged_output(&mut events, QUIET).await;

    assert!(received.len() <= window + MAX_MESSAGE);
    // A paused session still takes commands.
    manager.close(id).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
}

#[tokio::test]
async fn ctrl_c_stops_a_flood_of_output_and_the_prompt_returns() {
    let server = TestSshServer::start().await;
    let manager = SessionManager::new();
    let (id, mut events) = open_shell(&manager, &server).await;

    manager.write(id, b"flood\r".to_vec()).unwrap();
    // Far more than one window has gone through flow control.
    let mut flooded = 0;
    while flooded < 4 * OutputOptions::default().window {
        flooded += output_ending_with(&mut events, FLOOD_LINE).await.len();
    }
    manager.write(id, vec![CTRL_C]).unwrap();
    output_ending_with(&mut events, &[INTERRUPTED, PROMPT].concat()).await;

    manager.write(id, b"size\r".to_vec()).unwrap();
    assert_eq!(
        output_ending_with(&mut events, PROMPT).await,
        transcript("size", b"80x24\r\n")
    );
}
