//! Serial console sessions over a virtual serial port pair
//! (`support::serial`): the port list, data both ways, line parameters
//! (defaults, set on a host, inherited from a folder), break, unplugging
//! the adapter, a port in use, flow control and reconnecting, all observed
//! as the bytes, states and line events a user (or the device) would see.
//!
//! The virtual pair replaces only the `serialport` crate's driver calls;
//! `system_*` tests below use the real system ports where that is safe
//! (listing, and opening a port that does not exist).

mod support;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use netterm_core::credentials::{HostLogin, MemoryStore, ProfileError, Profiles};
use netterm_core::session::{
    CloseReason, OpenError, OutputOptions, SerialParameters, SerialPorts, SerialTarget,
    SessionError, SessionEvents, SessionId, SessionManager, SessionState, SessionTarget,
    SystemSerialPorts, TerminalSize, Transport, BREAK_DURATION,
};
use netterm_core::workspace::{FlowControl, HostId, Parity, Protocol, Workspace};
use support::serial::{VirtualDevice, VirtualSerialPorts};
use support::{known_hosts, next_state, output_ending_with, unacknowledged_output};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

fn manager(ports: &VirtualSerialPorts) -> SessionManager {
    manager_with_output_options(ports, OutputOptions::default())
}

fn manager_with_output_options(
    ports: &VirtualSerialPorts,
    options: OutputOptions,
) -> SessionManager {
    SessionManager::with_serial_ports(known_hosts(), options, Arc::new(ports.clone()))
}

fn target(port: &str) -> SerialTarget {
    SerialTarget {
        port: port.into(),
        parameters: SerialParameters::default(),
    }
}

async fn open(manager: &SessionManager, target: SerialTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Serial(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

/// Opens a session on `COM3` and waits for the device's prompt.
async fn console() -> (
    VirtualSerialPorts,
    VirtualDevice,
    SessionManager,
    SessionId,
    SessionEvents,
) {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);
    let (id, mut events) = open(&manager, target("COM3")).await;
    device.send(b"\r\nSwitch>").unwrap();
    output_ending_with(&mut events, b"Switch>").await;
    (ports, device, manager, id, events)
}

async fn closed_reason(events: &mut SessionEvents) -> CloseReason {
    match next_state(events).await {
        SessionState::Closed(reason) => reason,
        other => panic!("expected the session to close, got {other:?}"),
    }
}

#[tokio::test]
async fn detected_ports_are_listed_with_their_adapters_in_port_order() {
    let ports = VirtualSerialPorts::new();
    ports.add("COM10", "Prolific USB-to-Serial Comm Port");
    ports.add("COM3", "USB Serial Port");
    ports.add("COM9", "Silicon Labs CP210x USB to UART Bridge");
    let manager = manager(&ports);

    let listed: Vec<(String, String)> = manager
        .serial_ports()
        .unwrap()
        .into_iter()
        .map(|port| (port.name, port.description))
        .collect();
    assert_eq!(
        listed,
        [
            ("COM3".to_string(), "USB Serial Port".to_string()),
            (
                "COM9".to_string(),
                "Silicon Labs CP210x USB to UART Bridge".to_string()
            ),
            (
                "COM10".to_string(),
                "Prolific USB-to-Serial Comm Port".to_string()
            ),
        ]
    );

    // Listing again finds what was plugged in meanwhile.
    ports.add("COM4", "FTDI FT232R");
    let names: Vec<String> = manager
        .serial_ports()
        .unwrap()
        .into_iter()
        .map(|port| port.name)
        .collect();
    assert_eq!(names, ["COM3", "COM4", "COM9", "COM10"]);
}

#[tokio::test]
async fn console_output_and_typed_input_cross_the_line_unchanged() {
    let (_ports, device, manager, id, mut events) = console().await;
    assert_eq!(manager.transport(id), Ok(Transport::Serial));

    // Typed as is: Enter is a bare CR, and nothing is echoed locally (the
    // device echoes).
    manager.write(id, b"show version\r".to_vec()).unwrap();
    assert_eq!(device.received_ending_with(b"\r").await, b"show version\r");

    // Every byte value arrives as sent, both ways.
    let all: Vec<u8> = (0..=255).collect();
    device.send(&all).unwrap();
    assert_eq!(output_ending_with(&mut events, &[254, 255]).await, all);
    manager.write(id, all.clone()).unwrap();
    assert_eq!(device.received_ending_with(&[254, 255]).await, all);
}

#[tokio::test]
async fn a_port_is_read_on_its_own_dedicated_thread() {
    let (_ports, device, _manager, _id, _events) = console().await;
    assert_eq!(device.reader_threads(), ["serial reader COM3"]);
}

#[tokio::test]
async fn the_line_defaults_to_9600_8n1_without_flow_control() {
    let defaults = SerialParameters::default();
    assert_eq!(
        defaults,
        SerialParameters {
            baud_rate: 9600,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            flow_control: FlowControl::None,
        }
    );
    let (_ports, device, ..) = console().await;
    assert_eq!(device.opened(), [defaults]);
}

#[tokio::test]
async fn the_line_is_opened_with_the_parameters_given() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM5", "USB Serial Port");
    let manager = manager(&ports);
    let parameters = SerialParameters {
        baud_rate: 115_200,
        data_bits: 7,
        parity: Parity::Even,
        stop_bits: 2,
        flow_control: FlowControl::Hardware,
    };

    let (_id, _events) = open(
        &manager,
        SerialTarget {
            port: "COM5".into(),
            parameters,
        },
    )
    .await;
    assert_eq!(device.opened(), [parameters]);
}

#[tokio::test]
async fn parameters_no_port_can_use_are_refused_before_opening() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);

    for (parameters, complaint) in [
        (
            SerialParameters {
                data_bits: 9,
                ..SerialParameters::default()
            },
            "data bits",
        ),
        (
            SerialParameters {
                stop_bits: 3,
                ..SerialParameters::default()
            },
            "stop bits",
        ),
        (
            SerialParameters {
                baud_rate: 0,
                ..SerialParameters::default()
            },
            "baud rate",
        ),
    ] {
        let (_id, mut events) = manager.open(
            SessionTarget::Serial(SerialTarget {
                port: "COM3".into(),
                parameters,
            }),
            SIZE,
        );
        assert_eq!(next_state(&mut events).await, SessionState::Connecting);
        let reason = closed_reason(&mut events).await;
        assert!(
            matches!(&reason, CloseReason::Failed(OpenError::SerialPort { port, .. }) if port == "COM3"),
            "{reason:?}"
        );
        assert!(reason.to_string().contains(complaint), "{reason}");
    }
    assert!(device.opened().is_empty(), "the port was never opened");
}

/// A workspace whose folder sets serial parameters and whose host sets
/// some of its own.
fn console_workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("Lab")).unwrap();
    fs::write(
        root.join("Lab/_folder.yaml"),
        "serial:\n  baud_rate: 115200\n  flow_control: software\n",
    )
    .unwrap();
    fs::write(
        root.join("Lab/console-sw1.yaml"),
        "address: 192.0.2.10\nprotocol: serial\nserial:\n  port: COM7\n  data_bits: 7\n  parity: odd\n",
    )
    .unwrap();
    fs::write(
        root.join("Lab/no-port.yaml"),
        "address: 192.0.2.11\nprotocol: serial\n",
    )
    .unwrap();
    fs::write(root.join("Lab/ssh-r1.yaml"), "address: 192.0.2.12\n").unwrap();
    let workspace = Workspace::open(root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[tokio::test]
async fn a_host_opens_with_its_own_and_inherited_serial_settings() {
    let (_temp, workspace) = console_workspace();
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let host = HostId::new("Lab/console-sw1.yaml");

    // A serial host asks nothing before connecting: the device asks for any
    // login in the terminal.
    assert_eq!(profiles.login(&host), Ok(HostLogin::Serial));
    assert_eq!(
        profiles.login_with(&HostId::new("Lab/ssh-r1.yaml"), Protocol::Serial),
        Ok(HostLogin::Serial)
    );

    let target = profiles.serial_target(&host).unwrap();
    let expected = SerialParameters {
        baud_rate: 115_200,
        data_bits: 7,
        parity: Parity::Odd,
        stop_bits: 1,
        flow_control: FlowControl::Software,
    };
    assert_eq!(
        target,
        SerialTarget {
            port: "COM7".into(),
            parameters: expected,
        }
    );

    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM7", "USB Serial Port");
    let manager = manager(&ports);
    let (id, mut events) = open(&manager, target).await;
    assert_eq!(device.opened(), [expected]);
    device.send(b"Password: ").unwrap();
    output_ending_with(&mut events, b"Password: ").await;
    manager.write(id, b"secret\r".to_vec()).unwrap();
    assert_eq!(device.received_ending_with(b"\r").await, b"secret\r");
}

#[tokio::test]
async fn a_host_without_a_serial_port_cannot_be_opened_by_serial() {
    let (_temp, workspace) = console_workspace();
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);

    let host = HostId::new("Lab/no-port.yaml");
    let error = profiles.serial_target(&host).unwrap_err();
    assert_eq!(error, ProfileError::NoSerialPort(host));
    assert!(error.to_string().contains("serial port"), "{error}");
}

#[tokio::test]
async fn a_break_is_held_for_the_break_duration_after_the_input_before_it() {
    let (_ports, device, manager, id, _events) = console().await;

    manager.write(id, b"boot\r".to_vec()).unwrap();
    manager.send_break(id).unwrap();
    manager.write(id, b"confreg 0x2142\r".to_vec()).unwrap();

    assert_eq!(
        device.received_ending_with(b"0x2142\r").await,
        b"boot\rconfreg 0x2142\r"
    );
    let breaks = device.breaks();
    assert_eq!(breaks.len(), 1, "{breaks:?}");
    assert_eq!(breaks[0].after_bytes, b"boot\r".len());
    assert!(
        breaks[0].held >= BREAK_DURATION,
        "held for {:?}",
        breaks[0].held
    );
}

#[tokio::test]
async fn only_serial_sessions_send_a_break() {
    let server = support::telnet::TestTelnetServer::start().await;
    let ports = VirtualSerialPorts::new();
    let manager = manager(&ports);
    let (id, mut events) = manager.open(SessionTarget::Telnet(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    assert_eq!(manager.transport(id), Ok(Transport::Telnet));

    assert_eq!(manager.send_break(id), Err(SessionError::BreakUnsupported));
    assert_eq!(
        manager.send_break(SessionId(9999)),
        Err(SessionError::NotFound)
    );
}

#[tokio::test]
async fn unplugging_the_adapter_ends_the_session_with_a_clear_message() {
    let (ports, device, manager, id, mut events) = console().await;

    device.unplug();
    let reason = tokio::time::timeout(Duration::from_secs(5), closed_reason(&mut events))
        .await
        .expect("the session did not notice the unplugged adapter");
    assert!(
        matches!(&reason, CloseReason::PortLost { port, .. } if port == "COM3"),
        "{reason:?}"
    );
    let message = reason.to_string();
    assert!(message.contains("COM3"), "{message}");
    assert!(message.contains("unplugged"), "{message}");

    // The session let go of the port, and the manager is still usable.
    device.wait_until_closed().await;
    assert_eq!(
        manager.write(id, b"x".to_vec()),
        Err(SessionError::NotFound)
    );
    assert!(ports.list().unwrap().is_empty(), "the port is gone");
}

#[tokio::test]
async fn unplugging_while_output_is_paused_still_ends_the_session() {
    let ports = VirtualSerialPorts::new();
    let device = Arc::new(ports.add("COM3", "USB Serial Port"));
    let manager = manager_with_output_options(
        &ports,
        OutputOptions {
            window: 4096,
            ..OutputOptions::default()
        },
    );
    let (_id, mut events) = open(&manager, target("COM3")).await;

    // Fill the window and never acknowledge: the session stops reading.
    let sender = device.clone();
    let flood = std::thread::spawn(move || while sender.send(&[b'x'; 1024]).is_ok() {});
    let first = unacknowledged_output(&mut events, Duration::from_millis(300)).await;
    assert!(!first.is_empty());

    device.unplug();
    let reason = loop {
        // Acknowledge nothing: the loss must be noticed anyway.
        match tokio::time::timeout(Duration::from_secs(5), events.next())
            .await
            .expect("the session did not notice the unplugged adapter")
        {
            Some(netterm_core::session::SessionEvent::State(SessionState::Closed(reason))) => {
                break reason
            }
            Some(_) => {}
            None => panic!("events ended without a closed state"),
        }
    };
    assert!(matches!(reason, CloseReason::PortLost { .. }), "{reason:?}");
    // The device's writes fail once the session has let go of the port.
    tokio::task::spawn_blocking(move || flood.join().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn reading_pauses_while_output_is_unacknowledged_and_input_still_goes_out() {
    let window = 8 * 1024;
    let ports = VirtualSerialPorts::new();
    let device = Arc::new(ports.add("COM3", "USB Serial Port"));
    let manager = manager_with_output_options(
        &ports,
        OutputOptions {
            window,
            ..OutputOptions::default()
        },
    );
    let (id, mut events) = open(&manager, target("COM3")).await;

    let total = 256 * 1024;
    let sender = device.clone();
    let flood = std::thread::spawn(move || {
        let chunk = vec![b'y'; 1024];
        for _ in 0..total / chunk.len() {
            sender.send(&chunk).unwrap();
        }
    });

    let paused = unacknowledged_output(&mut events, Duration::from_millis(400)).await;
    assert!(
        !paused.is_empty() && paused.len() < window + 16 * 1024,
        "read {} bytes without acknowledgement (window {window})",
        paused.len()
    );

    // Typing still reaches the device while reading is paused.
    manager.write(id, b"\x03".to_vec()).unwrap();
    assert_eq!(device.received_ending_with(b"\x03").await, b"\x03");

    // Acknowledging lets the rest through.
    events.acknowledge(paused.len());
    let mut received = paused.len();
    while received < total {
        match tokio::time::timeout(support::WAIT, events.next()).await {
            Ok(Some(netterm_core::session::SessionEvent::Output(bytes))) => {
                received += bytes.len();
                events.acknowledge(bytes.len());
            }
            other => panic!("expected output, got {other:?} after {received} bytes"),
        }
    }
    assert_eq!(received, total);
    flood.join().unwrap();
}

#[tokio::test]
async fn a_port_in_use_is_refused_and_free_again_once_the_session_closes() {
    let (_ports, device, manager, id, mut events) = console().await;

    // A second session on the same port, as duplicating would open.
    let (copy, mut copy_events) = manager.duplicate(id, SIZE).unwrap();
    assert_eq!(next_state(&mut copy_events).await, SessionState::Connecting);
    let reason = closed_reason(&mut copy_events).await;
    assert!(
        matches!(&reason, CloseReason::Failed(OpenError::SerialPort { port, .. }) if port == "COM3"),
        "{reason:?}"
    );
    assert!(reason.to_string().contains("Access is denied"), "{reason}");
    manager.discard(copy);

    // Typed input still queued goes out before the port closes, and the
    // port is free by the time the session says it closed.
    manager.write(id, b"exit\r".to_vec()).unwrap();
    manager.close(id).unwrap();
    assert_eq!(closed_reason(&mut events).await, CloseReason::ClosedLocally);
    assert!(!device.is_open(), "the port is still held");
    assert_eq!(device.received_ending_with(b"exit\r").await, b"exit\r");

    let mut events = manager.reconnect(id, SIZE).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    device.send(b"Switch>").unwrap();
    output_ending_with(&mut events, b"Switch>").await;
    assert_eq!(device.opened().len(), 2);
}

#[tokio::test]
async fn blanks_around_a_typed_port_name_are_ignored_and_no_name_is_refused() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = manager(&ports);
    let (_id, _events) = open(&manager, target("  COM3 ")).await;
    assert_eq!(device.opened().len(), 1);

    let (_id, mut events) = manager.open(SessionTarget::Serial(target(" ")), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let reason = closed_reason(&mut events).await;
    assert!(reason.to_string().contains("no port was given"), "{reason}");
}

#[tokio::test]
async fn a_port_that_does_not_exist_fails_with_its_name() {
    let ports = VirtualSerialPorts::new();
    let manager = manager(&ports);
    let (_id, mut events) = manager.open(SessionTarget::Serial(target("COM8")), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let reason = closed_reason(&mut events).await;
    assert!(
        reason
            .to_string()
            .starts_with("cannot open the serial port COM8"),
        "{reason}"
    );
}

#[test]
fn system_ports_can_be_listed() {
    // Whatever this machine has; listing must not fail.
    let ports = SystemSerialPorts.list().unwrap();
    for port in ports {
        assert!(!port.name.is_empty());
    }
}

#[tokio::test]
async fn system_a_high_numbered_port_that_does_not_exist_fails_clearly() {
    // Opens through the real driver API (COM ports above 9 need the
    // `\\.\` prefix on Windows); no machine has this many ports.
    let manager = SessionManager::new(known_hosts());
    let (_id, mut events) = manager.open(SessionTarget::Serial(target("COM250")), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let reason = closed_reason(&mut events).await;
    assert!(
        matches!(&reason, CloseReason::Failed(OpenError::SerialPort { port, .. }) if port == "COM250"),
        "{reason:?}"
    );
}

#[tokio::test]
async fn system_mark_and_space_parity_are_refused_with_a_reason() {
    let target = SerialTarget {
        port: "COM250".into(),
        parameters: SerialParameters {
            parity: Parity::Mark,
            ..SerialParameters::default()
        },
    };
    let error = SystemSerialPorts
        .open(&target.port, &target.parameters)
        .err()
        .expect("mark parity was accepted");
    assert!(error.to_string().contains("mark"), "{error}");
}
