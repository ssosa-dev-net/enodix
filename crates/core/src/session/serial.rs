//! The serial transport: a console port on a local serial line, such as a
//! USB serial adapter's COM port.
//!
//! The port is driven through the `serialport` crate's blocking interface,
//! not an async one (the async serial crates have known Windows issues).
//! Each open port has a dedicated reader thread that feeds the async
//! runtime: it waits until flow control has room ([`Flow::room_to_read`]),
//! reads with a short timeout ([`SERIAL_POLL`]) and passes what came on. A
//! second thread writes typed input and sends breaks, in order, so a line
//! held back by flow control never stops reading, nor the session taking
//! commands.
//!
//! The bytes go through unchanged both ways: the device echoes, and Enter
//! is a bare CR as consoles expect. A serial line has no window size.
//!
//! [`SerialPorts`] is the seam to the system: [`SystemSerialPorts`] is the
//! real one; tests put a virtual port pair behind it.

use std::future::Future;
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::output::Flow;
use super::{CloseReason, Command, Link, OpenError, SessionState};
use crate::workspace::{
    FlowControl, Parity, DEFAULT_BAUD_RATE, DEFAULT_DATA_BITS, DEFAULT_STOP_BITS,
};

/// How long a [`SerialLine`] read or write waits at most, so that the
/// threads driving a port notice in time that the session has ended.
pub const SERIAL_POLL: Duration = Duration::from_millis(100);

/// How long a break holds the line: long enough for devices that look for
/// one while booting (Cisco ROMMON, Juniper) to see it.
pub const BREAK_DURATION: Duration = Duration::from_millis(500);

/// How much is read from the port at a time.
const READ_SIZE: usize = 4096;

/// How long closing waits for typed input still queued to go out.
const CLOSE_FLUSH: Duration = Duration::from_secs(2);

/// A serial port and the line parameters to open it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialTarget {
    /// The port as the system names it: `COM3`, `/dev/ttyUSB0`.
    pub port: String,
    pub parameters: SerialParameters,
}

/// Serial line parameters. The default is 9600 8N1 without flow control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialParameters {
    pub baud_rate: u32,
    /// 5 to 8.
    pub data_bits: u8,
    pub parity: Parity,
    /// 1 or 2.
    pub stop_bits: u8,
    pub flow_control: FlowControl,
}

impl Default for SerialParameters {
    fn default() -> Self {
        Self {
            baud_rate: DEFAULT_BAUD_RATE,
            data_bits: DEFAULT_DATA_BITS,
            parity: Parity::None,
            stop_bits: DEFAULT_STOP_BITS,
            flow_control: FlowControl::None,
        }
    }
}

impl SerialParameters {
    /// What is wrong with these parameters for any port, if anything.
    fn problem(&self) -> Option<&'static str> {
        if self.baud_rate == 0 {
            Some("the baud rate must be a positive whole number")
        } else if !(5..=8).contains(&self.data_bits) {
            Some("data bits must be a number from 5 to 8")
        } else if !(1..=2).contains(&self.stop_bits) {
            Some("stop bits must be 1 or 2")
        } else {
            None
        }
    }
}

/// A serial port the system has, as the port list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialPortInfo {
    /// What to open it by: `COM3`, `/dev/ttyUSB0`.
    pub name: String,
    /// The adapter, as its driver names it (`Prolific USB-to-Serial Comm
    /// Port`); empty if nothing says.
    pub description: String,
    pub kind: SerialPortKind,
}

/// How a serial port is attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SerialPortKind {
    #[serde(rename_all = "camelCase")]
    Usb {
        vendor_id: u16,
        product_id: u16,
        manufacturer: Option<String>,
        serial_number: Option<String>,
    },
    Pci,
    Bluetooth,
    Unknown,
}

/// The system's serial ports: what there is, and opening one.
pub trait SerialPorts: Send + Sync {
    /// The ports there are now, in any order.
    fn list(&self) -> io::Result<Vec<SerialPortInfo>>;

    /// Opens the port called `port` with `parameters`. A port that does not
    /// exist (or is in use) is an error saying so.
    fn open(&self, port: &str, parameters: &SerialParameters) -> io::Result<Box<dyn SerialLine>>;
}

/// An open serial port. Reads and writes wait at most about
/// [`SERIAL_POLL`]; an error from any call means the port no longer works
/// (the adapter was unplugged, say).
pub trait SerialLine: Send {
    /// Reads what has arrived, waiting at most about [`SERIAL_POLL`] for
    /// something: `Ok(0)` if nothing came.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;

    /// Writes some of `bytes`, waiting at most about [`SERIAL_POLL`]: how
    /// many went out, `Ok(0)` if the line took none (flow control holds it
    /// back).
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize>;

    /// Puts the line in the break state (`on`) or takes it out.
    fn set_break(&mut self, on: bool) -> io::Result<()>;

    /// Whether the port still works, without reading from it.
    fn check(&mut self) -> io::Result<()>;

    /// Another handle on the same open port, for writing on one thread
    /// while another reads.
    fn try_clone(&self) -> io::Result<Box<dyn SerialLine>>;
}

/// The serial ports of this computer, through the `serialport` crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSerialPorts;

impl SerialPorts for SystemSerialPorts {
    fn list(&self) -> io::Result<Vec<SerialPortInfo>> {
        Ok(serialport::available_ports()?
            .into_iter()
            .map(port_info)
            .collect())
    }

    fn open(&self, port: &str, parameters: &SerialParameters) -> io::Result<Box<dyn SerialLine>> {
        let unsupported = |what: &str| io::Error::new(io::ErrorKind::Unsupported, what.to_string());
        let parity = match parameters.parity {
            Parity::None => serialport::Parity::None,
            Parity::Odd => serialport::Parity::Odd,
            Parity::Even => serialport::Parity::Even,
            Parity::Mark | Parity::Space => {
                return Err(unsupported(
                    "mark and space parity are not supported; use none, odd or even",
                ))
            }
        };
        let data_bits = match parameters.data_bits {
            5 => serialport::DataBits::Five,
            6 => serialport::DataBits::Six,
            7 => serialport::DataBits::Seven,
            8 => serialport::DataBits::Eight,
            _ => return Err(unsupported("data bits must be a number from 5 to 8")),
        };
        let stop_bits = match parameters.stop_bits {
            1 => serialport::StopBits::One,
            2 => serialport::StopBits::Two,
            _ => return Err(unsupported("stop bits must be 1 or 2")),
        };
        let flow_control = match parameters.flow_control {
            FlowControl::None => serialport::FlowControl::None,
            FlowControl::Hardware => serialport::FlowControl::Hardware,
            FlowControl::Software => serialport::FlowControl::Software,
        };
        let mut opened = serialport::new(port, parameters.baud_rate)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(flow_control)
            .timeout(SERIAL_POLL)
            // Like terminal programs do: some console cables and devices
            // only talk while DTR (and RTS) are up.
            .dtr_on_open(true)
            .open()?;
        if parameters.flow_control != FlowControl::Hardware {
            opened.write_request_to_send(true)?;
        }
        Ok(Box::new(SystemLine(opened)))
    }
}

fn port_info(port: serialport::SerialPortInfo) -> SerialPortInfo {
    let (description, kind) = match port.port_type {
        serialport::SerialPortType::UsbPort(usb) => (
            usb.product
                .clone()
                .or_else(|| usb.manufacturer.clone())
                .unwrap_or_default(),
            SerialPortKind::Usb {
                vendor_id: usb.vid,
                product_id: usb.pid,
                manufacturer: usb.manufacturer,
                serial_number: usb.serial_number,
            },
        ),
        serialport::SerialPortType::PciPort => (String::new(), SerialPortKind::Pci),
        serialport::SerialPortType::BluetoothPort => {
            ("Bluetooth".to_string(), SerialPortKind::Bluetooth)
        }
        serialport::SerialPortType::Unknown => (String::new(), SerialPortKind::Unknown),
    };
    SerialPortInfo {
        // The name without anything the driver appends, e.g. "(COM3)".
        description: description
            .trim()
            .trim_end_matches(&format!("({})", port.port_name))
            .trim()
            .to_string(),
        name: port.port_name,
        kind,
    }
}

/// A port opened through the `serialport` crate.
struct SystemLine(Box<dyn serialport::SerialPort>);

impl SerialLine for SystemLine {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the port was closed",
            )),
            Ok(count) => Ok(count),
            // Nothing arrived. Some drivers keep timing out on a removed
            // adapter instead of failing: ask the port whether it is there.
            Err(error) if is_timeout(&error) => self.check().map(|()| 0),
            Err(error) => Err(error),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.0.write(bytes) {
            Err(error) if is_timeout(&error) => self.check().map(|()| 0),
            other => other,
        }
    }

    fn set_break(&mut self, on: bool) -> io::Result<()> {
        if on {
            self.0.set_break()?;
        } else {
            self.0.clear_break()?;
        }
        Ok(())
    }

    fn check(&mut self) -> io::Result<()> {
        self.0.bytes_to_read()?;
        Ok(())
    }

    fn try_clone(&self) -> io::Result<Box<dyn SerialLine>> {
        Ok(Box::new(SystemLine(self.0.try_clone()?)))
    }
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

/// The detected ports in the order people number them: `COM3` before
/// `COM10`.
pub(super) fn sorted(mut ports: Vec<SerialPortInfo>) -> Vec<SerialPortInfo> {
    ports.sort_by_cached_key(|port| natural_key(&port.name));
    ports
}

/// `name` split into text and numbers, so numbers compare by value.
fn natural_key(name: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut text = String::new();
    let mut digits = String::new();
    for char in name.chars() {
        if char.is_ascii_digit() {
            digits.push(char);
        } else {
            if !digits.is_empty() {
                key.push((
                    std::mem::take(&mut text),
                    digits.parse().unwrap_or(u64::MAX),
                ));
                digits.clear();
            }
            text.extend(char.to_lowercase());
        }
    }
    key.push((text, digits.parse().unwrap_or(0)));
    key
}

/// What the writer thread does, in order.
enum Job {
    Write(Vec<u8>),
    Break,
}

/// Runs one serial session over `link` until it ends.
pub(super) async fn run(
    target: SerialTarget,
    ports: Arc<dyn SerialPorts>,
    link: &mut Link,
) -> CloseReason {
    // As typed or saved: surrounding blanks are not part of a port's name.
    let port = target.port.trim().to_string();
    let failed = |reason: String| {
        CloseReason::Failed(OpenError::SerialPort {
            port: port.clone(),
            reason,
        })
    };
    if port.is_empty() {
        return failed("no port was given".into());
    }
    if let Some(problem) = target.parameters.problem() {
        return failed(problem.into());
    }

    // Opening can take a while in a driver: off the runtime's threads.
    let (_prompter, _asker, replies) = link.askers();
    let name = port.clone();
    let parameters = target.parameters;
    let mut opening = tokio::task::spawn_blocking(move || {
        let reader = ports.open(&name, &parameters)?;
        let writer = reader.try_clone()?;
        Ok::<_, io::Error>((reader, writer))
    });
    let (reader, writer) = tokio::select! {
        opened = &mut opening => match opened {
            Ok(Ok(lines)) => lines,
            Ok(Err(error)) => return failed(open_failure(&error)),
            Err(error) => return failed(error.to_string()),
        },
        () = link.close_requested(&replies) => {
            // Let the open finish and close what it opened, so the port is
            // free once the session says it closed.
            let _ = tokio::time::timeout(CLOSE_FLUSH, opening).await;
            return CloseReason::ClosedLocally;
        }
    };

    // Both threads hold `finished` until they end: once it reports closed,
    // the port is closed too.
    let (finished_tx, mut finished) = mpsc::channel::<()>(1);
    let (output_tx, mut output) = mpsc::unbounded_channel();
    let flow = link.flow();
    let reading = finished_tx.clone();
    let spawned = thread::Builder::new()
        .name(format!("serial reader {port}"))
        .spawn(move || read_port(reader, &flow, &output_tx, reading));
    if let Err(error) = spawned {
        return failed(format!("cannot start reading it: {error}"));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (jobs, queued) = std_mpsc::channel();
    let (write_failed_tx, mut write_failed) = mpsc::unbounded_channel();
    let stopping = stop.clone();
    let spawned = thread::Builder::new()
        .name(format!("serial writer {port}"))
        .spawn(move || write_port(writer, &queued, &write_failed_tx, &stopping, finished_tx));
    if let Err(error) = spawned {
        // Dropping `output` stops the reader; wait until it let go.
        drop(output);
        let _ = tokio::time::timeout(CLOSE_FLUSH, finished.recv()).await;
        return failed(format!("cannot start writing to it: {error}"));
    }

    link.state(SessionState::Connected);
    let lost = |error: io::Error| CloseReason::PortLost {
        port: port.clone(),
        reason: error.to_string(),
    };
    let reason = loop {
        tokio::select! {
            command = link.command() => match command {
                Command::Write(bytes) => {
                    let _ = jobs.send(Job::Write(bytes));
                }
                Command::Break => {
                    let _ = jobs.send(Job::Break);
                }
                // A serial line has no window size, and asks nothing.
                Command::Resize(_) | Command::Answer(_) | Command::HostKey(_) => {}
                Command::Close => break CloseReason::ClosedLocally,
            },
            read = output.recv() => match read {
                Some(Ok(bytes)) => {
                    if !link.forward_counted(bytes) {
                        break CloseReason::ClosedLocally;
                    }
                }
                Some(Err(error)) => break lost(error),
                None => break lost(io::Error::other("reading it stopped")),
            },
            Some(error) = write_failed.recv() => break lost(error),
        }
    };

    // Closing from this side lets typed input still queued go out first;
    // otherwise the port is gone already. Either way the session reports
    // closed only once the port is closed, so it can be opened again.
    drop(jobs);
    drop(output);
    if reason != CloseReason::ClosedLocally {
        stop.store(true, Ordering::Relaxed);
    }
    if tokio::time::timeout(CLOSE_FLUSH, finished.recv())
        .await
        .is_err()
    {
        stop.store(true, Ordering::Relaxed);
        let _ = tokio::time::timeout(CLOSE_FLUSH, finished.recv()).await;
    }
    reason
}

/// Why a port could not be opened, with a hint for the usual causes.
fn open_failure(error: &io::Error) -> String {
    let hint = match error.kind() {
        // Windows says "access denied" for a port another program holds,
        // which the crate reports as no device.
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
            " (is the adapter plugged in, and the port not in use by another program?)"
        }
        _ => "",
    };
    format!("{error}{hint}")
}

/// The reader thread: reads the port whenever flow control has room and
/// passes what came on (counted in `flow` at once, so the window is exact),
/// until the session stops listening or the port fails.
fn read_port(
    mut line: Box<dyn SerialLine>,
    flow: &Flow,
    output: &mpsc::UnboundedSender<io::Result<Vec<u8>>>,
    _finished: mpsc::Sender<()>,
) {
    let mut buffer = vec![0; READ_SIZE];
    loop {
        let room = wait_on_thread(
            async {
                tokio::select! {
                    () = flow.room_to_read() => true,
                    () = output.closed() => false,
                }
            },
            SERIAL_POLL,
        );
        let result = match room {
            Some(false) => return,
            // While paused, still notice an unplugged adapter.
            None => line.check().map(|()| 0),
            Some(true) => line.read(&mut buffer),
        };
        match result {
            Ok(0) => {}
            Ok(count) => {
                flow.read(count);
                if output.send(Ok(buffer[..count].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = output.send(Err(error));
                return;
            }
        }
    }
}

/// Waits on this (non-runtime) thread for `future`, at most `limit`:
/// `None` if it was not ready by then. Needs no runtime: what it waits on
/// (flow control, a channel closing) wakes the thread itself.
fn wait_on_thread<T>(future: impl Future<Output = T>, limit: Duration) -> Option<T> {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    let deadline = Instant::now() + limit;
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return Some(value);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        thread::park_timeout(left);
    }
}

/// The writer thread: writes typed input and sends breaks in order, until
/// the queue closes or `stop` is set; reports a failing port on `failed`.
fn write_port(
    mut line: Box<dyn SerialLine>,
    queued: &std_mpsc::Receiver<Job>,
    failed: &mpsc::UnboundedSender<io::Error>,
    stop: &AtomicBool,
    _finished: mpsc::Sender<()>,
) {
    while let Ok(job) = queued.recv() {
        let done = match job {
            Job::Write(bytes) => write_all(line.as_mut(), &bytes, stop),
            Job::Break => send_break(line.as_mut()),
        };
        if let Err(error) = done {
            let _ = failed.send(error);
            return;
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

fn write_all(line: &mut dyn SerialLine, mut bytes: &[u8], stop: &AtomicBool) -> io::Result<()> {
    while !bytes.is_empty() && !stop.load(Ordering::Relaxed) {
        let written = line.write(bytes)?;
        bytes = &bytes[written..];
    }
    Ok(())
}

fn send_break(line: &mut dyn SerialLine) -> io::Result<()> {
    line.set_break(true)?;
    thread::sleep(BREAK_DURATION);
    line.set_break(false)
}
