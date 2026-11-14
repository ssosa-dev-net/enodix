//! Sessions: interactive connections to a device.
//!
//! Every transport sits behind the same session interface: bytes in
//! ([`SessionManager::write`]), bytes out and state changes
//! ([`SessionEvents`]), resize and close. Callers never see which transport
//! is underneath once a session is open. Output goes through the
//! output pipeline: batching and flow control (see [`OutputOptions`]).

mod output;
mod ssh;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

use output::Flow;
pub use output::{OutputOptions, SessionEvents};

/// Identifies one session within its [`SessionManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// Terminal dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u32,
    pub rows: u32,
}

/// What to connect to.
#[derive(Clone)]
pub enum SessionTarget {
    Ssh(SshTarget),
}

/// The port SSH uses when nothing sets one.
pub const DEFAULT_SSH_PORT: u16 = 22;

/// An SSH server and the login to use on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
}

/// How an SSH session proves who the user is. `Debug` leaves the secrets
/// out, so a target can be logged.
#[derive(Clone, PartialEq, Eq)]
pub enum SshAuth {
    Password(String),
    /// Public-key authentication with the private key in `key_file`
    /// (OpenSSH, PEM or PuTTY format), decrypted with `passphrase` if it is
    /// encrypted.
    PrivateKey {
        key_file: PathBuf,
        passphrase: Option<String>,
    },
}

impl fmt::Debug for SshAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password(_) => f.write_str("Password(..)"),
            Self::PrivateKey {
                key_file,
                passphrase,
            } => f
                .debug_struct("PrivateKey")
                .field("key_file", key_file)
                .field("passphrase", &passphrase.as_ref().map(|_| ".."))
                .finish(),
        }
    }
}

/// Something a session reports to whoever opened it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Bytes from the device, exactly as received.
    Output(Vec<u8>),
    State(SessionState),
}

/// A session goes `Connecting`, then `Connected`, then `Closed`; or straight
/// from `Connecting` to `Closed` if it cannot be opened. `Closed` is final.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Connected,
    Closed(CloseReason),
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloseReason {
    /// Closed from this side with [`SessionManager::close`], or because the
    /// manager or the event stream was dropped.
    #[error("session closed")]
    ClosedLocally,
    /// The remote shell ended, for example because the user typed `exit`.
    #[error("the remote end closed the session{}", exit_status_suffix(.exit_status))]
    RemoteExited { exit_status: Option<u32> },
    /// The connection went away while the session was open.
    #[error("connection lost")]
    ConnectionLost,
    /// The session never got as far as `Connected`.
    #[error("{0}")]
    Failed(OpenError),
}

fn exit_status_suffix(exit_status: &Option<u32>) -> String {
    match exit_status {
        Some(status) => format!(" (exit status {status})"),
        None => String::new(),
    }
}

/// Why a session could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// The device could not be reached (refused, unreachable, timed out, ...).
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("authentication failed")]
    AuthenticationFailed,
    /// The private key to log in with could not be read or decrypted.
    #[error("cannot use the private key {file}: {reason}")]
    PrivateKey { file: String, reason: String },
    /// The device was reached but the session could not be set up.
    #[error("could not open the session: {0}")]
    Protocol(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The session does not exist or has already closed.
    #[error("no such session")]
    NotFound,
}

/// Opens, tracks and closes sessions.
///
/// Cloning gives another handle to the same set of sessions. Sessions are
/// closed when the last handle is dropped.
#[derive(Clone, Default)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    next_id: AtomicU64,
    output: OutputOptions,
    sessions: Mutex<HashMap<SessionId, SessionHandle>>,
}

/// How the manager reaches an open session.
struct SessionHandle {
    commands: mpsc::UnboundedSender<Command>,
    flow: Arc<Flow>,
}

impl Inner {
    fn forget(&self, id: SessionId) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// A manager whose sessions batch and pace their output as `options` say.
    pub fn with_output_options(options: OutputOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                output: options,
                ..Inner::default()
            }),
        }
    }

    /// Starts opening a session and returns at once; progress and failure
    /// arrive as [`SessionState`] events, starting with `Connecting`.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open(&self, target: SessionTarget, size: TerminalSize) -> (SessionId, SessionEvents) {
        let id = SessionId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        // Unbounded, so the transport never waits on the consumer and keeps
        // taking commands; flow control bounds the output queued here.
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let flow = Arc::new(Flow::new(self.inner.output.window));
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                SessionHandle {
                    commands: command_tx,
                    flow: flow.clone(),
                },
            );

        // The task holds the manager weakly so that dropping the last manager
        // handle drops the command senders, which closes every session.
        let manager = Arc::downgrade(&self.inner);
        let link_flow = flow.clone();
        tokio::spawn(async move {
            let mut link = Link {
                commands: command_rx,
                events: event_tx,
                flow: link_flow,
                size,
            };
            link.state(SessionState::Connecting);
            let reason = match target {
                SessionTarget::Ssh(target) => ssh::run(target, &mut link).await,
            };
            // Forget the session before announcing the close, so that nobody
            // who has seen `Closed` can still reach it.
            if let Some(manager) = manager.upgrade() {
                manager.forget(id);
            }
            link.state(SessionState::Closed(reason));
        });

        (id, SessionEvents::new(event_rx, flow, self.inner.output))
    }

    /// Tells the session that `bytes` bytes of its output have been consumed,
    /// for a consumer that does not hold the [`SessionEvents`] itself.
    pub fn acknowledge(&self, id: SessionId, bytes: usize) -> Result<(), SessionError> {
        let sessions = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let session = sessions.get(&id).ok_or(SessionError::NotFound)?;
        session.flow.acknowledge(bytes);
        Ok(())
    }

    /// Sends bytes to the device, for example keystrokes.
    pub fn write(&self, id: SessionId, bytes: Vec<u8>) -> Result<(), SessionError> {
        self.send(id, Command::Write(bytes))
    }

    /// Tells the device the terminal has a new size.
    pub fn resize(&self, id: SessionId, size: TerminalSize) -> Result<(), SessionError> {
        self.send(id, Command::Resize(size))
    }

    /// Closes the session. The `Closed` state follows on its event stream.
    pub fn close(&self, id: SessionId) -> Result<(), SessionError> {
        self.send(id, Command::Close)
    }

    fn send(&self, id: SessionId, command: Command) -> Result<(), SessionError> {
        let sessions = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let session = sessions.get(&id).ok_or(SessionError::NotFound)?;
        session
            .commands
            .send(command)
            .map_err(|_| SessionError::NotFound)
    }
}

/// What a session's owner asks of its transport.
enum Command {
    Write(Vec<u8>),
    Resize(TerminalSize),
    Close,
}

/// A transport's side of the session interface: where it takes commands from
/// and reports output and state to. Each transport is a function that drives
/// a `Link` until the session ends and returns why it ended.
///
/// A transport reads from the device only once [`Link::flow`]'s
/// `room_to_read` resolves, and keeps taking commands meanwhile.
struct Link {
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<SessionEvent>,
    flow: Arc<Flow>,
    /// The size the terminal should have, kept current while connecting.
    size: TerminalSize,
}

impl Link {
    /// The next command. Losing the session's owner counts as `Close`.
    async fn command(&mut self) -> Command {
        self.commands.recv().await.unwrap_or(Command::Close)
    }

    /// Resolves when the session should be abandoned while it is still being
    /// opened. Input typed meanwhile is dropped; resizes are remembered.
    async fn close_requested(&mut self) {
        loop {
            match self.command().await {
                Command::Close => return,
                Command::Resize(size) => self.size = size,
                Command::Write(_) => {}
            }
        }
    }

    /// The session's flow control, for waiting until there is room to read.
    fn flow(&self) -> Arc<Flow> {
        self.flow.clone()
    }

    /// Passes on output read from the device. Returns `false` if nobody is
    /// listening any more.
    fn output(&mut self, bytes: Vec<u8>) -> bool {
        self.flow.read(bytes.len());
        self.events.send(SessionEvent::Output(bytes)).is_ok()
    }

    fn state(&mut self, state: SessionState) {
        let _ = self.events.send(SessionEvent::State(state));
    }
}
