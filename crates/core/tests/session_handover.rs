//! Handing a live session from one consumer (a window) to another, through
//! a [`Relay`]: output is delivered once and in order across the handover,
//! flow control keeps counting, and a consumer that goes away lets go of
//! the session without closing it (#82).

mod support;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use netterm_core::known_hosts::KnownHosts;
use netterm_core::logging::LogFolder;
use netterm_core::session::{
    Consumer, ConsumerGone, ConsumerKey, HandoverError, HostKeyDecision, OutputOptions, Relay,
    SerialTarget, SessionEvent, SessionId, SessionManager, SessionState, SessionTarget,
    TerminalSize,
};
use support::serial::VirtualSerialPorts;
use support::telnet::{TestTelnetServer, PASSWORD_PROMPT, TELNET_PROMPT, USERNAME_PROMPT};
use support::{
    next_state, numbered_lines, output_ending_with, TestSshServer, CTRL_C, FLOOD_LINE, INTERRUPTED,
    PASSWORD, PROMPT, USERNAME, WAIT,
};
use tokio::sync::mpsc;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// How far one read can overshoot the flow-control window (russh's
/// largest data message).
const MAX_MESSAGE: usize = 32 * 1024;

/// How long without output before a test decides none is coming.
const QUIET: Duration = Duration::from_millis(500);

/// A window showing a session: what the relay delivered to it, in order.
struct Window {
    key: ConsumerKey,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    /// Set once the relay has told this window's consumer it is gone.
    gone: Arc<AtomicBool>,
}

/// The consumer side of a [`Window`].
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

fn window() -> (
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

/// Starts relaying `events` to a first window.
fn relay(events: netterm_core::session::SessionEvents) -> (Relay, Window) {
    let (consumer, events_rx, gone) = window();
    let (relay, key) = Relay::start(events, consumer);
    (
        relay,
        Window {
            key,
            events: events_rx,
            gone,
        },
    )
}

/// Moves the session to a new window; returns it and where its share of
/// the output starts.
fn attach(relay: &Relay) -> (Window, u64) {
    let (consumer, events, gone) = window();
    let attached = relay.attach(consumer).expect("nobody is shown the session");
    (
        Window {
            key: attached.key,
            events,
            gone,
        },
        attached.offset,
    )
}

impl Window {
    async fn next(&mut self) -> SessionEvent {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("timed out waiting for an event")
            .expect("the relay stopped delivering")
    }

    async fn next_state(&mut self, relay: &Relay) -> SessionState {
        loop {
            match self.next().await {
                SessionEvent::State(state) => return state,
                SessionEvent::Output(bytes) => relay.acknowledge(self.key, bytes.len()),
                SessionEvent::Algorithms(_) => {}
                other => panic!("expected a state change, got {other:?}"),
            }
        }
    }

    /// Output until it ends with `expected`, acknowledged as it arrives.
    async fn output_ending_with(&mut self, relay: &Relay, expected: &[u8]) -> Vec<u8> {
        let mut received = Vec::new();
        while !received.ends_with(expected) {
            match self.next().await {
                SessionEvent::Output(bytes) => {
                    relay.acknowledge(self.key, bytes.len());
                    received.extend_from_slice(&bytes);
                }
                other => panic!(
                    "expected output, got {other:?} after {:?}",
                    String::from_utf8_lossy(&received)
                ),
            }
        }
        received
    }

    /// At least `bytes` bytes of output, acknowledged as they arrive.
    async fn output_of_at_least(&mut self, relay: &Relay, bytes: usize) -> Vec<u8> {
        let mut received = Vec::new();
        while received.len() < bytes {
            match self.next().await {
                SessionEvent::Output(output) => {
                    relay.acknowledge(self.key, output.len());
                    received.extend_from_slice(&output);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
        received
    }

    /// Output until none arrives for `quiet`, not acknowledged.
    async fn unacknowledged_output(&mut self, quiet: Duration) -> Vec<u8> {
        let mut received = Vec::new();
        loop {
            match tokio::time::timeout(quiet, self.events.recv()).await {
                Ok(Some(SessionEvent::Output(bytes))) => received.extend_from_slice(&bytes),
                Ok(other) => panic!("expected output, got {other:?}"),
                Err(_) => return received,
            }
        }
    }

    /// The output already delivered and not yet read, acknowledged: what a
    /// window giving the session up still has to show.
    fn delivered_output(&mut self, relay: &Relay) -> Vec<u8> {
        let mut received = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                SessionEvent::Output(bytes) => {
                    relay.acknowledge(self.key, bytes.len());
                    received.extend_from_slice(&bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
        received
    }

    fn nothing_delivered(&mut self) -> bool {
        self.events.try_recv().is_err()
    }
}

async fn connected_shell(
    manager: &SessionManager,
    server: &TestSshServer,
) -> (SessionId, Relay, Window) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    let (relay, window) = relay(events);
    (id, relay, window)
}

fn transcript(typed: &str, output: &[u8], prompt: &[u8]) -> Vec<u8> {
    [typed.as_bytes(), b"\r\n", output, prompt].concat()
}

#[tokio::test]
async fn a_session_moved_mid_flood_delivers_every_byte_once_and_in_order() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, relay, mut first) = connected_shell(&manager, &server).await;
    let listing = numbered_lines(300_000);

    manager.write(id, b"lines 300000\r".to_vec()).unwrap();
    let mut shown_first = first.output_of_at_least(&relay, 512 * 1024).await;
    let offset = relay.release(first.key).unwrap();
    shown_first.extend(first.delivered_output(&relay));
    let (mut second, from) = attach(&relay);
    let shown_second = second.output_ending_with(&relay, PROMPT).await;

    assert_eq!(from, offset);
    assert_eq!(shown_first.len() as u64, offset);
    assert!(
        shown_second.len() > 1024 * 1024,
        "the move was not mid-flood"
    );
    assert!([shown_first, shown_second].concat() == transcript("lines 300000", &listing, PROMPT));
    assert!(first.nothing_delivered());
}

#[tokio::test]
async fn a_session_moved_back_and_forth_keeps_its_output_whole() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, relay, mut window) = connected_shell(&manager, &server).await;
    let listing = numbered_lines(300_000);

    manager.write(id, b"lines 300000\r".to_vec()).unwrap();
    let mut shown = Vec::new();
    for _ in 0..6 {
        shown.extend(window.output_of_at_least(&relay, 64 * 1024).await);
        relay.release(window.key).unwrap();
        shown.extend(window.delivered_output(&relay));
        window = attach(&relay).0;
    }
    shown.extend(window.output_ending_with(&relay, PROMPT).await);

    assert!(shown == transcript("lines 300000", &listing, PROMPT));
}

#[tokio::test]
async fn typing_reaches_the_device_from_the_window_now_showing_the_session() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, relay, first) = connected_shell(&manager, &server).await;

    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);
    manager.write(id, b"lines 2\r".to_vec()).unwrap();

    assert_eq!(
        second.output_ending_with(&relay, PROMPT).await,
        transcript("lines 2", &numbered_lines(2), PROMPT)
    );
}

#[tokio::test]
async fn a_window_that_stops_acknowledging_after_a_move_pauses_the_device_and_resuming_continues() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, relay, first) = connected_shell(&manager, &server).await;
    let listing = numbered_lines(200_000);
    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let before_pause = second.unacknowledged_output(QUIET).await;

    assert!(!before_pause.is_empty());
    assert!(
        before_pause.len() <= window + MAX_MESSAGE,
        "{} bytes arrived unacknowledged",
        before_pause.len()
    );

    relay.acknowledge(second.key, before_pause.len());
    let after_pause = second.output_ending_with(&relay, PROMPT).await;

    assert!([before_pause, after_pause].concat() == transcript("lines 200000", &listing, PROMPT));
}

#[tokio::test]
async fn output_the_old_window_never_acknowledged_counts_as_acknowledged_by_the_move() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, relay, mut first) = connected_shell(&manager, &server).await;
    let listing = numbered_lines(200_000);

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    // The first window stalls: the device is paused on its account.
    let mut shown = first.unacknowledged_output(QUIET).await;
    relay.release(first.key).unwrap();
    while let Ok(SessionEvent::Output(bytes)) = first.events.try_recv() {
        shown.extend(bytes);
    }
    let (mut second, _) = attach(&relay);
    shown.extend(second.output_ending_with(&relay, PROMPT).await);

    assert!(shown == transcript("lines 200000", &listing, PROMPT));
}

#[tokio::test]
async fn acknowledgements_from_a_window_the_session_left_are_ignored() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, relay, first) = connected_shell(&manager, &server).await;
    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let mut unacknowledged = second.unacknowledged_output(QUIET).await;
    // A late acknowledgement from the old window does not make room.
    relay.acknowledge(first.key, 10 * window);
    unacknowledged.extend(second.unacknowledged_output(QUIET).await);

    assert!(
        unacknowledged.len() <= window + MAX_MESSAGE,
        "{} bytes arrived unacknowledged",
        unacknowledged.len()
    );
}

/// A reconnect gives the session new events, and the shell a new relay for
/// them (#83): its consumer's key is one no earlier relay gave out, so a
/// late acknowledgement for the old connection cannot make room.
#[tokio::test]
async fn a_reconnected_sessions_new_relay_ignores_the_old_relays_key() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, old_relay, mut old) = connected_shell(&manager, &server).await;
    manager.close(id).unwrap();
    assert!(matches!(
        old.next_state(&old_relay).await,
        SessionState::Closed(_)
    ));

    let mut events = manager.reconnect(id, SIZE).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    let (relay, mut new) = relay(events);
    assert_ne!(new.key, old.key);

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let mut unacknowledged = new.unacknowledged_output(QUIET).await;
    relay.acknowledge(old.key, 10 * window);
    unacknowledged.extend(new.unacknowledged_output(QUIET).await);

    assert!(
        unacknowledged.len() <= window + MAX_MESSAGE,
        "{} bytes arrived unacknowledged",
        unacknowledged.len()
    );
}

#[tokio::test]
async fn a_window_found_gone_on_delivery_lets_go_of_the_session_without_closing_it() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, relay, mut first) = connected_shell(&manager, &server).await;
    let listing = numbered_lines(200_000);

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let mut shown = first.unacknowledged_output(QUIET).await;
    let gone = first.gone.clone();
    drop(first);
    // Nothing is delivered until a delivery finds the window gone: make room
    // for one.
    let key = relay.consumer().expect("still the first window's");
    relay.acknowledge(key, shown.len());
    tokio::time::timeout(WAIT, async {
        while relay.consumer().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the relay never noticed the window had gone");
    assert!(gone.load(Ordering::Acquire));

    let (mut second, from) = attach(&relay);
    // What the dead window never got goes to the next one; what it did get
    // is lost with it.
    shown.truncate(from as usize);
    shown.extend(second.output_ending_with(&relay, PROMPT).await);

    assert!(shown == transcript("lines 200000", &listing, PROMPT));
    manager.write(id, b"lines 1\r".to_vec()).unwrap();
    second
        .output_ending_with(&relay, &[&numbered_lines(1)[..], PROMPT].concat())
        .await;
}

#[tokio::test]
async fn a_host_key_question_raised_while_nobody_is_shown_the_session_reaches_the_next_window() {
    let server = TestSshServer::start().await;
    let manager = SessionManager::new(KnownHosts::in_memory());
    let (_, events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let (relay, first) = relay(events);

    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);
    let question = loop {
        match second.next().await {
            SessionEvent::HostKey(question) => break question,
            SessionEvent::State(SessionState::Connecting) => {}
            other => panic!("expected the host-key question, got {other:?}"),
        }
    };

    assert_eq!(question.port, server.port());
}

#[tokio::test]
async fn answering_a_question_from_the_new_window_connects_the_session() {
    let server = TestSshServer::start().await;
    let manager = SessionManager::new(KnownHosts::in_memory());
    let (id, events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let (relay, mut first) = relay(events);
    loop {
        if let SessionEvent::HostKey(_) = first.next().await {
            break;
        }
    }

    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);
    manager
        .decide_host_key(id, HostKeyDecision::TrustOnce)
        .unwrap();

    assert_eq!(second.next_state(&relay).await, SessionState::Connected);
}

#[tokio::test]
async fn a_session_is_shown_to_one_window_at_a_time() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (_, relay, first) = connected_shell(&manager, &server).await;
    let (consumer, _, _) = window();

    assert_eq!(relay.attach(consumer).err(), Some(HandoverError::Shown));
    relay.release(first.key).unwrap();
    assert_eq!(
        relay.release(first.key).err(),
        Some(HandoverError::NotShownThere)
    );
}

/// Waits until the session no longer takes input. Like dropping its
/// `SessionEvents`, dropping the relay closes a session once it next has
/// something to report: the echo of what is typed here.
async fn closes(manager: &SessionManager, id: SessionId) {
    tokio::time::timeout(WAIT, async {
        while manager.write(id, b"x".to_vec()).is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the session stayed open");
}

#[tokio::test]
async fn dropping_the_relay_while_nobody_is_shown_the_session_closes_it() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, relay, first) = connected_shell(&manager, &server).await;

    relay.release(first.key).unwrap();
    drop(relay);

    closes(&manager, id).await;
}

#[tokio::test]
async fn dropping_the_relay_while_a_window_is_shown_the_session_closes_it() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, relay, _window) = connected_shell(&manager, &server).await;

    drop(relay);

    closes(&manager, id).await;
}

#[tokio::test]
async fn a_closed_session_can_move_and_the_new_window_hears_nothing_more() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, relay, mut first) = connected_shell(&manager, &server).await;

    manager.close(id).unwrap();
    assert!(matches!(
        first.next_state(&relay).await,
        SessionState::Closed(_)
    ));
    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);

    tokio::time::sleep(QUIET).await;
    assert!(second.nothing_delivered());
}

#[tokio::test]
async fn a_telnet_session_moved_mid_flood_keeps_flooding_whole_lines() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Telnet(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    manager
        .write(id, format!("{USERNAME}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, PASSWORD_PROMPT).await;
    manager
        .write(id, format!("{PASSWORD}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, TELNET_PROMPT).await;
    let (relay, mut first) = relay(events);

    manager.write(id, b"flood\r".to_vec()).unwrap();
    let mut shown = first.output_of_at_least(&relay, 256 * 1024).await;
    relay.release(first.key).unwrap();
    shown.extend(first.delivered_output(&relay));
    let (mut second, _) = attach(&relay);
    shown.extend(second.output_of_at_least(&relay, 256 * 1024).await);
    manager.write(id, vec![CTRL_C]).unwrap();
    shown.extend(
        second
            .output_ending_with(&relay, &[INTERRUPTED, TELNET_PROMPT].concat())
            .await,
    );

    let flood = shown
        .strip_prefix(b"flood\r\n".as_slice())
        .expect("the echo first");
    let flood = &flood[..flood.len() - INTERRUPTED.len() - TELNET_PROMPT.len()];
    assert!(flood.len() > 512 * 1024);
    assert!(
        flood
            .chunks(FLOOD_LINE.len())
            .all(|line| line == FLOOD_LINE),
        "a line was cut, doubled or dropped"
    );
}

#[tokio::test]
async fn a_serial_session_moved_mid_output_delivers_every_byte_once_and_in_order() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM7", "Virtual console");
    let manager = SessionManager::with_serial_ports(
        support::known_hosts(),
        OutputOptions::default(),
        Arc::new(ports),
    );
    let (_, mut events) = manager.open(
        SessionTarget::Serial(SerialTarget {
            port: "COM7".into(),
            parameters: Default::default(),
        }),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    let (relay, mut first) = relay(events);
    let listing = numbered_lines(100_000);

    // The device prints it all while the session moves.
    let sent = listing.clone();
    let printing = std::thread::spawn(move || device.send(&sent).unwrap());
    let mut shown = first.output_of_at_least(&relay, listing.len() / 3).await;
    relay.release(first.key).unwrap();
    shown.extend(first.delivered_output(&relay));
    let (mut second, _) = attach(&relay);
    shown.extend(second.output_ending_with(&relay, b"line 100000\r\n").await);
    printing.join().unwrap();

    assert!(shown == listing);
}

#[tokio::test]
async fn logging_goes_on_in_the_same_file_across_a_move() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let root = tempfile::tempdir().unwrap();
    let log = LogFolder::new(root.path()).quick_log("moved");
    let (id, mut events) =
        manager.open_logged(SessionTarget::Ssh(server.target()), SIZE, Some(log));
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    let (relay, mut first) = relay(events);

    manager.write(id, b"show before\r".to_vec()).unwrap();
    first.output_ending_with(&relay, PROMPT).await;
    relay.release(first.key).unwrap();
    let (mut second, _) = attach(&relay);
    manager.write(id, b"show after\r".to_vec()).unwrap();
    second.output_ending_with(&relay, PROMPT).await;
    manager.close(id).unwrap();
    assert!(matches!(
        second.next_state(&relay).await,
        SessionState::Closed(_)
    ));

    let logs = log_files(&root.path().join("quick connect").join("moved"));
    assert_eq!(logs.len(), 1, "{logs:?}");
    let text = std::fs::read_to_string(&logs[0]).unwrap();
    let before = text.find("show before").expect("logged before the move");
    let after = text.find("show after").expect("logged after the move");
    assert!(before < after);
}

fn log_files(folder: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}
