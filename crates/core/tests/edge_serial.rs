//! Edge testing of serial sessions (#118), as far as the virtual port pair
//! goes (`support::serial`): the adapter pulled out mid-output and while a
//! paste is held back, closing with a megabyte paste the device will not
//! take, a console answering a byte at a time, a dropped reader, a port
//! another program holds.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use netterm_core::paste::{plan_paste, PasteSafety};
use netterm_core::session::{
    CloseReason, OpenError, OutputOptions, SerialParameters, SerialTarget, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, TerminalSize,
};
use support::serial::VirtualSerialPorts;
use support::{known_hosts, next_state, output_of_at_least, output_until_closed, WAIT};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

fn manager(ports: &VirtualSerialPorts) -> SessionManager {
    SessionManager::with_serial_ports(
        known_hosts(),
        OutputOptions::default(),
        Arc::new(ports.clone()),
    )
}

fn target(port: &str) -> SerialTarget {
    SerialTarget {
        port: port.into(),
        parameters: SerialParameters::default(),
    }
}

async fn open(manager: &SessionManager, port: &str) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Serial(target(port)), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

fn listing(lines: u32) -> Vec<u8> {
    (1..=lines)
        .flat_map(|n| format!("line {n}\r\n").into_bytes())
        .collect()
}

/// A megabyte of configuration, as lines the interface pastes one by one.
fn megabyte_paste() -> Vec<String> {
    let text: String = (1..=20_000)
        .map(|n| format!("interface Ethernet{n} description pasted line number {n}\r"))
        .collect();
    assert!(text.len() > 1 << 20);
    plan_paste(&text, &PasteSafety::default(), false).lines
}

#[tokio::test(flavor = "multi_thread")]
async fn unplugging_mid_output_ends_the_session_after_what_had_arrived_in_order() {
    let ports = VirtualSerialPorts::new();
    let device = Arc::new(ports.add("COM3", "USB Serial Port"));
    let manager = manager(&ports);
    let (_id, mut events) = open(&manager, "COM3").await;
    let output = listing(200_000);
    let sending = {
        let (device, output) = (device.clone(), output.clone());
        // Blocks while the session does not read; fails once unplugged.
        tokio::task::spawn_blocking(move || device.send(&output))
    };

    let start = output_of_at_least(&mut events, 64 * 1024).await;
    device.unplug();
    let (rest, reason) = output_until_closed(&mut events, WAIT).await;

    assert!(matches!(reason, CloseReason::PortLost { .. }), "{reason:?}");
    let received = [start, rest].concat();
    assert!(
        output.starts_with(&received),
        "output lost, doubled or out of order"
    );
    assert!(received.len() < output.len());
    let _ = sending.await;
}

/// The device stops reading (a console paused by flow control): a paste
/// waits, and pulling the adapter out still ends the session at once.
#[tokio::test(flavor = "multi_thread")]
async fn unplugging_while_a_paste_is_held_back_ends_the_session() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);
    let (id, mut events) = open(&manager, "COM3").await;
    for line in megabyte_paste() {
        manager.write(id, line.into_bytes()).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    device.unplug();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;

    assert!(matches!(reason, CloseReason::PortLost { .. }), "{reason:?}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    device.wait_until_closed().await;
}

/// Closing while a megabyte paste is still held back by a device that
/// does not read: the session waits a little for it, then ends, and the
/// port is free again.
#[tokio::test(flavor = "multi_thread")]
async fn closing_with_a_megabyte_paste_the_device_will_not_take_ends_in_bounded_time() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);
    let (id, mut events) = open(&manager, "COM3").await;
    for line in megabyte_paste() {
        manager.write(id, line.into_bytes()).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    manager.close(id).unwrap();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;

    assert_eq!(reason, CloseReason::ClosedLocally);
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "{:?}",
        started.elapsed()
    );
    assert!(!device.is_open(), "the port is still held");
    // Free for the next session at once.
    open(&manager, "COM3").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_megabyte_paste_reaches_the_console_whole_and_in_order() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);
    let (id, _events) = open(&manager, "COM3").await;
    let lines = megabyte_paste();
    let expected = lines.concat().into_bytes();

    for line in lines {
        manager.write(id, line.into_bytes()).unwrap();
    }
    let received = device
        .received_ending_with(b"description pasted line number 20000\r")
        .await;

    assert!(received == expected, "the paste arrived changed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_console_answering_a_byte_at_a_time_shows_each_byte_as_it_comes() {
    let ports = VirtualSerialPorts::new();
    let device = Arc::new(ports.add("COM3", "USB Serial Port"));
    let manager = manager(&ports);
    let (_id, mut events) = open(&manager, "COM3").await;

    let typing = {
        let device = device.clone();
        tokio::task::spawn_blocking(move || {
            for &byte in b"Router>" {
                std::thread::sleep(Duration::from_millis(40));
                device.send(&[byte]).unwrap();
            }
        })
    };
    let mut seen = Vec::new();
    let mut first = None;
    while seen != b"Router>" {
        match tokio::time::timeout(WAIT, events.next()).await.unwrap() {
            Some(SessionEvent::Output(bytes)) => {
                events.acknowledge(bytes.len());
                seen.extend_from_slice(&bytes);
                first.get_or_insert(bytes);
            }
            other => panic!("expected output, got {other:?}"),
        }
    }
    typing.await.unwrap();
    // Not held back to be gathered: the first bytes came while the rest
    // were still on their way.
    assert!(first.unwrap().len() < 7, "the prompt came in one piece");
}

/// The reader of a session paused by flow control goes away without
/// closing: the session ends and lets go of the port.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_session_whose_reader_goes_away_lets_go_of_the_port() {
    let ports = VirtualSerialPorts::new();
    let device = Arc::new(ports.add("COM3", "USB Serial Port"));
    let manager = SessionManager::with_serial_ports(
        known_hosts(),
        OutputOptions {
            window: 16 * 1024,
            ..OutputOptions::default()
        },
        Arc::new(ports.clone()),
    );
    let (_id, events) = open(&manager, "COM3").await;
    let flooding = {
        let device = device.clone();
        tokio::task::spawn_blocking(move || while device.send(&[b'x'; 4096]).is_ok() {})
    };
    tokio::time::sleep(Duration::from_millis(500)).await;

    drop(events);

    device.wait_until_closed().await;
    device.unplug();
    let _ = flooding.await;
}

/// A port another program holds is refused with the system's words and a
/// hint at the usual causes.
#[tokio::test(flavor = "multi_thread")]
async fn a_port_held_by_another_program_is_refused_with_a_hint() {
    let ports = VirtualSerialPorts::new();
    let _device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);
    // Another program holds it (here: another session).
    let (_id, _held) = open(&manager, "COM3").await;

    let (_id, mut events) = manager.open(SessionTarget::Serial(target("COM3")), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let error = match next_state(&mut events).await {
        SessionState::Closed(CloseReason::Failed(error)) => error,
        other => panic!("expected a failure, got {other:?}"),
    };

    let OpenError::SerialPort { reason, .. } = &error else {
        panic!("expected a serial port failure, got {error:?}");
    };
    assert!(reason.contains("in use by another program"), "{reason}");
}
