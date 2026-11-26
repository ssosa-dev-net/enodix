//! Sessions: interactive connections to a device.
//!
//! Every transport sits behind the same session interface: bytes in
//! ([`SessionManager::write`]), bytes out and state changes
//! ([`SessionEvents`]), resize and close. Callers never see which transport
//! is underneath once a session is open. Output goes through the
//! output pipeline: batching and flow control (see [`OutputOptions`]).

mod agent;
mod algorithms;
mod auto_reconnect;
pub(crate) mod connection;
mod forward;
mod local;
mod origin;
mod output;
mod output_watch;
mod record;
mod recording;
mod relay;
mod relays;
mod serial;
mod ssh;
mod system_ssh;
mod tcp;
mod telnet;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::known_hosts::{HostKey, KnownHosts};
use crate::logging::{LogFailure, Logger, SessionLog};
pub use crate::workspace::TerminalType;
pub use algorithms::{
    is_legacy, offered_algorithms, AlgorithmKind, AlgorithmLists, NegotiatedAlgorithms,
    NegotiationFailure, LEGACY_ALGORITHMS, UNSUPPORTED_ALGORITHMS,
};
pub use auto_reconnect::{AutoReconnect, AutoReconnects, Retry, AUTO_RECONNECT_DELAYS};
pub use forward::{ForwardKind, ForwardState, ForwardStatus, PortForward, SOCKS_HANDSHAKE_TIMEOUT};
pub use local::{
    local_environment, local_shells, local_start_folder, pick_local_shell, LocalShell, LocalTarget,
    CLI_VARIABLE, WORKSPACE_VARIABLE,
};
pub use origin::{HostOrigin, HostSession, HostWay, ReconnectRefused, Reconnection, TypedLogin};
use output::Flow;
pub use output::{OutputOptions, SessionEvents};
use output_watch::Watchers;
pub use output_watch::{OutputWatch, WATCH_LIMIT};
pub use record::{SessionRecord, SessionRecords};
pub use relay::{Attached, Consumer, ConsumerGone, ConsumerKey, HandoverError, Relay, Reported};
pub use relays::SessionRelays;
pub use serial::{
    SerialLine, SerialParameters, SerialPortInfo, SerialPortKind, SerialPorts, SerialTarget,
    SystemSerialPorts, BREAK_DURATION, SERIAL_POLL,
};
pub(crate) use ssh::JumpConnections;
pub use system_ssh::{system_ssh_target, SystemSshHost, SYSTEM_SSH_PROGRAM, SYSTEM_SSH_WARNING};

/// Identifies one session within its [`SessionManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// Terminal dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u32,
    pub rows: u32,
}

/// What to connect to. `Debug` leaves the secrets out.
#[derive(Debug, Clone)]
pub enum SessionTarget {
    Ssh(SshTarget),
    Telnet(TelnetTarget),
    Serial(SerialTarget),
    Local(LocalTarget),
}

impl SessionTarget {
    /// Whether the session's traffic is safe from eavesdroppers on the
    /// network: SSH encrypts it; telnet sends everything, passwords
    /// included, in the clear. A serial line does not cross a network, so
    /// it counts as safe, and so does a local shell.
    pub fn encrypted(&self) -> bool {
        match self {
            Self::Ssh(_) | Self::Serial(_) | Self::Local(_) => true,
            Self::Telnet(_) => false,
        }
    }

    /// The transport the session uses.
    pub fn transport(&self) -> Transport {
        match self {
            Self::Ssh(_) => Transport::Ssh,
            Self::Telnet(_) => Transport::Telnet,
            Self::Serial(_) => Transport::Serial,
            Self::Local(_) => Transport::Local,
        }
    }
}

/// How a session reaches its device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Ssh,
    Telnet,
    Serial,
    /// A shell on this computer (#22), not a device.
    Local,
}

/// The port SSH uses when nothing sets one.
pub const DEFAULT_SSH_PORT: u16 = 22;

/// The port telnet uses when nothing sets one.
pub const DEFAULT_TELNET_PORT: u16 = 23;

/// How long a connection may wait on the network while it is being opened
/// when nothing sets it: see [`SshTarget::connect_timeout`].
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// How long opening the session may wait on the device: for the TCP
    /// connection, the SSH handshake, each answer while logging in, and the
    /// shell to start. The time the user takes to answer a question (host
    /// key, password, one-time code) does not count, and the clock starts
    /// again after each answer. Jump hosts have their own.
    pub connect_timeout: Duration,
    /// The terminal type announced in the pty request.
    pub terminal_type: TerminalType,
}

/// A telnet server. Telnet has no login of its own: the device asks for
/// one in its output, and the user answers in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelnetTarget {
    pub host: String,
    pub port: u16,
    /// How long opening the session may wait for the TCP connection.
    pub connect_timeout: Duration,
    /// The SSH jump hosts to go through, in connect order (none: connect
    /// directly). The last one opens the TCP connection to `host:port`
    /// (`direct-tcpip`), so only the leg from it to the device is in the
    /// clear.
    pub jump_hosts: Vec<JumpHost>,
    /// The terminal type answered to the device's request (RFC 1091).
    pub terminal_type: TerminalType,
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
    /// [`SshTarget::connect_timeout`] for reaching and logging in to this
    /// jump host.
    pub connect_timeout: Duration,
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
    /// The session's log could not be written (a full disk, a folder that
    /// cannot be created, ...). The session goes on, no longer logged.
    LogFailed(LogFailure),
    /// How the backup the session started once connected ended (see
    /// [`crate::job::LoginBackup`]): sent once per connection, after
    /// [`SessionState::Connected`], whenever the backup ends; the session
    /// itself is never held up by it.
    Backup(crate::job::LoginBackupOutcome),
    /// The session's port forwards ([`OpenOptions::forwards`]) as they are
    /// now: sent once they listen, after each change (one stopped, a
    /// connection through one that failed), and empty when the connection
    /// ends. [`SessionManager::forwards`] gives the same list.
    Forwards(Vec<ForwardStatus>),
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
    /// The shell of a local session ended, for example because the user
    /// typed `exit`.
    #[error("the shell ended{}", exit_status_suffix(.exit_status))]
    LocalExited { exit_status: Option<u32> },
    /// The serial port stopped working while the session was open: most
    /// often, its USB adapter was unplugged.
    #[error("the serial port {port} stopped working; was the adapter unplugged? ({reason})")]
    PortLost { port: String, reason: String },
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
    /// The device's name has no address (a typo, or a name the DNS does not
    /// know).
    #[error("the name {host} could not be resolved")]
    NameNotResolved { host: String },
    /// No route leads to the device's address (the system said so at once).
    #[error("{host} cannot be reached ({reason})")]
    Unreachable { host: String, reason: String },
    /// The device answered that nothing listens on the port.
    #[error("{host} refused the connection on port {port}")]
    ConnectionRefused { host: String, port: u16 },
    /// The device did not answer in time while the session was being
    /// opened ([`SshTarget::connect_timeout`],
    /// [`TelnetTarget::connect_timeout`]); `stage` says at which step it
    /// went silent (#140).
    #[error("{stage} within {} s", .after.as_secs_f64())]
    TimedOut {
        after: Duration,
        stage: ConnectStage,
    },
    /// The device could not be reached for another reason, or the
    /// connection broke while it was being set up.
    #[error("could not connect: {0}")]
    Connect(String),
    /// The device closed the connection before sending a byte of SSH: a
    /// device whose lines (vty) are all in use does that, as IOS does.
    #[error(
        "the device closed the connection before the login began; all its lines may be in use"
    )]
    ClosedBeforeLogin,
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
    /// The serial port could not be opened: it does not exist, another
    /// program holds it, or it refused the line parameters.
    #[error("cannot open the serial port {port}: {reason}")]
    SerialPort { port: String, reason: String },
    /// A local shell could not be started: the program is not there, or
    /// the pseudo terminal could not be made.
    #[error("cannot start {program}: {reason}")]
    LocalShell { program: String, reason: String },
    /// The device was reached but the session could not be set up.
    #[error("could not open the session: {0}")]
    Protocol(String),
    /// A second channel ([`SessionManager::open_channel`]): the device
    /// refused to open another shell on the connection.
    #[error("the device refused a second channel on the connection ({reason})")]
    ChannelRefused { reason: String },
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

/// The step of opening a session at which the device (or the jump host
/// being reached) stopped answering: what an [`OpenError::TimedOut`] says
/// did not happen (#140). Each step has the whole connect timeout, the
/// clock starting again after the user answers a question.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectStage {
    /// No TCP connection to `host` on `port`: nothing answered the
    /// connection request (a wrong address, a device that is down, a
    /// firewall dropping it). `via` is the jump host that was asked to make
    /// the connection, if one was.
    #[error(
        "no TCP connection to {host} port {port}{}",
        .via.as_ref().map(|via| format!(" through the jump host {via}")).unwrap_or_default()
    )]
    Tcp {
        host: String,
        port: u16,
        via: Option<String>,
    },
    /// Connected over TCP, but the device sent no SSH identification line
    /// (`SSH-2.0-...`): something listens there that does not speak SSH,
    /// or the SSH server is stuck.
    #[error("connected, but no SSH identification line came")]
    Identification,
    /// The device sent its identification line, but the key exchange did
    /// not finish.
    #[error("the SSH identification line came, but the key exchange did not finish")]
    KeyExchange,
    /// The key exchange finished, but the device did not answer the login
    /// (a device waiting on its RADIUS or TACACS+ server, say).
    #[error("the key exchange finished, but the login got no answer")]
    Login,
    /// Logged in, but the device did not answer the request for a session
    /// channel (for the shell, or a file connection's SFTP or SCP).
    #[error("logged in, but the request for a session channel got no answer")]
    Channel,
    /// Logged in and the channel opened, but the device did not start the
    /// shell.
    #[error("logged in, but the shell did not start")]
    Shell,
    /// Logged in and the channel opened, but the device did not answer the
    /// request for the subsystem `name` (`sftp`).
    #[error("logged in, but the request for the {name} subsystem got no answer")]
    Subsystem { name: String },
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

impl OpenError {
    /// What the user can do about the failure, when the reason alone does
    /// not say it (#116): what to check and where it is set. `None` when
    /// the reason says it already, or the interface asks its own question
    /// (a failed negotiation's legacy-crypto card).
    pub fn advice(&self) -> Option<String> {
        let advice = match self {
            Self::NameNotResolved { .. } => {
                "Check the address: a name your DNS knows, or an IP address.".into()
            }
            Self::Unreachable { .. } => {
                "Check the address, and that this computer has a route to it (a VPN, say).".into()
            }
            Self::ConnectionRefused { port, .. } => format!(
                "Nothing accepts connections on port {port}: check the Port setting, and that \
                 the device has SSH (or telnet) turned on."
            ),
            Self::TimedOut { stage, .. } => stage.advice(),
            Self::AuthenticationFailed => "Check the username and password (or key). For a \
                host with a credential profile, change them in the Credentials view, then \
                reconnect."
                .into(),
            Self::Agent(AgentError::NotRunning(_)) => {
                "Start your SSH agent with your key in it and connect again, or log in with a \
                 password."
                    .into()
            }
            Self::Agent(AgentError::NoKeys) => {
                "Add your key to the SSH agent (ssh-add) and connect again, or log in with a \
                 password."
                    .into()
            }
            Self::Agent(AgentError::KeysRefused) => {
                "Check the username, and that the device has your public key; or log in with a \
                 password."
                    .into()
            }
            Self::HostKeyChanged => "If the device was replaced or reinstalled, check its new \
                fingerprint with the device, then connect again and replace the key."
                .into(),
            Self::JumpHost { name, error } => {
                return error.advice().map(|advice| {
                    let mut chars = advice.chars();
                    let first = chars.next().map(|c| c.to_lowercase().collect::<String>());
                    format!(
                        "For the jump host {name}: {}{}",
                        first.unwrap_or_default(),
                        chars.as_str()
                    )
                });
            }
            _ => return None,
        };
        Some(advice)
    }
}

impl ConnectStage {
    /// What to check when the device went silent at this stage.
    fn advice(&self) -> String {
        match self {
            Self::Tcp { via: None, .. } => "Check the address and that the device is up; a \
                firewall may be dropping the connection. A slow device may need a longer Connect \
                timeout."
                .into(),
            Self::Tcp { via: Some(via), .. } => format!(
                "Check the address, that the device is up, and that {via} can reach it; a \
                 firewall may be dropping the connection. A slow device may need a longer \
                 Connect timeout."
            ),
            Self::Identification => "Check the Port setting: what answers there may not be an \
                SSH server (a telnet or console port, say), or the device's SSH server is stuck \
                or has no line free."
                .into(),
            Self::KeyExchange => "The device may be slow to compute the key exchange (an old \
                or busy one): try a longer Connect timeout."
                .into(),
            Self::Login => "The device may be waiting on its authentication server (RADIUS, \
                TACACS+): check that the device reaches it, or try a longer Connect timeout."
                .into(),
            Self::Channel | Self::Shell | Self::Subsystem { .. } => "The device accepted the \
                login but went no further: it may be busy, or out of sessions for this user. Try \
                again, or a longer Connect timeout."
                .into(),
        }
    }
}

impl OpenError {
    /// Whether trying the same login again by itself is safe: the device
    /// was not reached, or hung up before a login began. Never a refused
    /// login (accounts lock), a host-key question, or a failure that
    /// trying again cannot change (a key, the agent, the algorithms).
    pub fn allows_auto_reconnect(&self) -> bool {
        match self {
            Self::NameNotResolved { .. }
            | Self::Unreachable { .. }
            | Self::ConnectionRefused { .. }
            | Self::Connect(_)
            | Self::ClosedBeforeLogin => true,
            // Only before anything of the login was sent: from the login
            // on, the credentials may have reached an AAA server (RADIUS,
            // TACACS+) that counts each try against the account.
            Self::TimedOut { stage, .. } => matches!(
                stage,
                ConnectStage::Tcp { .. } | ConnectStage::Identification | ConnectStage::KeyExchange
            ),
            Self::JumpHost { error, .. } => error.allows_auto_reconnect(),
            _ => false,
        }
    }
}

impl CloseReason {
    /// Whether a session that closed for this reason may try again by
    /// itself (the `auto_reconnect` setting): its connection was lost, or
    /// a try failed before anyone could refuse a login ([`OpenError::
    /// allows_auto_reconnect`]). Never one the user ended (closed, `exit`).
    pub fn allows_auto_reconnect(&self) -> bool {
        match self {
            Self::ConnectionLost => true,
            Self::Failed(error) => error.allows_auto_reconnect(),
            _ => false,
        }
    }

    /// What the user can do about it: [`OpenError::advice`] for a session
    /// that could not be opened, else `None`.
    pub fn advice(&self) -> Option<String> {
        match self {
            Self::Failed(error) => error.advice(),
            _ => None,
        }
    }

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
    /// The session does not exist or has already closed (for
    /// [`SessionManager::reconnect`] and [`SessionManager::duplicate`]: does
    /// not exist, or was discarded).
    #[error("no such session")]
    NotFound,
    /// The session is still open, so it cannot be reconnected.
    #[error("the session is still open")]
    StillOpen,
    /// Only serial sessions can send a break.
    #[error("only serial sessions can send a break")]
    BreakUnsupported,
    /// Only SSH sessions carry more than one channel.
    #[error("only SSH sessions can open a second channel")]
    ChannelsUnsupported,
    /// The session is not connected (yet, or any more).
    #[error("the session is not connected")]
    NotConnected,
}

/// How [`SessionManager::open_with`] opens a session, beyond its target.
#[derive(Clone, Default)]
pub struct OpenOptions {
    /// Where the session is logged; `None`: not logged.
    pub log: Option<SessionLog>,
    /// Called each time the session connects: on opening, and again on
    /// every [`SessionManager::reconnect`] (a [`SessionManager::duplicate`]
    /// keeps it too; [`SessionManager::reconnect_host`] is given new
    /// options).
    pub when_connected: Option<Arc<dyn WhenConnected>>,
    /// Whether an SSH session shows the login banners its device and jump
    /// hosts send before the login (`banner login` on IOS, `/etc/issue.net`
    /// on Linux), as its first output: a terminal's does (#134). Off, as
    /// for jobs and the backups they make, the banners are dropped. Each
    /// connection shows them again (a reconnect does: the device sends them
    /// again); a second channel has none.
    pub login_banner: bool,
    /// The port forwards an SSH session opens each time it connects, over
    /// its connection (see [`PortForward`]); others, second channels and
    /// duplicates open none.
    pub forwards: Vec<PortForward>,
}

/// Something done each time a session connects, beside it.
pub trait WhenConnected: Send + Sync {
    /// The session `session` has just connected. Must return at once
    /// (spawn the work), as the session waits for it.
    fn connected(&self, session: ConnectedSession);
}

/// A session that has just connected, as [`WhenConnected`] sees it.
pub struct ConnectedSession {
    manager: SessionManager,
    id: SessionId,
    /// That connection's events; once it has ended, reports go nowhere.
    events: mpsc::WeakUnboundedSender<SessionEvent>,
}

impl ConnectedSession {
    /// The manager the session belongs to.
    pub fn manager(&self) -> &SessionManager {
        &self.manager
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Tells the session's owner how its backup ended
    /// ([`SessionEvent::Backup`]), unless that connection has ended.
    pub fn report_backup(&self, outcome: crate::job::LoginBackupOutcome) {
        if let Some(events) = self.events.upgrade() {
            let _ = events.send(SessionEvent::Backup(outcome));
        }
    }
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
    /// Where serial sessions find their ports.
    serial_ports: Arc<dyn SerialPorts>,
    /// Every session not discarded, open or closed.
    sessions: Mutex<HashMap<SessionId, Entry>>,
}

/// A session the manager knows: what it connects to, so that it can be
/// reconnected or duplicated, and how to reach it while it is open.
struct Entry {
    target: SessionTarget,
    /// The log, and what is done once connected.
    options: OpenOptions,
    /// For a session to a workspace host: how it was asked for, what
    /// reconnecting it works the target out from (#130). `None`: it
    /// reconnects as it was made (a quick connection).
    origin: Option<HostOrigin>,
    /// For a second channel: the session whose connection carries it.
    channel_of: Option<SessionId>,
    /// `None` once the session has closed.
    open: Option<SessionHandle>,
}

/// How the manager reaches an open session.
struct SessionHandle {
    commands: mpsc::UnboundedSender<Command>,
    flow: Arc<Flow>,
    /// Set once the session is connected (its transport sets it).
    connected: Arc<AtomicBool>,
    /// This connection's port forwards.
    forwards: Arc<forward::Forwards>,
    /// Who watches its output beside its consumer (#23).
    watchers: Arc<Watchers>,
}

/// What [`SessionManager::write_to_each`] did, per session, in the order
/// the sessions were given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fanout {
    /// The sessions the input was written to.
    pub written: Vec<SessionId>,
    /// The sessions that did not take it: unknown, closed or still
    /// connecting.
    pub skipped: Vec<SessionId>,
}

impl Inner {
    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, Entry>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks the session closed, keeping its target.
    fn mark_closed(&self, id: SessionId) {
        if let Some(entry) = self.sessions().get_mut(&id) {
            entry.open = None;
        }
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
        Self::with_serial_ports(known_hosts, options, Arc::new(SystemSerialPorts))
    }

    /// A manager whose serial sessions open their ports through
    /// `serial_ports` (the others use the system's, [`SystemSerialPorts`]).
    pub fn with_serial_ports(
        known_hosts: KnownHosts,
        options: OutputOptions,
        serial_ports: Arc<dyn SerialPorts>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(0),
                output: options,
                known_hosts,
                serial_ports,
                sessions: Mutex::default(),
            }),
        }
    }

    /// The known-hosts store its SSH sessions check host keys against
    /// (shared: the same store, not a copy), for file connections made
    /// beside its sessions.
    pub fn known_hosts(&self) -> KnownHosts {
        self.inner.known_hosts.clone()
    }

    /// The serial ports there are now, with their adapters' descriptions,
    /// in port order (`COM3` before `COM10`). Call again to refresh.
    pub fn serial_ports(&self) -> io::Result<Vec<SerialPortInfo>> {
        Ok(serial::sorted(self.inner.serial_ports.list()?))
    }

    /// Starts opening a session and returns at once; progress and failure
    /// arrive as [`SessionState`] events, starting with `Connecting`.
    ///
    /// Once closed, the session can be opened again with
    /// [`Self::reconnect`] until it is [discarded](Self::discard).
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open(&self, target: SessionTarget, size: TerminalSize) -> (SessionId, SessionEvents) {
        self.open_logged(target, size, None)
    }

    /// Like [`Self::open`], and logs the session to `log` (if given): each
    /// connection, reconnections and duplicates included, writes its own
    /// file there once connected. A log that cannot be written is reported
    /// with [`SessionEvent::LogFailed`] and the session goes on.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open_logged(
        &self,
        target: SessionTarget,
        size: TerminalSize,
        log: Option<SessionLog>,
    ) -> (SessionId, SessionEvents) {
        let options = OpenOptions {
            log,
            ..OpenOptions::default()
        };
        self.open_with(target, size, options)
    }

    /// Like [`Self::open`], with a log and something to do each time the
    /// session connects (see [`OpenOptions`]).
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open_with(
        &self,
        target: SessionTarget,
        size: TerminalSize,
        options: OpenOptions,
    ) -> (SessionId, SessionEvents) {
        self.add(target, options, None, None, size)
    }

    /// Like [`Self::open_with`], for a session to a workspace host worked
    /// out by [`crate::credentials::Profiles::host_session`]: the manager
    /// keeps its origin ([`Self::origin`]), so that reconnecting it can work
    /// the target out again ([`Reconnection::work_out`],
    /// [`Self::reconnect_host`]).
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open_host(
        &self,
        session: HostSession,
        size: TerminalSize,
        options: OpenOptions,
    ) -> (SessionId, SessionEvents) {
        self.add(session.target, options, Some(session.origin), None, size)
    }

    /// How the session `id` (open or closed, until it is discarded) was
    /// asked for, if it goes to a workspace host ([`Self::open_host`], or a
    /// duplicate of such a session); `None` for one opened by its target
    /// alone, which reconnects as it was made.
    pub fn origin(&self, id: SessionId) -> Result<Option<HostOrigin>, SessionError> {
        self.inner
            .sessions()
            .get(&id)
            .map(|entry| entry.origin.clone())
            .ok_or(SessionError::NotFound)
    }

    /// Opens another shell, with its own terminal of `size`, on the SSH
    /// connection of the connected session `of`: no second login, and
    /// nothing of it shows in `of`. It is a session of its own (written
    /// to, acknowledged and closed by its id; never logged), which connects
    /// once the device has started the shell, or closes with
    /// [`OpenError::ChannelRefused`] if the device refuses one; it ends
    /// when the connection does. Closing it leaves `of` open.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn open_channel(
        &self,
        of: SessionId,
        size: TerminalSize,
    ) -> Result<(SessionId, SessionEvents), SessionError> {
        let (target, of) = {
            let sessions = self.inner.sessions();
            let entry = sessions.get(&of).ok_or(SessionError::NotFound)?;
            if entry.target.transport() != Transport::Ssh {
                return Err(SessionError::ChannelsUnsupported);
            }
            // A channel's connection is the session's it is on.
            let of = entry.channel_of.unwrap_or(of);
            let open = open_session(&sessions, of)?;
            if !open.connected.load(Ordering::Acquire) {
                return Err(SessionError::NotConnected);
            }
            (entry.target.clone(), of)
        };
        Ok(self.add(target, OpenOptions::default(), None, Some(of), size))
    }

    /// Adds a session and starts it.
    fn add(
        &self,
        target: SessionTarget,
        options: OpenOptions,
        origin: Option<HostOrigin>,
        channel_of: Option<SessionId>,
        size: TerminalSize,
    ) -> (SessionId, SessionEvents) {
        let id = SessionId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let mut sessions = self.inner.sessions();
        let parent = channel_of.and_then(|of| {
            open_session(&sessions, of)
                .ok()
                .map(|parent| parent.commands.clone())
        });
        sessions.insert(
            id,
            Entry {
                target,
                options,
                origin,
                channel_of,
                open: None,
            },
        );
        let entry = sessions.get_mut(&id).expect("just added");
        let events = self.start(id, entry, size, parent);
        (id, events)
    }

    /// Opens a closed session again, to the same target with the same
    /// settings and login, under the same id: its owner keeps writing to
    /// the same session. Events start again with `Connecting` on the
    /// returned stream (the old one has ended). `size` is the terminal's
    /// size now. A session to a workspace host is reconnected with
    /// [`Self::reconnect_host`] instead, unless its workspace is not open
    /// ([`Reconnection::AsMade`]).
    ///
    /// Must be called from within a Tokio runtime.
    pub fn reconnect(
        &self,
        id: SessionId,
        size: TerminalSize,
    ) -> Result<SessionEvents, SessionError> {
        let mut sessions = self.inner.sessions();
        let entry = sessions.get(&id).ok_or(SessionError::NotFound)?;
        if entry.open.is_some() {
            return Err(SessionError::StillOpen);
        }
        let parent = parent_commands(&sessions, entry.channel_of);
        let entry = sessions.get_mut(&id).expect("just found");
        Ok(self.start(id, entry, size, parent))
    }

    /// Opens the closed session `id` to a workspace host again, in place
    /// as [`Self::reconnect`] does, but to `session`: its target worked out
    /// again ([`Reconnection::work_out`]), with `options` (the log and what
    /// is done once connected, as the host's settings are now). The
    /// session keeps `session`'s origin for the next reconnect.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn reconnect_host(
        &self,
        id: SessionId,
        session: HostSession,
        size: TerminalSize,
        options: OpenOptions,
    ) -> Result<SessionEvents, SessionError> {
        let mut sessions = self.inner.sessions();
        let entry = sessions.get(&id).ok_or(SessionError::NotFound)?;
        if entry.open.is_some() {
            return Err(SessionError::StillOpen);
        }
        let parent = parent_commands(&sessions, entry.channel_of);
        let entry = sessions.get_mut(&id).expect("just found");
        entry.target = session.target;
        entry.options = options;
        entry.origin = Some(session.origin);
        Ok(self.start(id, entry, size, parent))
    }

    /// Opens a new session to the same target as the session `id` (open
    /// or closed), with the same settings and login, logged in the same
    /// place (a second channel: another channel on the same connection).
    /// It keeps the same origin, so it reconnects as the original would.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn duplicate(
        &self,
        id: SessionId,
        size: TerminalSize,
    ) -> Result<(SessionId, SessionEvents), SessionError> {
        let (target, options, origin, channel_of) = {
            let sessions = self.inner.sessions();
            let entry = sessions.get(&id).ok_or(SessionError::NotFound)?;
            // A duplicate forwards nothing: the session it copies does.
            let options = OpenOptions {
                forwards: Vec::new(),
                ..entry.options.clone()
            };
            (
                entry.target.clone(),
                options,
                entry.origin.clone(),
                entry.channel_of,
            )
        };
        Ok(self.add(target, options, origin, channel_of, size))
    }

    /// The hosts of `moves` in the workspace at `root` have new ids (#99):
    /// the sessions to them (open or closed) reconnect to the new ones.
    /// Returns how many sessions it changed.
    pub fn move_hosts(
        &self,
        root: &std::path::Path,
        moves: &[crate::workspace::HostMove],
    ) -> usize {
        self.inner
            .sessions()
            .values_mut()
            .filter_map(|entry| entry.origin.as_mut())
            .map(|origin| origin.follow(root, moves))
            .filter(|followed| *followed)
            .count()
    }

    /// Makes every session (open or closed, not discarded) that logs to
    /// the folder `from` log to `to` from now on: a connection already
    /// logging goes on in its file wherever it was moved to, and the next
    /// ones start there. Returns how many sessions it changed.
    pub fn move_logs(&self, from: &std::path::Path, to: &std::path::Path) -> usize {
        self.inner
            .sessions()
            .values()
            .filter_map(|entry| entry.options.log.as_ref())
            .filter(|log| log.move_folder(from, to))
            .count()
    }

    /// Forgets the session, closing it first if it is open: it can no
    /// longer be reconnected or duplicated, and what it held (its login
    /// included) is dropped. Nothing happens if it is unknown.
    pub fn discard(&self, id: SessionId) {
        let entry = self.inner.sessions().remove(&id);
        if let Some(SessionHandle { commands, .. }) = entry.and_then(|entry| entry.open) {
            let _ = commands.send(Command::Close);
        }
    }

    /// Starts the transport of the session `id` described by `entry`; a
    /// second channel asks for its shell through `parent`, the commands of
    /// the session it is on (`None`: that one is not open).
    fn start(
        &self,
        id: SessionId,
        entry: &mut Entry,
        size: TerminalSize,
        parent: Option<mpsc::UnboundedSender<Command>>,
    ) -> SessionEvents {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        // Unbounded, so the transport never waits on the consumer and keeps
        // taking commands; flow control bounds the output queued here.
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let flow = Arc::new(Flow::new(self.inner.output.window));
        let connected = Arc::new(AtomicBool::new(false));
        let forwards = Arc::new(forward::Forwards::default());
        let watchers = Arc::new(Watchers::new());
        entry.open = Some(SessionHandle {
            commands: command_tx,
            flow: flow.clone(),
            connected: connected.clone(),
            forwards: forwards.clone(),
            watchers: watchers.clone(),
        });

        // The task holds the manager weakly so that dropping the last manager
        // handle drops the command senders, which closes every session.
        let manager = Arc::downgrade(&self.inner);
        let link_flow = flow.clone();
        let known_hosts = self.inner.known_hosts.clone();
        let serial_ports = self.inner.serial_ports.clone();
        let target = entry.target.clone();
        let log = entry.options.log.clone();
        let channel = entry.channel_of.is_some();
        let login_banner = entry.options.login_banner && !channel;
        let forward_options = if channel {
            Vec::new()
        } else {
            entry.options.forwards.clone()
        };
        let when_connected = entry.options.when_connected.clone().map(|hook| {
            let manager = manager.clone();
            Box::new(move |events: &mpsc::UnboundedSender<SessionEvent>| {
                if let Some(inner) = manager.upgrade() {
                    hook.connected(ConnectedSession {
                        manager: SessionManager { inner },
                        id,
                        events: events.downgrade(),
                    });
                }
            }) as OnConnected
        });
        tokio::spawn(async move {
            let mut link = Link {
                commands: command_rx,
                events: event_tx,
                flow: link_flow,
                size,
                log,
                logger: None,
                connected,
                watchers,
                when_connected,
                login_banner,
                forward_options,
                forwards,
            };
            link.state(SessionState::Connecting);
            let reason = match target {
                SessionTarget::Ssh(_) if channel => ssh::run_channel(parent, &mut link).await,
                SessionTarget::Ssh(target) => ssh::run(target, known_hosts, &mut link).await,
                SessionTarget::Telnet(target) => telnet::run(target, known_hosts, &mut link).await,
                SessionTarget::Serial(target) => serial::run(target, serial_ports, &mut link).await,
                SessionTarget::Local(target) => local::run(target, &mut link).await,
            };
            // Mark the session closed before announcing it, so that nobody
            // who has seen `Closed` can still reach it, and anybody can
            // reconnect it.
            if let Some(manager) = manager.upgrade() {
                manager.mark_closed(id);
            }
            link.state(SessionState::Closed(reason));
        });

        SessionEvents::new(event_rx, flow, self.inner.output)
    }

    /// Whether the session `id` (open or closed, until it is discarded) is
    /// encrypted on the network: see [`SessionTarget::encrypted`].
    pub fn encrypted(&self, id: SessionId) -> Result<bool, SessionError> {
        self.target(id, SessionTarget::encrypted)
    }

    /// The transport of the session `id` (open or closed, until it is
    /// discarded).
    pub fn transport(&self, id: SessionId) -> Result<Transport, SessionError> {
        self.target(id, SessionTarget::transport)
    }

    fn target<T>(
        &self,
        id: SessionId,
        get: impl FnOnce(&SessionTarget) -> T,
    ) -> Result<T, SessionError> {
        self.inner
            .sessions()
            .get(&id)
            .map(|entry| get(&entry.target))
            .ok_or(SessionError::NotFound)
    }

    /// Tells the session that `bytes` bytes of its output have been consumed,
    /// for a consumer that does not hold the [`SessionEvents`] itself.
    pub fn acknowledge(&self, id: SessionId, bytes: usize) -> Result<(), SessionError> {
        let sessions = self.inner.sessions();
        let session = open_session(&sessions, id)?;
        session.flow.acknowledge(bytes);
        Ok(())
    }

    /// Sends bytes to the device, for example keystrokes.
    pub fn write(&self, id: SessionId, bytes: Vec<u8>) -> Result<(), SessionError> {
        self.send(id, Command::Write(bytes))
    }

    /// Writes the same input to each of `sessions`, in their order, as
    /// typed in a tab of the multi-send group: one session's failure never
    /// stops the others. A session that is unknown, closed or still
    /// connecting is skipped (input typed while connecting is dropped
    /// anyway), and the result says which took it.
    pub fn write_to_each(&self, sessions: &[SessionId], bytes: &[u8]) -> Fanout {
        let open = self.inner.sessions();
        let mut fanout = Fanout::default();
        for &id in sessions {
            let written = open_session(&open, id).is_ok_and(|session| {
                session.connected.load(Ordering::Acquire)
                    && session
                        .commands
                        .send(Command::Write(bytes.to_vec()))
                        .is_ok()
            });
            if written {
                fanout.written.push(id);
            } else {
                fanout.skipped.push(id);
            }
        }
        fanout
    }

    /// A copy of the output of the connected session `id` from now on,
    /// beside its consumer, until this connection ends (a reconnect is
    /// another connection): see [`OutputWatch`]. For a snippet's steps
    /// waiting for the device (#23).
    pub fn watch_output(&self, id: SessionId) -> Result<OutputWatch, SessionError> {
        let sessions = self.inner.sessions();
        let session = open_session(&sessions, id)?;
        if !session.connected.load(Ordering::Acquire) {
            return Err(SessionError::NotConnected);
        }
        Ok(session.watchers.watch())
    }

    /// Whether the session `id` is open and connected.
    pub fn is_connected(&self, id: SessionId) -> bool {
        open_session(&self.inner.sessions(), id)
            .is_ok_and(|session| session.connected.load(Ordering::Acquire))
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

    /// Sends a break on a serial session's line (held for
    /// [`BREAK_DURATION`]), after the input written before it; for
    /// interrupting a device's boot. Other sessions refuse it.
    pub fn send_break(&self, id: SessionId) -> Result<(), SessionError> {
        let sessions = self.inner.sessions();
        let entry = sessions.get(&id).ok_or(SessionError::NotFound)?;
        if entry.target.transport() != Transport::Serial {
            return Err(SessionError::BreakUnsupported);
        }
        open_session(&sessions, id)?
            .commands
            .send(Command::Break)
            .map_err(|_| SessionError::NotFound)
    }

    /// Closes the session. The `Closed` state follows on its event stream.
    pub fn close(&self, id: SessionId) -> Result<(), SessionError> {
        self.send(id, Command::Close)
    }

    /// The port forwards of the open session `id` ([`OpenOptions::forwards`])
    /// as they are now; empty until it has connected, and once its
    /// connection has ended.
    pub fn forwards(&self, id: SessionId) -> Result<Vec<ForwardStatus>, SessionError> {
        let sessions = self.inner.sessions();
        Ok(open_session(&sessions, id)?.forwards.status())
    }

    /// Every open session that has forwards, with them, in the order of
    /// their ids.
    pub fn all_forwards(&self) -> Vec<(SessionId, Vec<ForwardStatus>)> {
        let sessions = self.inner.sessions();
        let mut all: Vec<_> = sessions
            .iter()
            .filter_map(|(id, entry)| {
                let forwards = entry.open.as_ref()?.forwards.status();
                (!forwards.is_empty()).then_some((*id, forwards))
            })
            .collect();
        all.sort_by_key(|(id, _)| id.0);
        all
    }

    /// Stops the forward `name` of the open session `id`: it listens no
    /// more and the connections it carries end; the session goes on. It
    /// opens again when the session reconnects. `Ok(false)` if the session
    /// has no such forward listening. Once it returns, the port is closed.
    pub async fn stop_forward(&self, id: SessionId, name: &str) -> Result<bool, SessionError> {
        let forwards = {
            let sessions = self.inner.sessions();
            open_session(&sessions, id)?.forwards.clone()
        };
        Ok(forwards.stop(name).await)
    }

    fn send(&self, id: SessionId, command: Command) -> Result<(), SessionError> {
        let sessions = self.inner.sessions();
        open_session(&sessions, id)?
            .commands
            .send(command)
            .map_err(|_| SessionError::NotFound)
    }
}

/// The session `id` if it is open.
fn open_session(
    sessions: &HashMap<SessionId, Entry>,
    id: SessionId,
) -> Result<&SessionHandle, SessionError> {
    sessions
        .get(&id)
        .and_then(|entry| entry.open.as_ref())
        .ok_or(SessionError::NotFound)
}

/// The commands of the open session a second channel is on.
fn parent_commands(
    sessions: &HashMap<SessionId, Entry>,
    channel_of: Option<SessionId>,
) -> Option<mpsc::UnboundedSender<Command>> {
    let session = open_session(sessions, channel_of?).ok()?;
    Some(session.commands.clone())
}

/// What a session's owner asks of its transport.
enum Command {
    Write(Vec<u8>),
    Resize(TerminalSize),
    /// Answers to a login prompt.
    Answer(Vec<String>),
    /// The decision on a host key.
    HostKey(HostKeyDecision),
    /// A break on a serial line.
    Break,
    /// Another shell on this session's SSH connection, for a second
    /// channel; transports without channels drop the reply.
    OpenChannel {
        size: TerminalSize,
        reply: tokio::sync::oneshot::Sender<Result<ssh::OpenedChannel, OpenError>>,
    },
    Close,
}

/// What a session does each time it connects, given that connection's
/// events.
type OnConnected = Box<dyn Fn(&mpsc::UnboundedSender<SessionEvent>) + Send>;

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
    /// Where to log the connection, until its log file is created (on
    /// connecting, or the first output).
    log: Option<SessionLog>,
    logger: Option<Logger>,
    /// Shared with the manager's handle: set once connected.
    connected: Arc<AtomicBool>,
    /// Shared with the manager's handle: given a copy of the output.
    watchers: Arc<Watchers>,
    /// Called on every [`SessionState::Connected`].
    when_connected: Option<OnConnected>,
    /// Whether an SSH session shows its login banners
    /// ([`OpenOptions::login_banner`]).
    login_banner: bool,
    /// The forwards to open once connected ([`OpenOptions::forwards`]).
    forward_options: Vec<PortForward>,
    /// Those forwards, as they run (shared with the manager's handle).
    forwards: Arc<forward::Forwards>,
}

impl Link {
    /// The next command. Losing the session's owner counts as `Close`, and
    /// so does losing its events' reader: a session whose output is paused
    /// (flow control) sends nothing that would tell it otherwise (#118).
    async fn command(&mut self) -> Command {
        tokio::select! {
            command = self.commands.recv() => command.unwrap_or(Command::Close),
            () = self.events.closed() => Command::Close,
        }
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
                // Not connected: a second channel cannot open yet.
                Command::Write(_) | Command::Break | Command::OpenChannel { .. } => {}
            }
        }
    }

    /// What asks the user login questions and host-key questions while the
    /// session is being opened, and the senders [`Link::close_requested`]
    /// passes their replies to.
    fn askers(&self) -> (Prompter, HostKeyAsker, Replies) {
        self.askers_into(&self.events)
    }

    /// [`Link::askers`] whose questions go to `events` instead of straight
    /// to the session's owner: an SSH session passes them on in order with
    /// its login banners ([`Link::opening`]).
    fn askers_into(
        &self,
        events: &mpsc::UnboundedSender<SessionEvent>,
    ) -> (Prompter, HostKeyAsker, Replies) {
        let (answers_tx, answers) = mpsc::unbounded_channel();
        let (decisions_tx, decisions) = mpsc::unbounded_channel();
        let user = UserWait::default();
        let prompter = Prompter {
            events: events.clone(),
            answers,
            jump_host: None,
            user: user.clone(),
            banners: None,
        };
        let asker = HostKeyAsker {
            events: events.clone(),
            decisions,
            user,
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
        self.forward_counted(bytes)
    }

    /// Passes on output that another thread read from the device and
    /// already counted in [`Link::flow`]. Returns `false` if nobody is
    /// listening any more.
    fn forward_counted(&mut self, bytes: Vec<u8>) -> bool {
        self.start_log();
        if let Some(logger) = &mut self.logger {
            if let Err(failure) = logger.write(&bytes) {
                self.logger = None;
                self.log_failed(failure);
            }
        }
        self.watchers.output(&bytes);
        self.events.send(SessionEvent::Output(bytes)).is_ok()
    }

    /// Creates the connection's log file, if it is logged and has none yet:
    /// once connected, or on output that comes earlier.
    fn start_log(&mut self) {
        if let Some(log) = self.log.take() {
            match Logger::start(&log) {
                Ok(logger) => self.logger = Some(logger),
                Err(failure) => self.log_failed(failure),
            }
        }
    }

    fn log_failed(&mut self, failure: LogFailure) {
        let _ = self.events.send(SessionEvent::LogFailed(failure));
    }

    /// Passes on what an SSH session's opening sent through
    /// [`Link::askers_into`]: a login banner as output (counted, logged and
    /// acknowledged like any), anything else (a question) as it is.
    fn opening(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Output(bytes) => {
                self.output(bytes);
            }
            event => {
                let _ = self.events.send(event);
            }
        }
    }

    /// Opens the session's forwards over `device` (an SSH session, once
    /// its shell has started).
    async fn start_forwards(&mut self, device: ssh::DeviceConnection) {
        let options = std::mem::take(&mut self.forward_options);
        self.forwards.start(&options, device, &self.events).await;
    }

    /// Ends the session's forwards: its connection has ended. They listen
    /// no more once this returns.
    async fn end_forwards(&mut self) {
        self.forwards.end().await;
    }

    fn algorithms(&mut self, algorithms: Vec<NegotiatedAlgorithms>) {
        let _ = self.events.send(SessionEvent::Algorithms(algorithms));
    }

    fn state(&mut self, state: SessionState) {
        if let SessionState::Closed(_) = state {
            // Watches end, and the log is complete, before anybody learns
            // the session closed.
            self.watchers.close();
            self.log = None;
            if let Some(Err(failure)) = self.logger.take().map(Logger::finish) {
                self.log_failed(failure);
            }
        }
        let connected = state == SessionState::Connected;
        self.connected.store(connected, Ordering::Release);
        let _ = self.events.send(SessionEvent::State(state));
        if connected {
            self.start_log();
            if let Some(when_connected) = &self.when_connected {
                when_connected(&self.events);
            }
        }
    }
}

/// Connects to the jump chain `jump_hosts` (at least one) with nobody to
/// ask: an untrusted or changed host key is refused, and so is a login
/// that asks questions. The reason names the key or the questions.
pub(crate) async fn open_jump_chain_unattended(
    jump_hosts: &[JumpHost],
    known_hosts: &KnownHosts,
) -> Result<JumpConnections, String> {
    let (events, mut asked) = mpsc::unbounded_channel();
    // Nobody answers: each question gets `None` at once.
    let (_, answers) = mpsc::unbounded_channel();
    let (_, decisions) = mpsc::unbounded_channel();
    let user = UserWait::default();
    let prompter = Prompter {
        events: events.clone(),
        answers,
        jump_host: None,
        user: user.clone(),
        banners: None,
    };
    let asker = HostKeyAsker {
        events,
        decisions,
        user,
    };
    let error = match ssh::open_jump_chain(jump_hosts, known_hosts, asker, prompter).await {
        Ok(chain) => return Ok(chain),
        Err(error) => error,
    };
    let mut reason = error.to_string();
    while let Ok(event) = asked.try_recv() {
        match event {
            SessionEvent::HostKey(question) => {
                reason = format!(
                    "{reason}: its {} key {} (at {} port {}) is {}; connect to it once in the app to check and trust it",
                    question.key.algorithm,
                    question.key.fingerprint,
                    question.host,
                    question.port,
                    if question.changed() {
                        "not the one trusted for it"
                    } else {
                        "not trusted yet"
                    },
                );
            }
            SessionEvent::Prompt(prompt) => {
                let questions: Vec<&str> = prompt
                    .questions
                    .iter()
                    .map(|question| question.text.trim())
                    .collect();
                reason = format!(
                    "{reason}: it asked {}, and nobody can answer here",
                    questions.join(" ")
                );
            }
            _ => {}
        }
    }
    Err(reason)
}

/// Asks the session's owner login questions and waits for the answers.
struct Prompter {
    events: mpsc::UnboundedSender<SessionEvent>,
    answers: mpsc::UnboundedReceiver<Vec<String>>,
    /// The jump host being logged in to, if any: its questions say so in
    /// their [`LoginPrompt::name`], so they are not taken for the device's.
    jump_host: Option<String>,
    /// Says when the session waits for the user's answers.
    user: UserWait,
    /// Where the login banners of the connections it logs in go: `None`
    /// drops them (jobs, file connections, tunnels).
    banners: Option<ssh::Banners>,
}

/// Whether the session being opened waits for the user (to answer login
/// questions or decide on a host key) rather than for the network: the
/// connect timeout stops meanwhile. Shared by the session's askers.
#[derive(Clone)]
struct UserWait(Arc<watch::Sender<bool>>);

impl Default for UserWait {
    fn default() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }
}

impl UserWait {
    /// Follows whether the user is being waited for.
    fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    /// Runs `waiting` (which waits for the user) with the flag set.
    async fn during<T>(&self, waiting: impl std::future::Future<Output = T>) -> T {
        /// Clears the flag however `waiting` ends, dropped included.
        struct Clear<'a>(&'a watch::Sender<bool>);
        impl Drop for Clear<'_> {
            fn drop(&mut self) {
                self.0.send_replace(false);
            }
        }
        self.0.send_replace(true);
        let _clear = Clear(&self.0);
        waiting.await
    }
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
        let mut answers = self.user.during(self.answers.recv()).await?;
        answers.resize(count, String::new());
        Some(answers)
    }

    /// Follows whether the session waits for the user.
    fn user_wait(&self) -> watch::Receiver<bool> {
        self.user.subscribe()
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
    /// Says when the session waits for the user's decision.
    user: UserWait,
}

impl HostKeyAsker {
    /// The decision on `question`; `None` if nobody is listening any more.
    async fn ask(&mut self, question: HostKeyQuestion) -> Option<HostKeyDecision> {
        // Decisions sent before this question was asked were not about it.
        while self.decisions.try_recv().is_ok() {}
        self.events.send(SessionEvent::HostKey(question)).ok()?;
        self.user.during(self.decisions.recv()).await
    }
}
