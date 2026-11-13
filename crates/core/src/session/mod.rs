//! Sessions: interactive connections to a device.
//!
//! Every transport sits behind the same session interface: bytes in
//! ([`SessionManager::write`]), bytes out and state changes
//! ([`SessionEvents`]), resize and close. Callers never see which transport
//! is underneath once a session is open.

mod ssh;

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

/// Output chunks and state changes a session may queue before the transport
/// stops reading from the device until the consumer catches up.
const EVENT_QUEUE: usize = 64;

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

/// An SSH server and the password login to use on it.
#[derive(Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
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

/// The stream of output and state changes of one session.
///
/// It ends after the `Closed` state. If the consumer falls behind, the
/// session stops reading from the device until it catches up. Dropping the
/// stream closes the session.
pub struct SessionEvents(mpsc::Receiver<SessionEvent>);

impl SessionEvents {
    /// The next event, or `None` once the session has closed.
    pub async fn next(&mut self) -> Option<SessionEvent> {
        self.0.recv().await
    }
}

impl fmt::Debug for SessionEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionEvents").finish_non_exhaustive()
    }
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
    sessions: Mutex<HashMap<SessionId, mpsc::UnboundedSender<Command>>>,
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

    /// Starts opening a session and returns at once; progress and failure
    /// arrive as [`SessionState`] events, starting with `Connecting`.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open(&self, target: SessionTarget, size: TerminalSize) -> (SessionId, SessionEvents) {
        let id = SessionId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel(EVENT_QUEUE);
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, command_tx);

        // The task holds the manager weakly so that dropping the last manager
        // handle drops the command senders, which closes every session.
        let manager = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            let mut link = Link {
                commands: command_rx,
                events: event_tx,
                size,
            };
            link.state(SessionState::Connecting).await;
            let reason = match target {
                SessionTarget::Ssh(target) => ssh::run(target, &mut link).await,
            };
            // Forget the session before announcing the close, so that nobody
            // who has seen `Closed` can still reach it.
            if let Some(manager) = manager.upgrade() {
                manager.forget(id);
            }
            link.state(SessionState::Closed(reason)).await;
        });

        (id, SessionEvents(event_rx))
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
        let commands = sessions.get(&id).ok_or(SessionError::NotFound)?;
        commands.send(command).map_err(|_| SessionError::NotFound)
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
struct Link {
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::Sender<SessionEvent>,
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

    /// Delivers output, waiting if the consumer is behind. Returns `false`
    /// if nobody is listening any more.
    async fn output(&mut self, bytes: Vec<u8>) -> bool {
        self.events.send(SessionEvent::Output(bytes)).await.is_ok()
    }

    async fn state(&mut self, state: SessionState) {
        let _ = self.events.send(SessionEvent::State(state)).await;
    }
}
