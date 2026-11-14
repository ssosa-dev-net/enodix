//! Sessions: interactive connections to a device.
//!
//! Every transport sits behind the same session interface: bytes in
//! ([`SessionManager::write`]), bytes out and state changes
//! ([`SessionEvents`]), resize and close. Callers never see which transport
//! is underneath once a session is open. Output goes through the
//! output pipeline: batching and flow control (see [`OutputOptions`]).

mod agent;
mod algorithms;
mod output;
mod recording;
mod ssh;

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

use crate::known_hosts::{HostKey, KnownHosts};
pub use algorithms::{
    is_legacy, offered_algorithms, AlgorithmKind, AlgorithmLists, NegotiatedAlgorithms,
    NegotiationFailure, LEGACY_ALGORITHMS, UNSUPPORTED_ALGORITHMS,
};
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
    /// The SSH hosts to tunnel through, in the order they are connected:
    /// the first is reached directly, each next one through the one before
    /// it (a `direct-tcpip` channel), and the target through the last.
    /// Empty: the target is reached directly.
    pub jump_hosts: Vec<JumpHost>,
    /// Also offer the outdated algorithms old equipment needs
    /// ([`LEGACY_ALGORITHMS`]) to the target; jump hosts have their own.
    pub legacy_crypto: bool,
}

/// An SSH host a session tunnels through, with its own login. Its host
/// key is checked like the target's, by its own `host` and `port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpHost {
    /// What failures call it, such as the workspace host id.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    /// Also offer the outdated algorithms ([`LEGACY_ALGORITHMS`]) to this
    /// jump host.
    pub legacy_crypto: bool,
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
    /// Public-key authentication with the keys an SSH agent holds: the
    /// agent signs, and the private keys never leave it.
    Agent(SshAgent),
    /// Ask the user for the password while connecting, once the host key
    /// is trusted (a [`SessionEvent::Prompt`]), and for the username too if
    /// the target's is empty. Used for jump hosts without a usable profile.
    Prompt,
}

/// Which SSH agent to ask for keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshAgent {
    /// The agent this computer runs: the one `SSH_AUTH_SOCK` names if it is
    /// set; otherwise, on Windows, the Windows OpenSSH agent and Pageant
    /// (the keys of both, if both run).
    System,
    /// The OpenSSH-protocol agent listening on this named pipe (Windows) or
    /// Unix socket.
    At(PathBuf),
    /// Pageant (Windows only).
    Pageant,
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
            Self::Agent(agent) => f.debug_tuple("Agent").field(agent).finish(),
            Self::Prompt => f.write_str("Prompt"),
        }
    }
}

/// Something a session reports to whoever opened it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Bytes from the device, exactly as received.
    Output(Vec<u8>),
    State(SessionState),
    /// While connecting, the server asks the user something
    /// (keyboard-interactive login: a password, a one-time code, ...).
    /// Answer with [`SessionManager::answer`]; a server may ask several
    /// times in a row.
    Prompt(LoginPrompt),
    /// While connecting, the server presented a host key that is not
    /// trusted for it: the first connection, or a changed key. Nothing goes
    /// on until [`SessionManager::decide_host_key`] (or closing) decides.
    HostKey(HostKeyQuestion),
    /// The algorithms each connection of the session agreed on, in connect
    /// order (jump hosts first, the device last). Sent once, just before
    /// [`SessionState::Connected`]; a session using any of the outdated
    /// [`LEGACY_ALGORITHMS`] says so in [`NegotiatedAlgorithms::legacy`].
    Algorithms(Vec<NegotiatedAlgorithms>),
}

/// Whether to trust a host key the server presented.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HostKeyQuestion {
    /// The host and port connected to, as the target names them.
    pub host: String,
    pub port: u16,
    /// The key the server presented.
    pub key: HostKey,
    /// The keys trusted for this host and port until now; empty on the
    /// first connection. Not empty means the key has changed.
    pub stored: Vec<HostKey>,
}

impl HostKeyQuestion {
    /// Whether the server's key differs from the one trusted until now.
    pub fn changed(&self) -> bool {
        !self.stored.is_empty()
    }
}

/// The user's answer to a [`HostKeyQuestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKeyDecision {
    /// Connect, and trust the key from now on; for a changed key, this
    /// replaces the stored one.
    TrustAndRemember,
    /// Connect this once; the next connection asks again.
    TrustOnce,
    /// Do not connect.
    Cancel,
}

/// Questions the server asks the user while logging in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LoginPrompt {
    /// A title for the questions; often empty.
    pub name: String,
    /// What to tell the user before the questions; often empty.
    pub instructions: String,
    /// The questions, in order; the answers go back in the same order.
    pub questions: Vec<LoginQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LoginQuestion {
    /// What the server asks, such as `Password: `.
    pub text: String,
    /// Whether the answer may be shown as it is typed; `false` for secrets.
    pub echo: bool,
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
    /// Logging in with the SSH agent did not work. The user can still log
    /// in another way, such as with a password.
    #[error("{0}")]
    Agent(AgentError),
    /// The user did not trust the host key on the first connection.
    #[error("the device's host key was not trusted")]
    HostKeyNotTrusted,
    /// The host key differs from the trusted one, and the user did not
    /// accept the new one.
    #[error("the device's host key has changed; the connection was refused")]
    HostKeyChanged,
    /// The trusted host keys could not be read, or the key could not be
    /// saved.
    #[error("{0}")]
    KnownHosts(String),
    /// The device was reached but the session could not be set up.
    #[error("could not open the session: {0}")]
    Protocol(String),
    /// The device and the session had no algorithm in common for some
    /// kind (an old device, say): says what the device offered.
    #[error("{0}")]
    Negotiation(NegotiationFailure),
    /// A jump host on the way failed: `error` says how (unreachable,
    /// login refused, host key not trusted, ...). A device a jump host
    /// cannot reach is a [`OpenError::Connect`] of the device instead.
    #[error("jump host {name}: {error}")]
    JumpHost {
        /// The [`JumpHost::name`] of the hop that failed.
        name: String,
        error: Box<OpenError>,
    },
}

/// Why logging in with an SSH agent did not work.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// No agent could be reached; says which were tried and why.
    #[error("no SSH agent is running ({0})")]
    NotRunning(String),
    /// The agent runs but holds no keys.
    #[error("the SSH agent holds no keys")]
    NoKeys,
    /// The server accepted none of the agent's keys.
    #[error("the server accepted none of the SSH agent's keys")]
    KeysRefused,
    /// The agent failed while signing (or refused to).
    #[error("the SSH agent failed: {0}")]
    Failed(String),
}

impl CloseReason {
    /// Whether the session failed in a way that logging in with a password
    /// might get past: the SSH agent was not running or had no key the
    /// server accepts.
    pub fn offers_password_login(&self) -> bool {
        matches!(self, Self::Failed(OpenError::Agent(_)))
    }

    /// The failed algorithm negotiation that ended the session, with the
    /// device's or the jump host's name: `None` for the device, else the
    /// [`JumpHost::name`].
    pub fn negotiation_failure(&self) -> Option<(Option<&str>, &NegotiationFailure)> {
        match self {
            Self::Failed(OpenError::Negotiation(failure)) => Some((None, failure)),
            Self::Failed(OpenError::JumpHost { name, error }) => match error.as_ref() {
                OpenError::Negotiation(failure) => Some((Some(name), failure)),
                _ => None,
            },
            _ => None,
        }
    }
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
#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Inner>,
}

struct Inner {
    next_id: AtomicU64,
    output: OutputOptions,
    /// The host keys SSH sessions trust without asking.
    known_hosts: KnownHosts,
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
    /// A manager whose SSH sessions check host keys against `known_hosts`
    /// (and remember the keys the user trusts there).
    pub fn new(known_hosts: KnownHosts) -> Self {
        Self::with_output_options(known_hosts, OutputOptions::default())
    }

    /// A manager whose sessions batch and pace their output as `options` say.
    pub fn with_output_options(known_hosts: KnownHosts, options: OutputOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(0),
                output: options,
                known_hosts,
                sessions: Mutex::default(),
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
        let known_hosts = self.inner.known_hosts.clone();
        tokio::spawn(async move {
            let mut link = Link {
                commands: command_rx,
                events: event_tx,
                flow: link_flow,
                size,
            };
            link.state(SessionState::Connecting);
            let reason = match target {
                SessionTarget::Ssh(target) => ssh::run(target, known_hosts, &mut link).await,
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

    /// Answers the questions of the last [`SessionEvent::Prompt`], in their
    /// order. Missing answers count as empty; extra ones are dropped. To
    /// refuse to answer, close the session.
    pub fn answer(&self, id: SessionId, answers: Vec<String>) -> Result<(), SessionError> {
        self.send(id, Command::Answer(answers))
    }

    /// Decides the last [`SessionEvent::HostKey`] question.
    pub fn decide_host_key(
        &self,
        id: SessionId,
        decision: HostKeyDecision,
    ) -> Result<(), SessionError> {
        self.send(id, Command::HostKey(decision))
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
    /// Answers to a login prompt.
    Answer(Vec<String>),
    /// The decision on a host key.
    HostKey(HostKeyDecision),
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
    /// opened. Input typed meanwhile is dropped; resizes are remembered;
    /// answers to login prompts and host-key decisions go to `replies`.
    async fn close_requested(&mut self, replies: &Replies) {
        loop {
            match self.command().await {
                Command::Close => return,
                Command::Resize(size) => self.size = size,
                Command::Answer(given) => {
                    let _ = replies.answers.send(given);
                }
                Command::HostKey(decision) => {
                    let _ = replies.decisions.send(decision);
                }
                Command::Write(_) => {}
            }
        }
    }

    /// What asks the user login questions and host-key questions while the
    /// session is being opened, and the senders [`Link::close_requested`]
    /// passes their replies to.
    fn askers(&self) -> (Prompter, HostKeyAsker, Replies) {
        let (answers_tx, answers) = mpsc::unbounded_channel();
        let (decisions_tx, decisions) = mpsc::unbounded_channel();
        let prompter = Prompter {
            events: self.events.clone(),
            answers,
            jump_host: None,
        };
        let asker = HostKeyAsker {
            events: self.events.clone(),
            decisions,
        };
        let replies = Replies {
            answers: answers_tx,
            decisions: decisions_tx,
        };
        (prompter, asker, replies)
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

    fn algorithms(&mut self, algorithms: Vec<NegotiatedAlgorithms>) {
        let _ = self.events.send(SessionEvent::Algorithms(algorithms));
    }

    fn state(&mut self, state: SessionState) {
        let _ = self.events.send(SessionEvent::State(state));
    }
}

/// Asks the session's owner login questions and waits for the answers.
struct Prompter {
    events: mpsc::UnboundedSender<SessionEvent>,
    answers: mpsc::UnboundedReceiver<Vec<String>>,
    /// The jump host being logged in to, if any: its questions say so in
    /// their [`LoginPrompt::name`], so they are not taken for the device's.
    jump_host: Option<String>,
}

impl Prompter {
    /// The answers to `prompt`, exactly one per question; `None` if nobody
    /// is listening any more.
    async fn ask(&mut self, mut prompt: LoginPrompt) -> Option<Vec<String>> {
        if let Some(jump_host) = &self.jump_host {
            prompt.name = if prompt.name.is_empty() {
                format!("Jump host {jump_host}")
            } else {
                format!("Jump host {jump_host}: {}", prompt.name)
            };
        }
        // Answers sent before this prompt was asked were for an earlier one.
        while self.answers.try_recv().is_ok() {}
        let count = prompt.questions.len();
        self.events.send(SessionEvent::Prompt(prompt)).ok()?;
        let mut answers = self.answers.recv().await?;
        answers.resize(count, String::new());
        Some(answers)
    }
}

/// Where the owner's replies go while the session is being opened.
struct Replies {
    answers: mpsc::UnboundedSender<Vec<String>>,
    decisions: mpsc::UnboundedSender<HostKeyDecision>,
}

/// Asks the session's owner whether to trust a host key and waits for the
/// decision.
struct HostKeyAsker {
    events: mpsc::UnboundedSender<SessionEvent>,
    decisions: mpsc::UnboundedReceiver<HostKeyDecision>,
}

impl HostKeyAsker {
    /// The decision on `question`; `None` if nobody is listening any more.
    async fn ask(&mut self, question: HostKeyQuestion) -> Option<HostKeyDecision> {
        // Decisions sent before this question was asked were not about it.
        while self.decisions.try_recv().is_ok() {}
        self.events.send(SessionEvent::HostKey(question)).ok()?;
        self.decisions.recv().await
    }
}
