//! What sessions hold (#118): memory stays bounded under a flood read at
//! every pace, and fifty sessions opened and closed in a row leave nothing
//! behind (tasks, sockets, log files). A file of its own, and each test
//! holds [`exclusive`] while it measures, so that nothing else runs in the
//! process meanwhile.

mod support;

use std::time::{Duration, Instant};

use enodix_core::logging::LogFolder;
use enodix_core::session::{
    CloseReason, SessionEvent, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, TerminalSize,
};
use support::memory::{exclusive, open_handles, resident, Peak, MIB};
use support::{
    next_state, output_ending_with, output_until_closed, unacknowledged_output, TestSshServer,
    CTRL_C, INTERRUPTED, PROMPT, WAIT,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// How much the process may grow while a session floods: the flow-control
/// window, the SSH buffers, allocator slack; far below what crosses.
const GROWTH: u64 = 24 * MIB;

async fn open(manager: &SessionManager, server: &TestSshServer) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    (id, events)
}

/// Reads `bytes` of output, acknowledging it, pausing `pause` per batch.
async fn read(events: &mut SessionEvents, bytes: usize, pause: Duration) {
    let mut received = 0;
    while received < bytes {
        match tokio::time::timeout(WAIT, events.next()).await.unwrap() {
            Some(SessionEvent::Output(batch)) => {
                received += batch.len();
                events.acknowledge(batch.len());
                if !pause.is_zero() {
                    tokio::time::sleep(pause).await;
                }
            }
            other => panic!("expected output, got {other:?}"),
        }
    }
}

/// Floods through one session read fast (`fast` bytes), slowly, and not
/// at all for a while; returns how much the process grew at most.
async fn flood_growth(fast: usize) -> Option<u64> {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, &server).await;
    let baseline = resident();
    let peak = Peak::start();

    manager.write(id, b"flood\r".to_vec()).unwrap();
    read(&mut events, fast, Duration::ZERO).await;
    read(&mut events, 2 << 20, Duration::from_millis(2)).await;
    // The reader stops acknowledging while the device keeps flooding.
    let held = unacknowledged_output(&mut events, Duration::from_secs(1)).await;
    events.acknowledge(held.len());
    read(&mut events, 8 << 20, Duration::ZERO).await;
    manager.write(id, vec![CTRL_C]).unwrap();
    output_ending_with(&mut events, &[INTERRUPTED, PROMPT].concat()).await;

    let highest = peak.stop();
    baseline.map(|baseline| highest.saturating_sub(baseline))
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_stays_bounded_under_a_flood_read_fast_slowly_and_not_at_all() {
    let _alone = exclusive().await;
    let fast = 64 << 20;
    if let Some(growth) = flood_growth(fast).await {
        eprintln!(
            "memory grew by {:.1} MiB through a {} MiB flood",
            growth as f64 / MIB as f64,
            fast >> 20
        );
        assert!(growth < GROWTH, "memory grew by {} MiB", growth / MIB);
    }
}

/// `cargo test -p enodix-core --test session_resources -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: a 512 MiB flood (about a minute)"]
async fn memory_stays_bounded_under_a_flood_of_hundreds_of_megabytes() {
    let _alone = exclusive().await;
    let fast = 512 << 20;
    if let Some(growth) = flood_growth(fast).await {
        eprintln!(
            "memory grew by {:.1} MiB through a {} MiB flood",
            growth as f64 / MIB as f64,
            fast >> 20
        );
        assert!(growth < GROWTH, "memory grew by {} MiB", growth / MIB);
    }
}

/// Output a log (or a job) turns into text holding a control sequence
/// that never ends: its parameters must not be kept without bound.
#[test]
fn plain_text_does_not_keep_an_endless_control_sequence() {
    let _alone = exclusive_blocking();
    let mut text = enodix_core::logging::PlainText::new();
    let baseline = resident();
    let peak = Peak::start();
    assert_eq!(text.push(b"before\r\n\x1b["), "before\n");
    let parameters = b"1;".repeat(1 << 20);
    for _ in 0..64 {
        assert_eq!(text.push(&parameters), "");
    }
    let after = text.push(b"mafter\r\n");
    let highest = peak.stop();

    assert_eq!(after, "after\n");
    if let Some(baseline) = baseline {
        let growth = highest.saturating_sub(baseline);
        assert!(growth < 16 * MIB, "grew by {} MiB", growth / MIB);
    }
}

/// [`exclusive`] for a test without a runtime.
fn exclusive_blocking() -> tokio::sync::MutexGuard<'static, ()> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(exclusive())
}

/// Whether `path` can be opened with nobody else holding it.
#[cfg(windows)]
fn held_open(path: &std::path::Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .is_err()
}

/// Linux has no exclusive open; the descriptor count covers it.
#[cfg(not(windows))]
fn held_open(_path: &std::path::Path) -> bool {
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn fifty_sessions_opened_and_closed_in_a_row_leave_nothing_behind() {
    let _alone = exclusive().await;
    let server = TestSshServer::start().await;
    let logs = TempDir::new().unwrap();
    let folder = LogFolder::new(logs.path());
    let manager = support::manager();
    // One session first, so that what lives once per process (the runtime's
    // threads, russh's) is there before counting.
    let (id, mut events) = open(&manager, &server).await;
    manager.close(id).unwrap();
    output_until_closed(&mut events, WAIT).await;
    wait_until(
        || server.connections_alive() == 0,
        "the first connection to end",
    )
    .await;
    let metrics = tokio::runtime::Handle::current().metrics();
    let tasks = metrics.num_alive_tasks();
    let handles = open_handles();

    for n in 0..50 {
        let (id, mut events) = manager.open_logged(
            SessionTarget::Ssh(server.target()),
            SIZE,
            Some(folder.quick_log(&format!("device {n}"))),
        );
        assert_eq!(next_state(&mut events).await, SessionState::Connecting);
        assert_eq!(next_state(&mut events).await, SessionState::Connected);
        output_ending_with(&mut events, PROMPT).await;
        manager.write(id, b"lines 100\r".to_vec()).unwrap();
        output_ending_with(&mut events, PROMPT).await;
        // Half end from this side, half from the device's.
        if n % 2 == 0 {
            manager.close(id).unwrap();
            let (_, reason) = output_until_closed(&mut events, WAIT).await;
            assert_eq!(reason, CloseReason::ClosedLocally);
        } else {
            manager.write(id, b"exit\r".to_vec()).unwrap();
            let (_, reason) = output_until_closed(&mut events, WAIT).await;
            assert!(
                matches!(reason, CloseReason::RemoteExited { .. }),
                "{reason:?}"
            );
        }
        manager.discard(id);
    }

    wait_until(
        || server.connections_alive() == 0,
        "every connection to end",
    )
    .await;
    wait_until(
        || metrics.num_alive_tasks() <= tasks,
        "the sessions' tasks to end",
    )
    .await;
    if let Some(before) = handles {
        // Sockets and files close a little after the session reports it.
        let deadline = Instant::now() + WAIT;
        let mut now = open_handles().unwrap_or(before);
        while now > before + 5 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            now = open_handles().unwrap_or(before);
        }
        eprintln!("handles before {before}, after fifty sessions {now}");
        assert!(now <= before + 5, "{} handles left open", now - before);
    }
    let mut logged = Vec::new();
    for n in 0..50 {
        let dir = folder.quick_log(&format!("device {n}")).folder();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            assert!(!held_open(&path), "{} is still open", path.display());
            logged.push(std::fs::read_to_string(&path).unwrap());
        }
    }
    assert_eq!(logged.len(), 50);
    for log in logged {
        assert!(log.contains("line 100\n"), "{log}");
    }
}

/// Waits until `done` holds, polling; panics after [`WAIT`].
async fn wait_until(done: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
