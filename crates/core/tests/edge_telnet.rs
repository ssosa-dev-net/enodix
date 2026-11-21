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
use support::raw::{RawDevice, DO, IAC, UNKNOWN_OPTION, WILL, WONT};
use support::{next_state, output_ending_with, output_until_closed, WAIT};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

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
