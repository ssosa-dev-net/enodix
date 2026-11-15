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
    Consumer, ConsumerGone, ConsumerKey, OutputOptions, SessionEvent, SessionEvents, SessionId,
    SessionManager, SessionRelays, SessionState, SessionTarget, TerminalSize,
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
