//! Port forwarding over a terminal session's SSH connection (#21): local
//! forwards (`ssh -L`) and dynamic ones, a SOCKS5 proxy (`ssh -D`).
//!
//! A host's forwards are set in the workspace ([`PortForward::parse`] reads
//! them) and given to its session in [`super::OpenOptions::forwards`]. Each
//! time the session connects, every forward listens on its address (the
//! loopback one unless the forward names another); every connection
//! accepted there is carried, through a `direct-tcpip` channel of the
//! session's own connection to the device (so through its jump hosts,
//! with nothing logged in again), to the forward's destination as the
//! device reaches it. A dynamic forward asks the program connecting
//! (a browser, `curl --socks5-hostname`) where to, as SOCKS5 does.
//!
//! The forwards end with the connection; [`super::SessionManager::stop_forward`]
//! ends one before. Every change is reported as
//! [`super::SessionEvent::Forwards`], and [`super::SessionManager::forwards`]
//! lists them.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};

use super::ssh::DeviceConnection;
use super::SessionEvent;
use crate::tunnel::{pump, DEFAULT_BUFFER_LIMIT};

/// A port forward, as a host's settings define it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForward {
    /// Its name in the workspace (unique per host).
    pub name: String,
    /// The address it listens on: `127.0.0.1` unless it says otherwise.
    pub bind: IpAddr,
    /// The port it listens on; 0 takes any free one.
    pub port: u16,
    pub kind: ForwardKind,
}

/// Where a forward's connections go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardKind {
    /// To `host` and `port` as the device reaches them (`ssh -L`).
    Local { host: String, port: u16 },
    /// Wherever the program connecting asks, over SOCKS5 (`ssh -D`).
    Dynamic,
}

impl PortForward {
    /// Reads a forward as written in a settings file, as OpenSSH's options
    /// are: `L [bind:]port:host:hostport` or `D [bind:]port` (`-L` and
    /// `-D` too, in either case). An IPv6 address is written in brackets;
    /// `*` binds every address, `localhost` the loopback one. The error
    /// says what is wrong, for the workspace's problems.
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (kind, rest) = text
            .split_once(char::is_whitespace)
            .map(|(kind, rest)| (kind, rest.trim()))
            .unwrap_or((text, ""));
        let kind = kind.trim_start_matches('-').to_ascii_uppercase();
        let parts = split_colons(rest)?;
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        let (bind, port, kind) = match (kind.as_str(), parts.as_slice()) {
            ("L", [port, host, to]) => (None, *port, local(host, to)?),
            ("L", [bind, port, host, to]) => (Some(*bind), *port, local(host, to)?),
            ("L", _) => {
                return Err(format!(
                    "`{text}` is not a local forward: write `L [address:]port:host:port`"
                ))
            }
            ("D", [port]) => (None, *port, ForwardKind::Dynamic),
            ("D", [bind, port]) => (Some(*bind), *port, ForwardKind::Dynamic),
            ("D", _) => {
                return Err(format!(
                    "`{text}` is not a dynamic forward: write `D [address:]port`"
                ))
            }
            _ => {
                return Err(format!(
                    "`{text}` is not a forward: write `L [address:]port:host:port` or `D [address:]port`"
                ))
            }
        };
        let bind = match bind {
            None | Some("") => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Some("*") => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            Some(name) if name.eq_ignore_ascii_case("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Some(address) => address
                .parse()
                .map_err(|_| format!("`{address}` is not an address to listen on"))?,
        };
        let port = port
            .parse()
            .map_err(|_| format!("`{port}` is not a port to listen on (0 to 65535)"))?;
        Ok(Self {
            name: name.to_owned(),
            bind,
            port,
            kind,
        })
    }

    /// The forward as written, in the short form: the address is left out
    /// when it is the loopback one.
    pub fn spec(&self) -> String {
        let listen = match self.bind {
            IpAddr::V4(Ipv4Addr::LOCALHOST) => self.port.to_string(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED) => format!("*:{}", self.port),
            IpAddr::V6(address) => format!("[{address}]:{}", self.port),
            address => format!("{address}:{}", self.port),
        };
        match &self.kind {
            ForwardKind::Local { host, port } => format!("L {listen}:{}:{port}", bracketed(host)),
            ForwardKind::Dynamic => format!("D {listen}"),
        }
    }
}

impl fmt::Display for PortForward {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.spec())
    }
}

fn bracketed(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn local(host: &str, port: &str) -> Result<ForwardKind, String> {
    if host.is_empty() {
        return Err("a local forward needs the host to reach".into());
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("`{port}` is not a port to reach (1 to 65535)"))?;
    Ok(ForwardKind::Local {
        host: host.to_owned(),
        port,
    })
}

/// `text` split at the colons outside brackets, the brackets taken off.
fn split_colons(text: &str) -> Result<Vec<String>, String> {
    let mut parts = vec![String::new()];
    let mut bracket = false;
    for c in text.chars() {
        match c {
            '[' if !bracket => bracket = true,
            ']' if bracket => bracket = false,
            ':' if !bracket => parts.push(String::new()),
            c if c.is_whitespace() => return Err(format!("`{text}` has a space in it")),
            c => parts.last_mut().expect("never empty").push(c),
        }
    }
    if bracket {
        return Err(format!("`{text}` has a `[` without its `]`"));
    }
    Ok(parts)
}

/// A forward of an open session, as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardStatus {
    pub name: String,
    /// [`PortForward::spec`].
    pub spec: String,
    #[serde(flatten)]
    pub state: ForwardState,
    /// Why the last connection through it could not be carried (the device
    /// does not allow forwarding, the destination refused, ...), if one
    /// could not.
    pub last_failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ForwardState {
    /// Programs connect to `address` at `port` (the one taken, for port 0).
    Listening { address: String, port: u16 },
    /// It could not listen (the port is taken, say), and why.
    Failed { reason: String },
    /// The user stopped it.
    Stopped,
}

impl ForwardStatus {
    /// Whether programs can connect to it now.
    pub fn is_listening(&self) -> bool {
        matches!(self.state, ForwardState::Listening { .. })
    }
}

/// One forward and the task serving it.
struct Slot {
    status: ForwardStatus,
    task: Option<JoinHandle<()>>,
}

/// The forwards of one connection of a session, shared by its transport
/// (which starts and ends them) and its manager (which lists and stops
/// them).
#[derive(Default)]
pub(super) struct Forwards {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    slots: Vec<Slot>,
    /// Where changes are reported, once started.
    events: Option<mpsc::UnboundedSender<SessionEvent>>,
}

impl Forwards {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every forward, in the order they were given.
    pub(super) fn status(&self) -> Vec<ForwardStatus> {
        self.inner()
            .slots
            .iter()
            .map(|s| s.status.clone())
            .collect()
    }

    /// Reports the forwards as they are now.
    fn report(inner: &Inner) {
        if let Some(events) = &inner.events {
            let status = inner.slots.iter().map(|s| s.status.clone()).collect();
            let _ = events.send(SessionEvent::Forwards(status));
        }
    }

    /// Starts listening for each of `forwards`, carrying their connections
    /// over `device`; reports them all once, then on every change.
    pub(super) async fn start(
        self: &Arc<Self>,
        forwards: &[PortForward],
        device: DeviceConnection,
        events: &mpsc::UnboundedSender<SessionEvent>,
    ) {
        if forwards.is_empty() {
            return;
        }
        let mut slots = Vec::with_capacity(forwards.len());
        for forward in forwards {
            let mut status = ForwardStatus {
                name: forward.name.clone(),
                spec: forward.spec(),
                state: ForwardState::Stopped,
                last_failure: None,
            };
            let mut task = None;
            match TcpListener::bind(SocketAddr::new(forward.bind, forward.port)).await {
                Ok(listener) => {
                    let local = listener.local_addr().ok();
                    status.state = ForwardState::Listening {
                        address: local.map_or(forward.bind, |l| l.ip()).to_string(),
                        port: local.map_or(forward.port, |l| l.port()),
                    };
                    task = Some(tokio::spawn(serve(
                        listener,
                        forward.clone(),
                        device.clone(),
                        Arc::downgrade(self),
                    )));
                }
                Err(error) => {
                    status.state = ForwardState::Failed {
                        reason: format!(
                            "cannot listen on {}: {}",
                            SocketAddr::new(forward.bind, forward.port),
                            listen_failure(&error)
                        ),
                    };
                }
            }
            slots.push(Slot { status, task });
        }
        let mut inner = self.inner();
        inner.slots = slots;
        inner.events = Some(events.clone());
        Self::report(&inner);
    }

    /// Stops the forward `name`: it listens no more, and the connections it
    /// carries end. `false` if it has no such forward listening.
    pub(super) fn stop(&self, name: &str) -> bool {
        let mut inner = self.inner();
        let Some(slot) = inner
            .slots
            .iter_mut()
            .find(|slot| slot.status.name == name && slot.status.is_listening())
        else {
            return false;
        };
        if let Some(task) = slot.task.take() {
            task.abort();
        }
        slot.status.state = ForwardState::Stopped;
        Self::report(&inner);
        true
    }

    /// Ends every forward (the connection has ended): none are left.
    pub(super) fn end(&self) {
        let mut inner = self.inner();
        if inner.slots.is_empty() {
            return;
        }
        for slot in &mut inner.slots {
            if let Some(task) = slot.task.take() {
                task.abort();
            }
        }
        inner.slots.clear();
        Self::report(&inner);
        inner.events = None;
    }

    /// A connection through `name` could not be carried, and why.
    fn failed(&self, name: &str, reason: String) {
        let mut inner = self.inner();
        if let Some(slot) = inner.slots.iter_mut().find(|s| s.status.name == name) {
            slot.status.last_failure = Some(reason);
            Self::report(&inner);
        }
    }
}

impl Drop for Forwards {
    fn drop(&mut self) {
        for slot in &mut self.inner().slots {
            if let Some(task) = slot.task.take() {
                task.abort();
            }
        }
    }
}

/// The words for a listener that could not be bound.
fn listen_failure(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::AddrInUse => "the port is already in use".into(),
        std::io::ErrorKind::AddrNotAvailable => "this computer has no such address".into(),
        std::io::ErrorKind::PermissionDenied => "the system does not allow it".into(),
        _ => error.to_string(),
    }
}

/// Accepts connections to one forward until aborted; aborting it ends the
/// connections it carries too.
async fn serve(
    listener: TcpListener,
    forward: PortForward,
    device: DeviceConnection,
    forwards: std::sync::Weak<Forwards>,
) {
    let forward = Arc::new(forward);
    let mut carried = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((socket, from)) => {
                    let forward = forward.clone();
                    let device = device.clone();
                    let forwards = forwards.clone();
                    carried.spawn(async move {
                        if let Err(reason) = carry(socket, from, &forward, &device).await {
                            if let Some(forwards) = forwards.upgrade() {
                                forwards.failed(&forward.name, reason);
                            }
                        }
                    });
                }
                // Out of sockets, say: try again shortly.
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            },
            Some(_) = carried.join_next(), if !carried.is_empty() => {}
        }
    }
}

/// Carries one accepted connection to where its forward goes.
async fn carry(
    mut socket: TcpStream,
    from: SocketAddr,
    forward: &PortForward,
    device: &DeviceConnection,
) -> Result<(), String> {
    let channel = match &forward.kind {
        ForwardKind::Local { host, port } => device
            .direct_tcpip(host, *port, from)
            .await
            .map_err(|refusal| refusal.reason)?,
        ForwardKind::Dynamic => {
            let (host, port) = match socks_request(&mut socket).await {
                Ok(asked) => asked,
                // Not SOCKS5, or not a connection: nothing to report but
                // to the program.
                Err(Some(reply)) => {
                    let _ = socket.write_all(&socks_reply(reply)).await;
                    return Ok(());
                }
                Err(None) => return Ok(()),
            };
            match device.direct_tcpip(&host, port, from).await {
                Ok(channel) => {
                    if socket
                        .write_all(&socks_reply(SOCKS_SUCCEEDED))
                        .await
                        .is_err()
                    {
                        // The program went away meanwhile.
                        return Ok(());
                    }
                    channel
                }
                Err(refusal) => {
                    let _ = socket.write_all(&socks_reply(refusal.socks_reply)).await;
                    return Err(refusal.reason);
                }
            }
        }
    };
    pump(socket, channel, DEFAULT_BUFFER_LIMIT).await
}

/// Why the device did not open a channel to a destination.
pub(super) struct Refusal {
    /// In words, naming the destination.
    pub reason: String,
    /// What a SOCKS5 client is answered.
    pub socks_reply: u8,
}

pub(super) const SOCKS_SUCCEEDED: u8 = 0x00;
pub(super) const SOCKS_GENERAL_FAILURE: u8 = 0x01;
pub(super) const SOCKS_NOT_ALLOWED: u8 = 0x02;
pub(super) const SOCKS_HOST_UNREACHABLE: u8 = 0x04;
pub(super) const SOCKS_CONNECTION_REFUSED: u8 = 0x05;
const SOCKS_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const SOCKS_ADDRESS_NOT_SUPPORTED: u8 = 0x08;

/// A SOCKS5 reply with `code` and an empty bound address.
fn socks_reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

/// Reads a SOCKS5 greeting (no authentication) and CONNECT request: where
/// the program wants to go. `Err(Some(code))`: answer `code` and close;
/// `Err(None)`: just close (not SOCKS5, or gone).
async fn socks_request(socket: &mut TcpStream) -> Result<(String, u16), Option<u8>> {
    let mut head = [0u8; 2];
    socket.read_exact(&mut head).await.map_err(|_| None)?;
    if head[0] != 5 {
        return Err(None);
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    socket.read_exact(&mut methods).await.map_err(|_| None)?;
    if !methods.contains(&0) {
        // No acceptable method.
        let _ = socket.write_all(&[5, 0xff]).await;
        return Err(None);
    }
    socket.write_all(&[5, 0]).await.map_err(|_| None)?;
    let mut request = [0u8; 4];
    socket.read_exact(&mut request).await.map_err(|_| None)?;
    if request[0] != 5 {
        return Err(None);
    }
    let host = match request[3] {
        1 => {
            let mut address = [0u8; 4];
            socket.read_exact(&mut address).await.map_err(|_| None)?;
            Ipv4Addr::from(address).to_string()
        }
        3 => {
            let mut length = [0u8; 1];
            socket.read_exact(&mut length).await.map_err(|_| None)?;
            let mut name = vec![0u8; usize::from(length[0])];
            socket.read_exact(&mut name).await.map_err(|_| None)?;
            String::from_utf8(name).map_err(|_| Some(SOCKS_ADDRESS_NOT_SUPPORTED))?
        }
        4 => {
            let mut address = [0u8; 16];
            socket.read_exact(&mut address).await.map_err(|_| None)?;
            Ipv6Addr::from(address).to_string()
        }
        _ => return Err(Some(SOCKS_ADDRESS_NOT_SUPPORTED)),
    };
    let mut port = [0u8; 2];
    socket.read_exact(&mut port).await.map_err(|_| None)?;
    if request[1] != 1 {
        return Err(Some(SOCKS_COMMAND_NOT_SUPPORTED));
    }
    Ok((host, u16::from_be_bytes(port)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> PortForward {
        PortForward::parse("f", text).unwrap()
    }

    #[test]
    fn reads_local_and_dynamic_forwards() {
        let gui = parsed("L 8443:192.0.2.10:443");
        assert_eq!(gui.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(gui.port, 8443);
        assert_eq!(
            gui.kind,
            ForwardKind::Local {
                host: "192.0.2.10".into(),
                port: 443
            }
        );
        assert_eq!(gui.spec(), "L 8443:192.0.2.10:443");
        let proxy = parsed("-d 1080");
        assert_eq!(proxy.kind, ForwardKind::Dynamic);
        assert_eq!(proxy.spec(), "D 1080");
        assert_eq!(parsed("D *:1080").spec(), "D *:1080");
        assert_eq!(parsed("D localhost:1080").spec(), "D 1080");
        let v6 = parsed("L [::1]:8080:[2001:db8::1]:80");
        assert_eq!(v6.bind, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(v6.spec(), "L [::1]:8080:[2001:db8::1]:80");
    }

    #[test]
    fn says_what_is_wrong() {
        for (text, says) in [
            ("8443:host:443", "is not a forward"),
            ("L 8443:host", "is not a local forward"),
            ("L 8443:host:0", "not a port to reach"),
            ("L 70000:host:22", "not a port to listen on"),
            ("D a:b:c", "is not a dynamic forward"),
            ("D nowhere:1080", "not an address to listen on"),
            ("L [::1:80:h:1", "without its `]`"),
        ] {
            let error = PortForward::parse("f", text).unwrap_err();
            assert!(error.contains(says), "{text}: {error}");
        }
    }
}
