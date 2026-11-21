//! Edge testing of telnet sessions (#118): option negotiation that never
//! ends, a device that sends nothing but negotiation, a login prompt and
//! commands split across packets, a console that answers a byte at a time,
//! a device closing with output in flight, a megabyte pasted into a device
//! that reads slowly. Played by [`RawDevice`], which speaks raw bytes.

mod support;

use std::time::{Duration, Instant};

use netterm_core::paste::{plan_paste, PasteSafety};
use netterm_core::session::{
    CloseReason, SessionEvent, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, TelnetTarget, TerminalSize,
};
use support::proxy::LinkProxy;
use support::raw::{RawDevice, DO, ECHO, IAC, UNKNOWN_OPTION, WILL, WONT};
use support::telnet::{TestTelnetServer, PASSWORD_PROMPT, TELNET_PROMPT, USERNAME_PROMPT};
use support::{
    next_state, output_ending_with, output_until_closed, unacknowledged_output, CTRL_C,
    INTERRUPTED, PASSWORD, USERNAME, WAIT,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A connect timeout short enough for tests about silence: a quiet session
/// probes the device this often.
const SHORT: Duration = Duration::from_secs(1);

/// How long a session may take to notice a link that went silent with
/// [`SHORT`]: three timeouts, and room for a loaded machine.
const NOTICED: Duration = Duration::from_secs(12);

/// The telnet option a probe asks about (RFC 860).
const TIMING_MARK: u8 = 6;

const NAWS: u8 = 31;
const SB: u8 = 250;

async fn open(manager: &SessionManager, target: TelnetTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Telnet(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

/// Waits until `done` holds, polling; panics after [`WAIT`].
async fn wait_until(done: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A device that offers and withdraws the echo without end and never
/// reads the answers: the session must stop reading once its answers back
/// up, rather than queue them without bound, and still close at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_negotiating_without_end_cannot_make_the_session_queue_without_bound() {
    let device = RawDevice::negotiating_forever().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, device.telnet_target()).await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let sent = device.sent();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Each change costs the session a three-byte answer it cannot send:
    // what the device got through must stop growing, far below what an
    // unbounded queue would take in a second.
    eprintln!("the device sent {} KiB of requests", device.sent() >> 10);
    assert!(device.sent() < 16 << 20, "{} bytes taken in", device.sent());
    assert!(device.sent() - sent < 1 << 20, "still taking requests in");
    let started = Instant::now();
    manager.close(id).unwrap();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;
    assert_eq!(reason, CloseReason::ClosedLocally);
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// A device that asks again each time the session refuses an option: the
/// two would go on for ever; the session stops answering the same request.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_asking_again_after_every_refusal_does_not_keep_the_session_answering() {
    let device = RawDevice::ping_pong().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, device.telnet_target()).await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let refusals = device.refusals();
    eprintln!("the session refused {refusals} times in a second");
    assert!(refusals < 100, "{refusals} refusals");
    // The session still works.
    manager.write(id, b"x".to_vec()).unwrap();
    wait_until(|| device.received().ends_with(b"x"), "the input to arrive").await;
    manager.close(id).unwrap();
    assert_eq!(
        output_until_closed(&mut events, WAIT).await.1,
        CloseReason::ClosedLocally
    );
}

/// A device that sends nothing but negotiation: the session is connected,
/// shows nothing, takes input and closes.
#[tokio::test]
async fn a_device_that_sends_only_negotiation_shows_nothing_and_takes_input() {
    let negotiation = [
        IAC,
        WILL,
        1,
        IAC,
        WILL,
        3,
        IAC,
        DO,
        24,
        IAC,
        DO,
        NAWS,
        IAC,
        DO,
        UNKNOWN_OPTION,
    ];
    let device = RawDevice::sending(vec![(Duration::ZERO, negotiation.to_vec())]).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, device.telnet_target()).await;

    let shown = tokio::time::timeout(Duration::from_millis(500), events.next()).await;
    assert!(shown.is_err(), "the session showed {shown:?}");
    manager.write(id, b"hello\r".to_vec()).unwrap();
    wait_until(
        || device.received().ends_with(b"hello\r\n"),
        "the input to arrive",
    )
    .await;
    // The unknown option was refused, the window size told.
    let received = device.received();
    assert!(received
        .windows(3)
        .any(|w| w == [IAC, WONT, UNKNOWN_OPTION]));
    assert!(received
        .windows(7)
        .any(|w| w == [IAC, SB, NAWS, 0, 80, 0, 24]));
}

/// A login prompt, and the telnet commands around it, split across
/// packets at every awkward place.
#[tokio::test]
async fn a_login_prompt_and_commands_split_across_packets_are_put_back_together() {
    let pause = Duration::from_millis(30);
    let pieces: Vec<&[u8]> = vec![
        &[IAC],
        &[DO],
        &[NAWS, IAC, WILL],
        &[1],
        b"\r",
        b"\0Rou",
        b"ter\r\n\r\nUser",
        b"name",
        &[b':', b' ', IAC],
        &[IAC, b'!', b'\r'],
        b"\n",
    ];
    let device = RawDevice::sending(pieces.iter().map(|p| (pause, p.to_vec())).collect()).await;
    let manager = support::manager();
    let (_id, mut events) = open(&manager, device.telnet_target()).await;

    let shown = output_ending_with(&mut events, b"!\r\n").await;

    assert_eq!(shown, b"\rRouter\r\n\r\nUsername: \xff!\r\n");
    wait_until(
        || {
            device
                .received()
                .windows(7)
                .any(|w| w == [IAC, SB, NAWS, 0, 80, 0, 24])
        },
        "the window size",
    )
    .await;
}

#[tokio::test]
async fn a_console_answering_a_byte_at_a_time_shows_its_prompt_whole() {
    let prompt = b"\r\nUser Access Verification\r\n\r\nUsername: ";
    let pieces = prompt
        .iter()
        .map(|&b| (Duration::from_millis(15), vec![b]))
        .collect();
    let device = RawDevice::sending(pieces).await;
    let manager = support::manager();
    let (_id, mut events) = open(&manager, device.telnet_target()).await;

    assert_eq!(output_ending_with(&mut events, b"Username: ").await, prompt);
}

#[tokio::test]
async fn a_device_closing_with_output_in_flight_delivers_all_of_it_first() {
    let listing: Vec<u8> = (1..=50_000)
        .flat_map(|n| format!("line {n}\r\n").into_bytes())
        .collect();
    let device = RawDevice::sending_then_closing(vec![(Duration::ZERO, listing.clone())]).await;
    let manager = support::manager();
    let (_id, mut events) = open(&manager, device.telnet_target()).await;

    let (output, reason) = output_until_closed(&mut events, WAIT).await;

    assert!(
        output == listing,
        "{} of {} bytes",
        output.len(),
        listing.len()
    );
    assert_eq!(reason, CloseReason::RemoteExited { exit_status: None });
}

/// A megabyte pasted, line by line as the interface sends it, into a
/// device that takes it in slowly: all of it arrives in order, and
/// closing while most of it still waits ends the session at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_megabyte_pasted_into_a_slow_device_arrives_whole_and_closing_does_not_wait_for_it() {
    let text: String = (1..=20_000)
        .map(|n| format!("interface Ethernet{n} description pasted line number {n}\r"))
        .collect();
    assert!(text.len() > 1 << 20);
    let plan = plan_paste(&text, &PasteSafety::default(), false);
    let expected = text.replace('\r', "\r\n").into_bytes();

    // A device that takes 1 KiB at a time and keeps up.
    let device = RawDevice::reading_slowly(b"", Duration::ZERO).await;
    let manager = support::manager();
    let (id, _events) = open(&manager, device.telnet_target()).await;
    for line in &plan.lines {
        manager.write(id, line.clone().into_bytes()).unwrap();
    }
    wait_until(
        || device.received().len() >= expected.len() + 9,
        "the paste to arrive",
    )
    .await;
    let received = device.received();
    // After the session's opening offers (nine bytes).
    assert!(
        received[9..] == expected[..],
        "the paste arrived out of order"
    );

    // A device that takes 1 KiB every 100 ms: the paste would take minutes.
    let slow = RawDevice::reading_slowly(b"", Duration::from_millis(100)).await;
    let (id, mut events) = open(&manager, slow.telnet_target()).await;
    for line in &plan.lines {
        manager.write(id, line.clone().into_bytes()).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    manager.close(id).unwrap();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;
    assert_eq!(reason, CloseReason::ClosedLocally);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

// Links that go silent (#133). Telnet has no keepalives of its own: a
// session that hears nothing for a connect timeout asks the device for a
// timing mark (`IAC DO TIMING-MARK`), which a telnet server answers, and a
// device that answered one and then answers nothing for about three
// timeouts is lost.

/// A telnet target behind `link`, with the short timeout.
fn through(link: &LinkProxy) -> TelnetTarget {
    TelnetTarget {
        host: "127.0.0.1".into(),
        port: link.port(),
        connect_timeout: SHORT,
    }
}

/// Whether the session asked `received`'s device for a timing mark, and
/// how many times.
fn probes_in(received: &[u8]) -> usize {
    received
        .windows(3)
        .filter(|w| *w == [IAC, DO, TIMING_MARK])
        .count()
}

/// A quiet session over a healthy link stays connected, the device
/// answering a timing mark each timeout (nothing of it shown); the link
/// going silent then ends it as lost within about three timeouts.
#[tokio::test(flavor = "multi_thread")]
async fn a_telnet_link_that_goes_silent_ends_the_session_as_lost_in_bounded_time() {
    let server = TestTelnetServer::start().await;
    let link = LinkProxy::start(server.port()).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, through(&link)).await;
    output_ending_with(&mut events, USERNAME_PROMPT).await;

    // Quiet for several timeouts: probed and answered, nothing shown.
    wait_until(|| server.probes() >= 3, "three probes answered").await;
    let shown = tokio::time::timeout(Duration::from_millis(50), events.next()).await;
    assert!(shown.is_err(), "the session showed {shown:?}");
    manager
        .write(id, format!("{USERNAME}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, PASSWORD_PROMPT).await;

    link.stall();
    let started = Instant::now();
    let (_, reason) = output_until_closed(&mut events, NOTICED).await;

    assert_eq!(reason, CloseReason::ConnectionLost);
    eprintln!("a silent telnet link noticed after {:?}", started.elapsed());
    assert!(started.elapsed() >= SHORT * 2, "{:?}", started.elapsed());
}

/// A device that speaks telnet but never answers a timing mark (it is not
/// obliged to): asked a few times, then left alone, and never taken for
/// dead however long it stays quiet.
#[tokio::test(flavor = "multi_thread")]
async fn a_telnet_device_that_never_answers_a_timing_mark_is_not_taken_for_dead() {
    let opening = [IAC, WILL, ECHO].to_vec();
    let mut pieces = vec![(Duration::ZERO, opening)];
    pieces.push((Duration::ZERO, b"Username: ".to_vec()));
    let device = RawDevice::sending(pieces).await;
    let manager = support::manager();
    let target = TelnetTarget {
        connect_timeout: SHORT,
        ..device.telnet_target()
    };
    let (id, mut events) = open(&manager, target).await;
    output_ending_with(&mut events, b"Username: ").await;

    // Quiet for six timeouts.
    let quiet = tokio::time::timeout(SHORT * 6, events.next()).await;
    assert!(quiet.is_err(), "the session ended or showed {quiet:?}");

    let probes = probes_in(&device.received());
    assert!((1..=3).contains(&probes), "{probes} probes");
    manager.write(id, b"admin\r".to_vec()).unwrap();
    wait_until(
        || device.received().ends_with(b"admin\r\n"),
        "the input to arrive",
    )
    .await;
}

/// A device that never sends a telnet command (a console server's raw
/// port, say) is never sent one either: the bytes would reach the console.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_never_negotiates_is_never_probed() {
    let device = RawDevice::sending(vec![(Duration::ZERO, b"login: ".to_vec())]).await;
    let manager = support::manager();
    let target = TelnetTarget {
        connect_timeout: SHORT,
        ..device.telnet_target()
    };
    let (_id, mut events) = open(&manager, target).await;
    output_ending_with(&mut events, b"login: ").await;

    let quiet = tokio::time::timeout(SHORT * 4, events.next()).await;

    assert!(quiet.is_err(), "the session ended or showed {quiet:?}");
    assert_eq!(probes_in(&device.received()), 0);
}

/// While the session's output is held back (a hidden terminal behind), an
/// answer to a probe could not be read: the silence does not count, and a
/// healthy session held for several timeouts carries on.
#[tokio::test(flavor = "multi_thread")]
async fn a_telnet_session_held_back_for_several_timeouts_is_not_taken_for_dead() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let target = TelnetTarget {
        connect_timeout: SHORT,
        ..server.target()
    };
    let (id, mut events) = open(&manager, target).await;
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    manager
        .write(id, format!("{USERNAME}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, PASSWORD_PROMPT).await;
    manager
        .write(id, format!("{PASSWORD}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, TELNET_PROMPT).await;
    // The device has answered a probe, so the session counts on its
    // answers from now on (the answer itself is not shown).
    wait_until(|| server.probes() >= 1, "a probe answered").await;
    let quiet = tokio::time::timeout(Duration::from_millis(200), events.next()).await;
    assert!(quiet.is_err(), "the session showed {quiet:?}");
    manager.write(id, b"flood\r".to_vec()).unwrap();
    let held = unacknowledged_output(&mut events, Duration::from_millis(300)).await;

    tokio::time::sleep(SHORT * 4).await;
    events.acknowledge(held.len());
    manager.write(id, vec![CTRL_C]).unwrap();

    output_ending_with(&mut events, &[INTERRUPTED, TELNET_PROMPT].concat()).await;
}

/// Output that is not UTF-8, and characters split across packets, reach
/// the session's reader byte for byte.
#[tokio::test]
async fn output_that_is_not_utf8_and_split_characters_arrive_byte_for_byte() {
    let pieces: Vec<&[u8]> = vec![
        b"caf\xe9 \x80 \xc3",
        b"\xa9 \xe8\xa1",
        b"\xa8 e",
        "\u{301}\r\n".as_bytes(),
    ];
    let device = RawDevice::sending(
        pieces
            .iter()
            .map(|p| (Duration::from_millis(30), p.to_vec()))
            .collect(),
    )
    .await;
    let manager = support::manager();
    let (_id, mut events) = open(&manager, device.telnet_target()).await;

    let mut shown = Vec::new();
    while !shown.ends_with(b"\r\n") {
        match tokio::time::timeout(WAIT, events.next()).await.unwrap() {
            Some(SessionEvent::Output(bytes)) => {
                events.acknowledge(bytes.len());
                shown.extend_from_slice(&bytes);
            }
            other => panic!("expected output, got {other:?}"),
        }
    }
    assert_eq!(shown, pieces.concat());
}
