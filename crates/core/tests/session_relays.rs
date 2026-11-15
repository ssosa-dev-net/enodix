//! The app's sessions each going through a relay (#83): [`SessionRelays`]
//! keeps every session's relay and the window it is shown in, passes on
//! acknowledgements only for the consumer shown the session now, and
//! drops a relay whose window is found gone, which closes the session as
//! dropping its events did before.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use netterm_core::layout::WindowId;
use netterm_core::session::{
    Consumer, ConsumerGone, ConsumerKey, OutputOptions, Reported, SessionEvent, SessionEvents,
    SessionId, SessionManager, SessionRelays, SessionState, SessionTarget, TerminalSize,
};
use support::{next_state, output_ending_with, TestSshServer, PROMPT, WAIT};
use tokio::sync::mpsc;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };
const MAX_MESSAGE: usize = 32 * 1024;
const QUIET: Duration = Duration::from_millis(500);
const FLOW_WINDOW: usize = 64 * 1024;

/// A window's end of a session: what was delivered to it.
struct Window {
    key: ConsumerKey,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    gone: Arc<AtomicBool>,
}

struct ToWindow {
    events: mpsc::UnboundedSender<SessionEvent>,
    gone: Arc<AtomicBool>,
}

impl Consumer for ToWindow {
    fn deliver(&mut self, event: SessionEvent) -> Result<(), ConsumerGone> {
        self.events.send(event).map_err(|_| ConsumerGone)
    }

    fn gone(self: Box<Self>) {
        self.gone.store(true, Ordering::Release);
    }
}

fn show(relays: &SessionRelays, session: SessionId, events: SessionEvents) -> Window {
    let (sender, receiver) = mpsc::unbounded_channel();
    let gone = Arc::new(AtomicBool::new(false));
    let key = relays.start(
        session,
        WindowId::MAIN,
        events,
        ToWindow {
            events: sender,
            gone: gone.clone(),
        },
    );
    Window {
        key,
        events: receiver,
        gone,
    }
}

impl Window {
    /// Output until none arrives for a while, not acknowledged.
    async fn unacknowledged_output(&mut self) -> Vec<u8> {
        let mut received = Vec::new();
        loop {
            match tokio::time::timeout(QUIET, self.events.recv()).await {
                Ok(Some(SessionEvent::Output(bytes))) => received.extend_from_slice(&bytes),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return received,
            }
        }
    }

    async fn closed(&mut self) {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.events.recv().await {
                    Some(SessionEvent::State(SessionState::Closed(_))) | None => return,
                    Some(_) => {}
                }
            }
        })
        .await
        .expect("the session did not close");
    }
}

fn manager() -> SessionManager {
    support::manager_with_output_options(OutputOptions {
        window: FLOW_WINDOW,
        ..OutputOptions::default()
    })
}

async fn connected(manager: &SessionManager, server: &TestSshServer) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    (id, events)
}

/// Waits until the session no longer takes input.
async fn closes(manager: &SessionManager, id: SessionId) {
    tokio::time::timeout(WAIT, async {
        while manager.write(id, b"x".to_vec()).is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the session stayed open");
}

/// Floods the session and checks how much arrives before flow control
/// stops it, with `acknowledge` called once in between.
async fn output_with_one_acknowledgement(
    manager: &SessionManager,
    id: SessionId,
    window: &mut Window,
    acknowledge: impl FnOnce(),
) -> usize {
    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let mut received = window.unacknowledged_output().await.len();
    acknowledge();
    received += window.unacknowledged_output().await.len();
    received
}

#[tokio::test]
async fn an_acknowledgement_naming_another_consumer_is_ignored() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let mut window = show(&relays, id, events);

    let wrong = ConsumerKey::from_raw(window.key.get() + 1000);
    let received = output_with_one_acknowledgement(&manager, id, &mut window, || {
        relays.acknowledge(id, wrong, 10 * FLOW_WINDOW)
    })
    .await;

    assert!(
        received <= FLOW_WINDOW + MAX_MESSAGE,
        "{received} bytes arrived unacknowledged"
    );
    // The right key makes room.
    relays.acknowledge(id, window.key, received);
    assert!(!window.unacknowledged_output().await.is_empty());
}

#[tokio::test]
async fn an_acknowledgement_for_an_unknown_session_changes_nothing() {
    let relays = SessionRelays::new();

    relays.acknowledge(SessionId(42), ConsumerKey::from_raw(0), 100);

    assert_eq!(relays.shown_in(SessionId(42)), None);
}

#[tokio::test]
async fn a_session_is_shown_in_the_window_that_started_its_relay() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;

    let _window = show(&relays, id, events);

    assert_eq!(relays.shown_in(id), Some(WindowId::MAIN));
}

#[tokio::test]
async fn a_window_found_gone_on_delivery_closes_its_session() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let Window { events, gone, .. } = show(&relays, id, events);

    drop(events);
    manager.write(id, b"lines 2\r".to_vec()).unwrap();

    closes(&manager, id).await;
    assert!(gone.load(Ordering::Acquire), "the consumer was not told");
    assert_eq!(relays.shown_in(id), None);
}

#[tokio::test]
async fn a_reconnect_starts_a_new_relay_and_the_old_key_no_longer_counts() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let mut old = show(&relays, id, events);
    manager.close(id).unwrap();
    old.closed().await;

    let (_, events) = {
        let mut events = manager.reconnect(id, SIZE).unwrap();
        assert_eq!(next_state(&mut events).await, SessionState::Connecting);
        assert_eq!(next_state(&mut events).await, SessionState::Connected);
        output_ending_with(&mut events, PROMPT).await;
        (id, events)
    };
    let mut new = show(&relays, id, events);
    assert_ne!(new.key, old.key);

    let received = output_with_one_acknowledgement(&manager, id, &mut new, || {
        relays.acknowledge(id, old.key, 10 * FLOW_WINDOW)
    })
    .await;

    assert!(
        received <= FLOW_WINDOW + MAX_MESSAGE,
        "{received} bytes arrived unacknowledged"
    );
    assert_eq!(relays.shown_in(id), Some(WindowId::MAIN));
    // The old window going now leaves the new relay be.
    drop(old);
    relays.acknowledge(id, new.key, received);
    assert!(!new.unacknowledged_output().await.is_empty());
}

#[tokio::test]
async fn forgetting_a_session_drops_its_relay_and_closes_it() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let _window = show(&relays, id, events);

    relays.forget(id);
    manager.write(id, b"lines 2\r".to_vec()).unwrap();

    closes(&manager, id).await;
    assert_eq!(relays.shown_in(id), None);
}

// Moving a session to another window (#84).

const SESSION_WINDOW: WindowId = WindowId(1);

fn consumer() -> (
    ToWindow,
    mpsc::UnboundedReceiver<SessionEvent>,
    Arc<AtomicBool>,
) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let gone = Arc::new(AtomicBool::new(false));
    (
        ToWindow {
            events: sender,
            gone: gone.clone(),
        },
        receiver,
        gone,
    )
}

/// Attaches a new window's consumer to `session`, shown in `window`.
fn attach(relays: &SessionRelays, session: SessionId, window: WindowId) -> (Window, u64) {
    let (consumer, events, gone) = consumer();
    let attached = relays
        .attach(session, window, consumer)
        .expect("the session is held by nobody");
    (
        Window {
            key: attached.key,
            events,
            gone,
        },
        attached.offset,
    )
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn a_released_session_is_attached_in_another_window_and_its_output_follows() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let mut main = show(&relays, id, events);
    manager.write(id, b"echo before\r".to_vec()).unwrap();
    let before = main.unacknowledged_output().await;

    let end = relays.release(id, main.key).expect("main shows it");
    let (mut moved, offset) = attach(&relays, id, SESSION_WINDOW);
    manager.write(id, b"echo after\r".to_vec()).unwrap();
    let after = moved.unacknowledged_output().await;

    assert_eq!(end, before.len() as u64);
    assert_eq!(offset, end);
    assert!(text(&after).contains("after"), "{}", text(&after));
    assert!(main.unacknowledged_output().await.is_empty());
    assert_eq!(relays.shown_in(id), Some(SESSION_WINDOW));
}

#[tokio::test]
async fn releasing_with_a_key_that_is_not_shown_the_session_is_refused() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let main = show(&relays, id, events);

    let wrong = ConsumerKey::from_raw(main.key.get() + 1000);

    assert!(relays.release(id, wrong).is_err());
    assert!(relays.release(SessionId(999), main.key).is_err());
    // Still held: another window cannot take it.
    let (consumer, _events, _gone) = consumer();
    assert!(relays.attach(id, SESSION_WINDOW, consumer).is_err());
}

#[tokio::test]
async fn letting_go_of_a_dead_windows_session_hands_it_to_the_next_window() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let mut main = show(&relays, id, events);
    relays.release(id, main.key).unwrap();
    // The session window that took it; its page then dies without a word.
    let (dead, _) = attach(&relays, id, SESSION_WINDOW);

    assert!(relays.let_go(id).is_some());
    let (mut adopting, _) = attach(&relays, id, WindowId::MAIN);
    manager.write(id, b"echo adopted\r".to_vec()).unwrap();

    assert!(text(&adopting.unacknowledged_output().await).contains("adopted"));
    assert!(main.unacknowledged_output().await.is_empty());
    assert_eq!(relays.shown_in(id), Some(WindowId::MAIN));
    // The dead window's key no longer counts.
    relays.acknowledge(id, dead.key, 1);
    assert_eq!(relays.let_go(SessionId(999)), None);
}

#[tokio::test]
async fn a_session_window_found_gone_leaves_its_session_open_for_the_main_window() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = connected(&manager, &server).await;
    let main = show(&relays, id, events);
    relays.release(id, main.key).unwrap();
    let (Window { events, gone, .. }, _) = attach(&relays, id, SESSION_WINDOW);

    drop(events);
    manager.write(id, b"echo lost\r".to_vec()).unwrap();
    tokio::time::timeout(WAIT, async {
        while !gone.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the consumer was not told");

    // Still open, and held by nobody: the main window takes it.
    assert!(manager.write(id, b"echo still\r".to_vec()).is_ok());
    let (mut adopting, _) = attach(&relays, id, WindowId::MAIN);
    let output = text(&adopting.unacknowledged_output().await);
    assert!(output.contains("lost"), "{output}");
    assert!(output.contains("still"), "{output}");
}

#[tokio::test]
async fn the_last_state_shown_is_kept_for_the_next_window() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let mut main = show(&relays, id, events);
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(SessionEvent::State(SessionState::Connected)) = main.events.recv().await {
                return;
            }
        }
    })
    .await
    .expect("the session did not connect");

    assert_eq!(
        relays.reported(id).unwrap().state,
        Some(SessionState::Connected)
    );

    manager.close(id).unwrap();
    main.closed().await;
    assert!(matches!(
        relays.reported(id).unwrap().state,
        Some(SessionState::Closed(_))
    ));
    assert_eq!(relays.reported(SessionId(999)), None);
}

// What a session reported, kept for a window taking it later (#86).

/// Events until the session's log failure arrives, acknowledging output.
async fn until_log_failure(window: &mut Window, relays: &SessionRelays, session: SessionId) {
    tokio::time::timeout(WAIT, async {
        loop {
            match window.events.recv().await {
                Some(SessionEvent::LogFailed(_)) => return,
                Some(SessionEvent::Output(bytes)) => {
                    relays.acknowledge(session, window.key, bytes.len())
                }
                Some(_) => {}
                None => panic!("the session ended before its log failed"),
            }
        }
    })
    .await
    .expect("the log failure did not arrive");
}

#[tokio::test]
async fn what_a_session_reported_is_kept_for_a_window_taking_it_later() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    // A file where the log folder should be: the log cannot be written.
    let temp = tempfile::TempDir::new().unwrap();
    let blocked = temp.path().join("logs");
    std::fs::write(&blocked, "not a folder").unwrap();
    let log = netterm_core::logging::LogFolder::new(&blocked).quick_log("127.0.0.1");
    let (id, events) = manager.open_logged(SessionTarget::Ssh(server.target()), SIZE, Some(log));
    assert!(relays.reported(id).is_none());
    let mut main = show(&relays, id, events);
    until_log_failure(&mut main, &relays, id).await;

    relays.release(id, main.key).unwrap();
    let moved = relays.reported(id).expect("the session has a relay");

    assert_eq!(moved.state, Some(SessionState::Connected));
    let algorithms = moved.algorithms.as_deref().expect("the algorithms agreed");
    assert_eq!(algorithms.len(), 1);
    let failure = moved.log_failure.as_ref().expect("the log failure");
    assert!(
        failure.path.starts_with(&blocked),
        "{}",
        failure.path.display()
    );
    assert_eq!(moved.backup, None);

    // A session window takes it, then dies: the next window is told the same.
    let (_dead, _) = attach(&relays, id, SESSION_WINDOW);
    relays.let_go(id);
    assert_eq!(relays.reported(id), Some(moved));
    assert_eq!(relays.reported(SessionId(999)), None);
}

#[tokio::test]
async fn a_reconnect_reports_afresh() {
    let server = TestSshServer::start().await;
    let manager = manager();
    let relays = SessionRelays::new();
    let (id, events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let mut old = show(&relays, id, events);
    tokio::time::timeout(WAIT, async {
        while !matches!(
            old.events.recv().await,
            Some(SessionEvent::State(SessionState::Connected))
        ) {}
    })
    .await
    .expect("the session did not connect");
    manager.close(id).unwrap();
    old.closed().await;
    assert!(relays.reported(id).unwrap().algorithms.is_some());

    let events = manager.reconnect(id, SIZE).unwrap();
    // Shown to a session window whose page is already gone: nothing of the
    // new connection is delivered, and the session stays open.
    let (consumer, receiver, _gone) = consumer();
    drop(receiver);
    relays.start(id, SESSION_WINDOW, events, consumer);

    assert_eq!(relays.reported(id), Some(Reported::default()));
}
