//! The system's serial driver (`SystemSerialPorts`, the `serialport`
//! crate) on a pseudo terminal, Unix only (#58). A pseudo terminal pair is
//! a serial line as far as the driver goes: the session opens the pair's
//! terminal end (`/dev/pts/<n>`) by its path, as a user types
//! `/dev/ttyUSB0`, and the test plays the device on the other end. This is
//! also how a console reached through `socat pty,link=... tcp:...` looks
//! to the app.
//!
//! What a pseudo terminal cannot show: the line's speed on a wire, a break
//! on the wire (the kernel accepts the request and sends nothing), DTR and
//! RTS (it has no modem lines), and a USB adapter being unplugged (the
//! nearest is the other end closing). A real adapter and its driver stay
//! unchecked.
#![cfg(unix)]

mod support;

use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use enodix_core::session::{
    CloseReason, SerialParameters, SerialTarget, SessionEvents, SessionId, SessionManager,
    SessionState, SessionTarget, TerminalSize, Transport,
};
use enodix_core::workspace::{FlowControl, Parity};
use support::{known_hosts, next_state, output_ending_with};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };
const WAIT: Duration = Duration::from_secs(10);

/// A pseudo terminal pair: the device's end, and the path of the end the
/// app opens as its serial port. The test keeps no descriptor of that end
/// open, as no other program holds a real port.
fn pseudo_terminal() -> (File, String) {
    let mut device = -1;
    let mut line = -1;
    // SAFETY: openpty writes two descriptors into the integers given; the
    // name, terminal settings and window size are left out (null).
    let done = unsafe {
        libc::openpty(
            &mut device,
            &mut line,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(done, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: both descriptors were just opened and are owned here.
    let (device, line) = unsafe { (File::from_raw_fd(device), OwnedFd::from_raw_fd(line)) };
    // ttyname_r, not ttyname: the tests run side by side, and ttyname's
    // buffer is shared.
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: ttyname_r writes at most `name.len()` bytes, NUL-terminated.
    let found = unsafe { libc::ttyname_r(line.as_raw_fd(), name.as_mut_ptr(), name.len()) };
    assert_eq!(
        found,
        0,
        "ttyname_r: {}",
        std::io::Error::from_raw_os_error(found)
    );
    // SAFETY: on success the buffer holds a NUL-terminated name.
    let path = unsafe { CStr::from_ptr(name.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    // The kernel keeps the pair while the device's end is open; the line's
    // end is opened again by path, like a real port.
    drop(line);
    (device, path)
}

fn target(port: &str, parameters: SerialParameters) -> SerialTarget {
    SerialTarget {
        port: port.into(),
        parameters,
    }
}

async fn open(manager: &SessionManager, target: SerialTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Serial(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

/// Reads the device's end until `expected` ends what came, on a thread
/// (the read blocks).
async fn received_ending_with(device: &File, expected: &[u8]) -> Vec<u8> {
    let mut device = device.try_clone().unwrap();
    let expected = expected.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut received = Vec::new();
        let mut buffer = [0; 4096];
        while !received.ends_with(&expected) {
            let count = device.read(&mut buffer).expect("the device's end");
            assert!(count > 0, "the line closed after {received:?}");
            received.extend_from_slice(&buffer[..count]);
        }
        received
    })
    .await
    .unwrap()
}

/// The line's settings as the kernel holds them (the device's end reads
/// the pair's one set of settings).
fn settings(device: &File) -> libc::termios {
    // SAFETY: tcgetattr fills the zeroed struct for an open terminal.
    unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(device.as_raw_fd(), &mut settings), 0);
        settings
    }
}

/// The line's input and output speeds in baud, as the kernel holds them.
/// Read with `termios2`, which gives the number whichever way the speed
/// was set: `cfgetispeed` gives a `Bnnnn` code, or `BOTHER` on kernels that
/// keep a speed set by number that way (as on CI's runners).
fn speeds(device: &File) -> (u32, u32) {
    // SAFETY: TCGETS2 fills the zeroed struct for an open terminal.
    unsafe {
        let mut settings: libc::termios2 = std::mem::zeroed();
        assert_eq!(
            libc::ioctl(device.as_raw_fd(), libc::TCGETS2, &mut settings),
            0
        );
        (settings.c_ispeed, settings.c_ospeed)
    }
}

async fn closed_reason(events: &mut SessionEvents) -> CloseReason {
    match tokio::time::timeout(WAIT, next_state(events)).await {
        Ok(SessionState::Closed(reason)) => reason,
        Ok(other) => panic!("expected the session to close, got {other:?}"),
        Err(_) => panic!("the session did not close"),
    }
}

#[tokio::test]
async fn a_pseudo_terminal_opens_as_a_port_and_bytes_cross_both_ways() {
    let (device, port) = pseudo_terminal();
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, target(&port, SerialParameters::default())).await;
    assert_eq!(manager.transport(id), Ok(Transport::Serial));

    (&device).write_all(b"\r\nSwitch>").unwrap();
    output_ending_with(&mut events, b"Switch>").await;
    manager.write(id, b"show version\r".to_vec()).unwrap();
    assert_eq!(
        received_ending_with(&device, b"\r").await,
        b"show version\r"
    );

    // The driver puts the line in raw mode: every byte value crosses as
    // sent, both ways (no CR to LF, no flow control characters eaten).
    let all: Vec<u8> = (0..=255).collect();
    (&device).write_all(&all).unwrap();
    assert_eq!(output_ending_with(&mut events, &[254, 255]).await, all);
    manager.write(id, all.clone()).unwrap();
    assert_eq!(received_ending_with(&device, &[254, 255]).await, all);
}

#[tokio::test]
async fn the_line_settings_given_are_the_ones_the_kernel_holds() {
    // Data bits and parity are accepted but cannot be read back: Linux's
    // pseudo terminal driver keeps its line at 8 bits, no parity, whatever
    // is asked. Speed, stop bits and software flow control are kept.
    for (parameters, two_stop) in [
        (SerialParameters::default(), false),
        (
            SerialParameters {
                baud_rate: 115_200,
                data_bits: 7,
                parity: Parity::Even,
                stop_bits: 2,
                flow_control: FlowControl::None,
            },
            true,
        ),
        (
            SerialParameters {
                baud_rate: 38_400,
                data_bits: 8,
                parity: Parity::Odd,
                stop_bits: 1,
                flow_control: FlowControl::Software,
            },
            false,
        ),
    ] {
        let (device, port) = pseudo_terminal();
        let manager = SessionManager::new(known_hosts());
        let (id, _events) = open(&manager, target(&port, parameters)).await;
        let held = settings(&device);
        let speed = parameters.baud_rate;
        assert_eq!(speeds(&device), (speed, speed), "{parameters:?}");
        assert_eq!(held.c_cflag & libc::CSTOPB != 0, two_stop, "{parameters:?}");
        let software = held.c_iflag & (libc::IXON | libc::IXOFF) != 0;
        assert_eq!(
            software,
            parameters.flow_control == FlowControl::Software,
            "{parameters:?}"
        );
        manager.close(id).unwrap();
    }
}

#[tokio::test]
async fn send_break_is_accepted_and_the_input_after_it_follows_in_order() {
    let (device, port) = pseudo_terminal();
    let manager = SessionManager::new(known_hosts());
    let (id, mut events) = open(&manager, target(&port, SerialParameters::default())).await;

    let started = Instant::now();
    manager.write(id, b"boot\r".to_vec()).unwrap();
    manager.send_break(id).unwrap();
    manager.write(id, b"confreg 0x2142\r".to_vec()).unwrap();
    assert_eq!(
        received_ending_with(&device, b"0x2142\r").await,
        b"boot\rconfreg 0x2142\r"
    );
    // The break was held before the input after it went out.
    assert!(
        started.elapsed() >= enodix_core::session::BREAK_DURATION,
        "{:?}",
        started.elapsed()
    );
    // The session goes on.
    (&device).write_all(b"rommon 1 >").unwrap();
    output_ending_with(&mut events, b"rommon 1 >").await;
}

#[tokio::test]
async fn a_port_in_use_is_refused_and_free_again_once_the_session_closes() {
    let (device, port) = pseudo_terminal();
    let manager = SessionManager::new(known_hosts());
    let (first, mut first_events) =
        open(&manager, target(&port, SerialParameters::default())).await;

    let (_second, mut second_events) = manager.open(
        SessionTarget::Serial(target(&port, SerialParameters::default())),
        SIZE,
    );
    assert_eq!(
        next_state(&mut second_events).await,
        SessionState::Connecting
    );
    let reason = closed_reason(&mut second_events).await;
    assert!(
        reason.to_string().contains(&port),
        "the refusal names the port: {reason}"
    );

    manager.close(first).unwrap();
    closed_reason(&mut first_events).await;
    let (again, mut events) = open(&manager, target(&port, SerialParameters::default())).await;
    (&device).write_all(b"Switch>").unwrap();
    output_ending_with(&mut events, b"Switch>").await;
    manager.close(again).unwrap();
}

#[tokio::test]
async fn the_other_end_going_away_ends_the_session_with_a_clear_message() {
    let (device, port) = pseudo_terminal();
    let manager = SessionManager::new(known_hosts());
    let (_id, mut events) = open(&manager, target(&port, SerialParameters::default())).await;

    // The nearest a pseudo terminal has to an unplugged adapter.
    drop(device);
    let reason = closed_reason(&mut events).await;
    assert!(
        matches!(&reason, CloseReason::PortLost { port: lost, .. } if *lost == port),
        "{reason:?}"
    );
}
