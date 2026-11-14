//! A virtual serial port pair, for serial session tests on machines without
//! a virtual serial driver (com0com and the like need admin rights).
//!
//! [`VirtualSerialPorts`] stands in for the system's serial ports behind the
//! core's [`SerialPorts`] seam: the session code above it (the reader
//! thread, the writer, flow control, break, unplug handling) is the real
//! one; only the `serialport` crate's driver calls are replaced. Each open
//! port is one end of a loopback TCP connection; the test holds the other
//! end as the device ([`VirtualDevice`]), so bytes cross a real OS socket
//! with blocking reads and writes that time out like a COM port's.
//!
//! Like a real port it can be opened once at a time (a second open is
//! refused as Windows refuses it: "Access is denied"), it records the line
//! parameters it was opened with and every break, and it can be unplugged:
//! reads and writes then fail, and it cannot be opened again.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use netterm_core::session::{
    SerialLine, SerialParameters, SerialPortInfo, SerialPortKind, SerialPorts, SERIAL_POLL,
};

use super::WAIT;

/// The system's serial ports, made up: add ports with [`Self::add`].
#[derive(Clone, Default)]
pub struct VirtualSerialPorts {
    ports: Arc<Mutex<BTreeMap<String, Arc<Port>>>>,
}

impl VirtualSerialPorts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plugs in a port called `name` (`COM3`) whose adapter is described as
    /// `description`, and returns the device at its other end.
    pub fn add(&self, name: &str, description: &str) -> VirtualDevice {
        let port = Arc::new(Port {
            info: SerialPortInfo {
                name: name.into(),
                description: description.into(),
                kind: SerialPortKind::Usb {
                    vendor_id: 0x0403,
                    product_id: 0x6001,
                    manufacturer: Some("FTDI".into()),
                    serial_number: Some(format!("VIRTUAL-{name}")),
                },
            },
            state: Mutex::default(),
        });
        lock(&self.ports).insert(name.into(), port.clone());
        VirtualDevice { port }
    }
}

impl SerialPorts for VirtualSerialPorts {
    fn list(&self) -> io::Result<Vec<SerialPortInfo>> {
        Ok(lock(&self.ports)
            .values()
            .filter(|port| !port.state().unplugged)
            .map(|port| port.info.clone())
            .collect())
    }

    fn open(&self, name: &str, parameters: &SerialParameters) -> io::Result<Box<dyn SerialLine>> {
        let not_found = || {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The system cannot find the file specified.",
            )
        };
        let port = lock(&self.ports).get(name).cloned().ok_or_else(not_found)?;
        let mut state = port.state();
        if state.unplugged {
            return Err(not_found());
        }
        if state.handles > 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Access is denied.",
            ));
        }
        let (line, device) = socket_pair()?;
        line.set_read_timeout(Some(SERIAL_POLL))?;
        line.set_write_timeout(Some(SERIAL_POLL))?;
        line.set_nodelay(true)?;
        state.device = Some(device);
        state.opened.push(*parameters);
        state.written = 0;
        state.handles += 1;
        drop(state);
        Ok(Box::new(VirtualLine { port, stream: line }))
    }
}

/// A connected pair of loopback sockets.
fn socket_pair() -> io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let near = TcpStream::connect(listener.local_addr()?)?;
    let (far, _) = listener.accept()?;
    Ok((near, far))
}

struct Port {
    info: SerialPortInfo,
    state: Mutex<PortState>,
}

impl Port {
    fn state(&self) -> MutexGuard<'_, PortState> {
        lock(&self.state)
    }
}

#[derive(Default)]
struct PortState {
    /// The device's end of the current connection.
    device: Option<TcpStream>,
    /// The parameters of each open, in order.
    opened: Vec<SerialParameters>,
    /// Open handles on the port (the line and its clones); the port is
    /// free again once none are left.
    handles: usize,
    unplugged: bool,
    /// Bytes the session has written to the line since it was last opened.
    written: usize,
    /// When the break being sent started, and after how many bytes.
    break_started: Option<(Instant, usize)>,
    breaks: Vec<BreakSignal>,
    /// The names of the threads that read from the line.
    readers: Vec<String>,
}

/// A break the session sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakSignal {
    /// How many bytes the session had written before it.
    pub after_bytes: usize,
    /// How long the line was held in the break state.
    pub held: Duration,
}

/// The device end of a virtual port.
pub struct VirtualDevice {
    port: Arc<Port>,
}

impl VirtualDevice {
    /// The parameters the port was opened with, one entry per open.
    pub fn opened(&self) -> Vec<SerialParameters> {
        self.port.state().opened.clone()
    }

    /// Whether the session holds the port open.
    pub fn is_open(&self) -> bool {
        self.port.state().handles > 0
    }

    /// The breaks the session has sent.
    pub fn breaks(&self) -> Vec<BreakSignal> {
        self.port.state().breaks.clone()
    }

    /// The names of the threads the session read the port on.
    pub fn reader_threads(&self) -> Vec<String> {
        self.port.state().readers.clone()
    }

    /// The device's end of the line; an error once unplugged.
    fn stream(&self) -> io::Result<TcpStream> {
        match &self.port.state().device {
            Some(device) => device.try_clone(),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "the port is not open, or was unplugged",
            )),
        }
    }

    /// Sends `bytes` to the session, as the device's console output.
    /// Blocks while the session does not read (its buffers are full).
    pub fn send(&self, bytes: &[u8]) -> io::Result<()> {
        self.stream()?.write_all(bytes)
    }

    /// Reads what the session sends until it ends with `expected`, and
    /// returns all of it. Panics if it does not arrive in time.
    pub async fn received_ending_with(&self, expected: &[u8]) -> Vec<u8> {
        let mut stream = self.stream().expect("the device cannot read");
        let expected = expected.to_vec();
        tokio::task::spawn_blocking(move || {
            stream
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let deadline = Instant::now() + WAIT;
            let mut received = Vec::new();
            let mut buffer = [0; 1024];
            while !received.ends_with(&expected) {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {:?}; the device received {:?}",
                    String::from_utf8_lossy(&expected),
                    String::from_utf8_lossy(&received)
                );
                match stream.read(&mut buffer) {
                    Ok(0) => panic!(
                        "the line closed; the device received {:?}",
                        String::from_utf8_lossy(&received)
                    ),
                    Ok(count) => received.extend_from_slice(&buffer[..count]),
                    Err(error) if is_timeout(&error) => {}
                    Err(error) => panic!("the device cannot read: {error}"),
                }
            }
            received
        })
        .await
        .unwrap()
    }

    /// Pulls the adapter out: the line fails from now on, and the port
    /// disappears from the list.
    pub fn unplug(&self) {
        let mut state = self.port.state();
        state.unplugged = true;
        if let Some(device) = state.device.take() {
            let _ = device.shutdown(Shutdown::Both);
        }
    }

    /// Waits until the session has let go of the port.
    pub async fn wait_until_closed(&self) {
        let deadline = Instant::now() + WAIT;
        while self.is_open() {
            assert!(Instant::now() < deadline, "the port is still open");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// The session's end of a virtual port.
struct VirtualLine {
    port: Arc<Port>,
    stream: TcpStream,
}

impl VirtualLine {
    fn check_plugged_in(&self) -> io::Result<()> {
        if self.port.state().unplugged {
            // What Windows says when a USB serial adapter is pulled out.
            Err(io::Error::other(
                "The device does not recognize the command.",
            ))
        } else {
            Ok(())
        }
    }
}

impl SerialLine for VirtualLine {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        {
            let mut state = self.port.state();
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("unnamed").to_string();
            if !state.readers.contains(&name) {
                state.readers.push(name);
            }
        }
        self.check_plugged_in()?;
        let read = self.stream.read(buffer);
        self.check_plugged_in()?;
        match read {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the device closed the line",
            )),
            Ok(count) => Ok(count),
            Err(error) if is_timeout(&error) => Ok(0),
            Err(error) => Err(error),
        }
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check_plugged_in()?;
        match self.stream.write(bytes) {
            Ok(count) => {
                self.port.state().written += count;
                Ok(count)
            }
            Err(error) if is_timeout(&error) => Ok(0),
            Err(error) => Err(error),
        }
    }

    fn set_break(&mut self, on: bool) -> io::Result<()> {
        self.check_plugged_in()?;
        let mut state = self.port.state();
        if on {
            let written = state.written;
            state.break_started = Some((Instant::now(), written));
        } else if let Some((started, after_bytes)) = state.break_started.take() {
            state.breaks.push(BreakSignal {
                after_bytes,
                held: started.elapsed(),
            });
        }
        Ok(())
    }

    fn check(&mut self) -> io::Result<()> {
        self.check_plugged_in()
    }

    fn try_clone(&self) -> io::Result<Box<dyn SerialLine>> {
        let stream = self.stream.try_clone()?;
        self.port.state().handles += 1;
        Ok(Box::new(VirtualLine {
            port: self.port.clone(),
            stream,
        }))
    }
}

impl Drop for VirtualLine {
    fn drop(&mut self) {
        self.port.state().handles -= 1;
    }
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
