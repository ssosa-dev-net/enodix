//! The SSH transport: an interactive shell over `russh`.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch, Mutex};

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{
    load_secret_key, Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh::{
    AgentAuthError, Channel, ChannelId, ChannelMsg, ChannelOpenFailure, ChannelWriteHalf,
    Disconnect, MethodKind, MethodSet,
};

use super::algorithms::{self, AlgorithmKind, NegotiationFailure};
use super::connection::Lost;
use super::recording::{FirstBytes, Recording};
use super::tcp;
use super::{
    agent, AgentError, CloseReason, Command, HostKeyAsker, HostKeyDecision, HostKeyQuestion,
    JumpHost, Link, LoginPrompt, LoginQuestion, NegotiatedAlgorithms, OpenError, Prompter,
    SessionEvent, SessionState, SshAuth, SshTarget, TerminalSize, TERM,
};
use crate::known_hosts::{HostKey, HostKeyStatus, KnownHosts, KnownHostsError};

/// How many keepalives (one per connect timeout) may go unanswered before
/// the connection counts as lost.
const KEEPALIVES_UNANSWERED: usize = 2;

/// The client side of the connection: checks the server's host key.
struct Client {
    /// The host and port as the target names them: what keys are known by.
    host: String,
    port: u16,
    known_hosts: KnownHosts,
    /// Asks the user, pausing the connect timeout meanwhile.
    asker: SharedAsker,
    /// The algorithms the last key exchange agreed on.
    negotiated: Arc<std::sync::Mutex<Option<russh::Names>>>,
    /// What a file connection follows of its device's connection.
    followed: Option<Followed>,
    /// Why the connection ended, once it has: what a login cut short
    /// reports (#118).
    ended: EndedWhy,
    /// Where its login banner goes; `None` drops it.
    banners: Option<HopBanners>,
}

/// What a file connection follows of its device's connection, told by the
/// connection's handler.
struct Followed {
    /// Told why the connection ended; dropped (with the handler) when it
    /// has.
    ended: watch::Sender<Option<Lost>>,
    /// The channels the device has ended in order: it sent an exit status,
    /// an EOF or a close on them.
    channels_ended: ChannelsEnded,
}

/// The channels of a connection that its device has ended in order,
/// shared by the connection's handler (which notes them) and its owner.
#[derive(Clone, Default)]
struct ChannelsEnded(Arc<std::sync::Mutex<HashSet<ChannelId>>>);

impl ChannelsEnded {
    fn note(&self, channel: ChannelId) {
        self.channels().insert(channel);
    }

    fn contains(&self, channel: ChannelId) -> bool {
        self.channels().contains(&channel)
    }

    fn channels(&self) -> std::sync::MutexGuard<'_, HashSet<ChannelId>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Client {
    /// The device ended `channel` in order (an exit status, an EOF or a
    /// close on it): noted for a file connection, which tells that apart
    /// from a connection that dropped (IOS-XE ends the connection right
    /// after the channel).
    fn channel_ended(&self, channel: ChannelId) {
        if let Some(followed) = &self.followed {
            followed.channels_ended.note(channel);
        }
    }
}

/// Where a terminal session's login banners go (#134): its opening's
/// events, as output, in order with its questions (see [`run`]). Shared by
/// the handlers of every hop of the session.
#[derive(Clone)]
pub(super) struct Banners {
    events: mpsc::UnboundedSender<SessionEvent>,
    /// Whether a jump host's banner was shown on this opening: the
    /// device's is then named too.
    jump_host_shown: Arc<AtomicBool>,
}

impl Banners {
    fn new(events: mpsc::UnboundedSender<SessionEvent>) -> Self {
        Self {
            events,
            jump_host_shown: Arc::default(),
        }
    }
}

/// The banners of one hop: the device's (`jump_host` `None`), or a jump
/// host's.
struct HopBanners {
    banners: Banners,
    jump_host: Option<String>,
}

impl HopBanners {
    /// Shows `banner` as the device sent it, its bare line ends made
    /// `\r\n` for the terminal and its last line ended, after a dim line
    /// saying whose it is when it is a jump host's (or the device's after
    /// one). The host key of the hop was accepted before it could be sent
    /// (the key exchange comes first), but it is still device output: its
    /// control sequences reach the terminal as they are. It never becomes
    /// a login question: those come only from the protocol.
    fn show(&self, banner: &str) {
        if banner.is_empty() {
            return;
        }
        let mut shown = Vec::with_capacity(banner.len() + 64);
        let whose = match &self.jump_host {
            Some(name) => {
                self.banners.jump_host_shown.store(true, Ordering::Relaxed);
                Some(format!("the jump host {name}"))
            }
            None if self.banners.jump_host_shown.load(Ordering::Relaxed) => {
                Some("the device".to_owned())
            }
            None => None,
        };
        if let Some(whose) = whose {
            shown.extend_from_slice(
                format!("\x1b[2m--- login banner of {whose} ---\x1b[0m\r\n").as_bytes(),
            );
        }
        shown.extend_from_slice(&terminal_line_ends(banner.as_bytes()));
        if !shown.ends_with(b"\n") {
            shown.extend_from_slice(b"\r\n");
        }
        let _ = self.banners.events.send(SessionEvent::Output(shown));
    }
}

/// `text` with every line feed not already after a carriage return
/// preceded by one.
fn terminal_line_ends(text: &[u8]) -> Vec<u8> {
    let mut ended = Vec::with_capacity(text.len() + text.len() / 16);
    let mut previous = None;
    for &byte in text {
        if byte == b'\n' && previous != Some(b'\r') {
            ended.push(b'\r');
        }
        ended.push(byte);
        previous = Some(byte);
    }
    ended
}

/// Asks about the host keys of every hop of a session, one at a time.
type SharedAsker = Arc<Mutex<HostKeyAsker>>;

/// Why the connection could not be set up.
#[derive(Debug)]
enum ClientError {
    Ssh(russh::Error),
    /// The host key was refused, or could not be checked.
    Refused(OpenError),
}

impl From<russh::Error> for ClientError {
    fn from(error: russh::Error) -> Self {
        Self::Ssh(error)
    }
}

impl client::Handler for Client {
    type Error = ClientError;

    /// Shows the login banner, for a terminal session (#134).
    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if let Some(banners) = &self.banners {
            banners.show(banner);
        }
        Ok(())
    }

    /// Trusts a known key; asks the user about an unknown or changed one.
    /// Certificates are never asked for (no certificate algorithm is
    /// offered), so a server sending one is refused.
    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Err(ClientError::Refused(OpenError::Protocol(
                "the server presented a host certificate, which was not asked for".into(),
            )));
        };
        let unusable =
            |error: KnownHostsError| ClientError::Refused(OpenError::KnownHosts(error.to_string()));
        let stored = match self
            .known_hosts
            .check(&self.host, self.port, key)
            .map_err(unusable)?
        {
            HostKeyStatus::Trusted => return Ok(true),
            HostKeyStatus::Unknown => Vec::new(),
            HostKeyStatus::Changed { stored } => stored,
        };
        let question = HostKeyQuestion {
            host: self.host.clone(),
            port: self.port,
            key: HostKey::of(key),
            stored,
        };
        let changed = question.changed();
        let decision = self.asker.lock().await.ask(question).await;
        match decision {
            Some(HostKeyDecision::TrustAndRemember) => {
                self.known_hosts
                    .remember(&self.host, self.port, key)
                    .map_err(unusable)?;
                Ok(true)
            }
            Some(HostKeyDecision::TrustOnce) => Ok(true),
            Some(HostKeyDecision::Cancel) | None => Err(ClientError::Refused(if changed {
                OpenError::HostKeyChanged
            } else {
                OpenError::HostKeyNotTrusted
            })),
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        *self
            .negotiated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(names.clone());
        Ok(())
    }

    /// Notes why an established connection ended, then does as russh's
    /// default does.
    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let (lost, result) = match reason {
            client::DisconnectReason::ReceivedDisconnect(info) => (
                Lost::Closed {
                    message: info.message,
                },
                Ok(()),
            ),
            client::DisconnectReason::Error(error) => {
                let lost = match &error {
                    ClientError::Ssh(russh::Error::KeepaliveTimeout) => Lost::NotAnswering,
                    ClientError::Ssh(error) => Lost::Broken(error.to_string()),
                    ClientError::Refused(error) => Lost::Broken(error.to_string()),
                };
                (lost, Err(error))
            }
        };
        // For a login cut short: the socket closing under it is the device
        // hanging up.
        let hung_up = matches!(
            &result,
            Err(ClientError::Ssh(russh::Error::IO(error))) if matches!(
                error.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
            )
        );
        *self
            .ended
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(if hung_up {
            Lost::Closed {
                message: String::new(),
            }
        } else {
            lost.clone()
        });
        if let Some(followed) = &self.followed {
            followed.ended.send_replace(Some(lost));
        }
        result
    }

    // The three ways a device ends a channel in order.

    async fn exit_status(
        &mut self,
        channel: ChannelId,
        _exit_status: u32,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        self.channel_ended(channel);
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        self.channel_ended(channel);
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        self.channel_ended(channel);
        Ok(())
    }
}

/// An open shell: the connection, its channel, and anything the shell sent
/// before it was confirmed.
struct Shell {
    connection: Handle<Client>,
    /// The connections to the jump hosts, in the order they were made;
    /// kept open as long as the shell.
    jumps: Vec<Handle<Client>>,
    channel: Channel<client::Msg>,
    early_output: Vec<u8>,
    /// What each connection agreed on, jump hosts first.
    algorithms: Vec<NegotiatedAlgorithms>,
}

/// Runs one SSH session over `link` until it ends.
pub(super) async fn run(
    target: SshTarget,
    known_hosts: KnownHosts,
    link: &mut Link,
) -> CloseReason {
    let requested_size = link.size;
    // The questions, and the login banners a terminal shows (#134), come
    // through one channel, so they reach the owner in the order the hops
    // sent them: a banner arrives before the answer to the login request
    // that a question follows.
    let (opening_tx, mut opening) = mpsc::unbounded_channel();
    let (mut prompter, asker, replies) = link.askers_into(&opening_tx);
    if link.login_banner {
        prompter.banners = Some(Banners::new(opening_tx));
    } else {
        drop(opening_tx);
    }
    let opened = {
        let opening_shell = open_shell(&target, known_hosts, asker, requested_size, &mut prompter);
        tokio::pin!(opening_shell);
        loop {
            tokio::select! {
                biased;
                Some(event) = opening.recv() => link.opening(event),
                opened = &mut opening_shell => break opened,
                () = link.close_requested(&replies) => return CloseReason::ClosedLocally,
            }
        }
    };
    // Shown before the session connects, or before it says why it failed.
    while let Ok(event) = opening.try_recv() {
        link.opening(event);
    }
    let Shell {
        connection,
        jumps,
        channel,
        early_output,
        algorithms,
    } = match opened {
        Ok(shell) => shell,
        Err(error) => return CloseReason::Failed(error),
    };

    link.algorithms(algorithms);
    let opened = OpenedChannel {
        channel,
        early_output,
    };
    match pump(link, opened, requested_size, Some(&connection)).await {
        Ended::Here => close(&connection, &jumps).await,
        Ended::Exited(exit_status) => {
            close(&connection, &jumps).await;
            CloseReason::RemoteExited { exit_status }
        }
        Ended::There(reason) => reason,
    }
}

/// Runs a second channel over `link`: asks the session it is on (through
/// `parent`, its commands) for another shell, then carries it until it
/// ends. Closing it closes only the channel.
pub(super) async fn run_channel(
    parent: Option<mpsc::UnboundedSender<Command>>,
    link: &mut Link,
) -> CloseReason {
    let not_open = || {
        CloseReason::Failed(OpenError::Protocol(
            "the session it was to open on is not connected".into(),
        ))
    };
    let requested_size = link.size;
    let (reply, opened) = oneshot::channel();
    let asked = parent.is_some_and(|parent| {
        parent
            .send(Command::OpenChannel {
                size: requested_size,
                reply,
            })
            .is_ok()
    });
    if !asked {
        return not_open();
    }
    let opened = tokio::select! {
        opened = opened => opened,
        () = async {
            loop {
                match link.command().await {
                    Command::Close => return,
                    Command::Resize(size) => link.size = size,
                    _ => {}
                }
            }
        } => return CloseReason::ClosedLocally,
    };
    let opened = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(error)) => return CloseReason::Failed(error),
        // The session ended before opening it.
        Err(_) => {
            return CloseReason::Failed(OpenError::Protocol(
                "the session it was to open on ended first".into(),
            ))
        }
    };
    match pump(link, opened, requested_size, None).await {
        Ended::Here => CloseReason::ClosedLocally,
        Ended::Exited(exit_status) => CloseReason::RemoteExited { exit_status },
        Ended::There(reason) => reason,
    }
}

/// A shell channel just started, and what it printed meanwhile.
pub(super) struct OpenedChannel {
    channel: Channel<client::Msg>,
    early_output: Vec<u8>,
}

/// How [`pump`] ended.
enum Ended {
    /// From this side: the owner closed the session, or went away. A
    /// second channel has been closed.
    Here,
    /// The device closed the channel, with this exit status if it gave one.
    Exited(Option<u32>),
    /// For this reason (the connection was lost, or the shell exited and
    /// the connection ended).
    There(CloseReason),
}

/// [`Ended::Here`], once a second channel (`second`) is closed.
async fn ended_here(outgoing: &ChannelWriteHalf<client::Msg>, second: bool) -> Ended {
    if second {
        let _ = outgoing.close().await;
    }
    Ended::Here
}

/// Carries the shell channel `opened` over `link` (connected from now on)
/// until it ends. `connection`, for the session's own channel, is what
/// second channels are opened on; without it, the channel is a second
/// channel, closed when this side ends it.
async fn pump(
    link: &mut Link,
    opened: OpenedChannel,
    requested_size: TerminalSize,
    connection: Option<&Handle<Client>>,
) -> Ended {
    let OpenedChannel {
        channel,
        early_output,
    } = opened;
    let (mut incoming, outgoing) = channel.split();
    let second = connection.is_none();

    link.state(SessionState::Connected);
    if link.size != requested_size && resize(&outgoing, link.size).await.is_err() {
        return Ended::There(CloseReason::ConnectionLost);
    }
    if !early_output.is_empty() && !link.output(early_output) {
        return ended_here(&outgoing, second).await;
    }

    // Input is sent one piece at a time while output keeps being read. A
    // piece larger than the remote window waits for the device to make room,
    // and the device may not make room until its own output (such as the echo
    // of that input) has been read.
    let mut unsent: VecDeque<Vec<u8>> = VecDeque::new();
    let mut sending: Option<Sending<'_>> = None;
    // Second channels asked for, opened one at a time, each answering its
    // asker when done.
    let mut asked: VecDeque<(TerminalSize, ChannelReply)> = VecDeque::new();
    let mut opening: Option<Opening<'_>> = None;
    let mut exit_status = None;
    let flow = link.flow();
    loop {
        if sending.is_none() {
            if let Some(bytes) = unsent.pop_front() {
                sending = Some(Box::pin(outgoing.data_bytes(bytes)));
            }
        }
        if opening.is_none() {
            if let (Some(connection), Some((size, reply))) = (connection, asked.pop_front()) {
                opening = Some(Box::pin(async move {
                    let _ = reply.send(start_second_shell(connection, size).await);
                }));
            }
        }
        tokio::select! {
            sent = finish(&mut sending) => {
                if sent.is_err() {
                    return Ended::There(CloseReason::ConnectionLost);
                }
            }
            () = finish(&mut opening) => {}
            command = link.command() => match command {
                Command::Write(bytes) => unsent.push_back(bytes),
                Command::Resize(size) => {
                    if resize(&outgoing, size).await.is_err() {
                        return Ended::There(CloseReason::ConnectionLost);
                    }
                }
                // A second channel carries no others (dropping the reply
                // says so).
                Command::OpenChannel { size, reply } => {
                    if connection.is_some() {
                        asked.push_back((size, reply));
                    }
                }
                // Logged in already: nothing asked for these.
                // The manager sends breaks to serial sessions only.
                Command::Answer(_) | Command::HostKey(_) | Command::Break => {}
                Command::Close => return ended_here(&outgoing, second).await,
            },
            // Output is read only while the consumer is less than a window
            // behind (flow control); commands are taken regardless.
            message = async {
                flow.room_to_read().await;
                incoming.wait().await
            } => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if !link.output(data.to_vec()) {
                        return ended_here(&outgoing, second).await;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(ChannelMsg::Close) => return Ended::Exited(exit_status),
                Some(_) => {}
                // The channel ended without the remote closing it: the
                // connection is gone, unless the shell had already exited.
                None if exit_status.is_some() => {
                    return Ended::There(CloseReason::RemoteExited { exit_status })
                }
                None => return Ended::There(CloseReason::ConnectionLost),
            },
        }
    }
}

/// A piece of input on its way to the device.
type Sending<'a> = Pin<Box<dyn Future<Output = Result<(), russh::Error>> + Send + 'a>>;

/// A second channel being opened; answers its asker when done.
type Opening<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Where a second channel, once open, goes.
type ChannelReply = oneshot::Sender<Result<OpenedChannel, OpenError>>;

/// Resolves when the piece being sent (or the work being done), if any, is
/// done; never if there is none. Dropping this future does not abandon it.
async fn finish<T>(sending: &mut Option<Pin<Box<dyn Future<Output = T> + Send + '_>>>) -> T {
    match sending {
        Some(in_flight) => {
            let sent = in_flight.await;
            *sending = None;
            sent
        }
        None => std::future::pending().await,
    }
}

/// The secret to log in with, ready to use.
enum Login<'a> {
    Password(&'a str),
    Key(Arc<PrivateKey>),
    Agent(Vec<agent::Agent>),
    /// Ask the user, once connected.
    Ask,
}

/// Reads the private key, or reaches the agent, if the target logs in with
/// one, before anything goes over the network.
async fn login(auth: &SshAuth) -> Result<Login<'_>, OpenError> {
    match auth {
        SshAuth::Password(password) => Ok(Login::Password(password)),
        SshAuth::Prompt => Ok(Login::Ask),
        SshAuth::Agent(which) => Ok(Login::Agent(
            agent::open(which).await.map_err(OpenError::Agent)?,
        )),
        SshAuth::PrivateKey {
            key_file,
            passphrase,
        } => {
            let unusable = |reason: String| OpenError::PrivateKey {
                file: key_file.display().to_string(),
                reason,
            };
            let passphrase = passphrase.as_deref().filter(|p| !p.is_empty());
            match load_secret_key(key_file, passphrase) {
                Ok(key) => Ok(Login::Key(Arc::new(key))),
                Err(russh::keys::Error::KeyIsEncrypted) => {
                    Err(unusable("it is encrypted and no passphrase is set".into()))
                }
                Err(russh::keys::Error::IO(error)) => Err(unusable(error.to_string())),
                Err(error) if passphrase.is_some() => Err(unusable(format!(
                    "wrong passphrase, or not a private key ({error})"
                ))),
                Err(error) => Err(unusable(error.to_string())),
            }
        }
    }
}

/// Logs in with `login`, then with keyboard-interactive if the server offers
/// it and asks for more (a one-time code after the password), takes no
/// password method (devices that ask for the password that way), or
/// refused the private key (as OpenSSH's client does). A refused password
/// or agent is reported as such instead: the user retries knowing why.
/// Keyboard-interactive questions go to the user through `prompter`,
/// except a lone password question when the password method was not
/// offered: that one gets the target's password.
async fn authenticate(
    connection: &mut Handle<Client>,
    username: &str,
    login: Login<'_>,
    prompter: &mut Prompter,
    ended: &EndedWhy,
) -> Result<(), OpenError> {
    // Learn which methods the server takes (some let anyone in).
    let offered = match connection
        .authenticate_none(username)
        .await
        .map_err(protocol)?
    {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    // Asked now, after the device's login banner (#134).
    let asked;
    let login = match login {
        Login::Ask => {
            asked = ask_login(username, prompter).await?;
            Login::Password(asked.1.as_str())
        }
        login => login,
    };

    let mut password_for_prompt = None;
    let mut agent_error = None;
    // Whether a refusal leads on to keyboard-interactive.
    let mut ask_after_refusal = false;
    let result = match login {
        Login::Password(password) if offered.contains(&MethodKind::Password) => connection
            .authenticate_password(username, password)
            .await
            .map_err(protocol)?,
        Login::Password(password) => {
            password_for_prompt = Some(password);
            ask_after_refusal = true;
            AuthResult::Failure {
                remaining_methods: offered,
                partial_success: false,
            }
        }
        // Asked for above; were it not, keyboard-interactive would ask the
        // user instead.
        Login::Ask => {
            ask_after_refusal = true;
            AuthResult::Failure {
                remaining_methods: offered,
                partial_success: false,
            }
        }
        Login::Key(key) => {
            ask_after_refusal = true;
            let hash = rsa_hash(connection, key.algorithm()).await?;
            connection
                .authenticate_publickey(username, PrivateKeyWithHashAlg::new(key, hash))
                .await
                .map_err(protocol)?
        }
        Login::Agent(agents) => {
            let (result, error) = with_agents(connection, username, agents).await?;
            agent_error = error;
            result
        }
    };
    let (remaining, partial_success) = match result {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => (remaining_methods, partial_success),
    };

    // russh answers a connection that ended as a refusal listing no
    // methods: a device that refuses lists those it takes (#118).
    if remaining.is_empty() && connection.is_closed() {
        return Err(ended_during_login(ended));
    }
    if (partial_success || ask_after_refusal)
        && remaining.contains(&MethodKind::KeyboardInteractive)
    {
        match keyboard_interactive(connection, username, password_for_prompt, prompter).await? {
            Interactive::LoggedIn => return Ok(()),
            Interactive::Refused { asked_user: true } => {
                return Err(OpenError::AuthenticationFailed)
            }
            // Refused without a question: the first refusal is the news.
            Interactive::Refused { asked_user: false } => {}
        }
    }
    Err(agent_error.map_or(OpenError::AuthenticationFailed, OpenError::Agent))
}

/// The hash an RSA key signs with: the best SHA-2 one the server accepts,
/// else SHA-1 (`ssh-rsa`). Other key types have one algorithm.
async fn rsa_hash(
    connection: &Handle<Client>,
    algorithm: Algorithm,
) -> Result<Option<HashAlg>, OpenError> {
    if !algorithm.is_rsa() {
        return Ok(None);
    }
    Ok(connection
        .best_supported_rsa_hash()
        .await
        .map_err(protocol)?
        .flatten())
}

/// Tries each key of each agent in turn. Unless a key is accepted, also
/// gives the reason to report if nothing else works.
async fn with_agents(
    connection: &mut Handle<Client>,
    username: &str,
    agents: Vec<agent::Agent>,
) -> Result<(AuthResult, Option<AgentError>), OpenError> {
    let mut last = AuthResult::Failure {
        remaining_methods: MethodSet::empty(),
        partial_success: false,
    };
    let mut signing_failed = None;
    // The server is asked once, for the first RSA key.
    let mut rsa = None;
    for mut agent in agents {
        for key in agent.keys {
            let hash = match rsa {
                Some(hash) if key.algorithm().is_rsa() => hash,
                _ => {
                    let hash = rsa_hash(connection, key.algorithm()).await?;
                    if key.algorithm().is_rsa() {
                        rsa = Some(hash);
                    }
                    hash
                }
            };
            match connection
                .authenticate_publickey_with(username, key, hash, &mut agent.client)
                .await
            {
                Ok(AuthResult::Success) => return Ok((AuthResult::Success, None)),
                // Accepted, but the server wants more (a code, say).
                Ok(
                    partial @ AuthResult::Failure {
                        partial_success: true,
                        ..
                    },
                ) => return Ok((partial, None)),
                Ok(failure) => last = failure,
                Err(AgentAuthError::Send(_)) => {
                    return Err(OpenError::Protocol("the connection was lost".into()))
                }
                Err(AgentAuthError::Key(error)) => signing_failed = Some(error.to_string()),
            }
        }
    }
    let error = signing_failed.map_or(AgentError::KeysRefused, AgentError::Failed);
    Ok((last, Some(error)))
}

/// How keyboard-interactive authentication ended.
enum Interactive {
    LoggedIn,
    /// Refused, after asking the user something or not.
    Refused {
        asked_user: bool,
    },
}

async fn keyboard_interactive(
    connection: &mut Handle<Client>,
    username: &str,
    mut password: Option<&str>,
    prompter: &mut Prompter,
) -> Result<Interactive, OpenError> {
    let mut asked_user = false;
    let mut reply = connection
        .authenticate_keyboard_interactive_start(username, None)
        .await
        .map_err(protocol)?;
    loop {
        let answers = match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(Interactive::LoggedIn),
            KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Ok(Interactive::Refused { asked_user })
            }
            // Servers may send a round without questions; it needs no one.
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } if prompts.is_empty() => {
                Vec::new()
            }
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. }
                if password.is_some() && asks_only_for_password(&prompts) =>
            {
                vec![password.take().unwrap_or_default().to_owned()]
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                asked_user = true;
                let prompt = LoginPrompt {
                    name,
                    instructions,
                    questions: prompts
                        .into_iter()
                        .map(|prompt| LoginQuestion {
                            text: prompt.prompt,
                            echo: prompt.echo,
                        })
                        .collect(),
                };
                prompter
                    .ask(prompt)
                    .await
                    .ok_or(OpenError::AuthenticationFailed)?
            }
        };
        reply = connection
            .authenticate_keyboard_interactive_respond(answers)
            .await
            .map_err(protocol)?;
    }
}

/// Whether a keyboard-interactive round is one hidden question about a
/// password, as servers that only offer keyboard-interactive ask it.
fn asks_only_for_password(prompts: &[russh::client::Prompt]) -> bool {
    matches!(prompts, [prompt] if !prompt.echo && prompt.prompt.to_lowercase().contains("password"))
}

/// One SSH server on the way to the device: a jump host or the target.
struct Hop<'a> {
    /// The jump host's name; `None` for the device.
    jump_host: Option<&'a str>,
    host: &'a str,
    port: u16,
    username: &'a str,
    auth: &'a SshAuth,
    legacy_crypto: bool,
    connect_timeout: Duration,
    /// For a file connection's device: what the connection is followed by.
    followed: Option<Followed>,
}

/// The jump host a hop is reached through.
struct Via<'a> {
    name: &'a str,
    connection: &'a Handle<Client>,
}

/// A logged-in connection to the device, with its jump connections (in
/// connect order) and what each agreed on, jump hosts first.
type Device = (
    Handle<Client>,
    Vec<Handle<Client>>,
    Vec<NegotiatedAlgorithms>,
);

/// Connects to each jump host in turn, each through the one before, then
/// to the target through the last, and logs in there; `followed` is told
/// why the device's connection ends and which channels the device ends (a
/// file connection's).
async fn connect_device(
    target: &SshTarget,
    known_hosts: KnownHosts,
    asker: HostKeyAsker,
    prompter: &mut Prompter,
    followed: Option<Followed>,
) -> Result<Device, OpenError> {
    let asker = Arc::new(Mutex::new(asker));
    let (jumps, mut algorithms) =
        connect_jumps(&target.jump_hosts, &known_hosts, &asker, prompter).await?;
    let hop = Hop {
        jump_host: None,
        host: &target.host,
        port: target.port,
        username: &target.username,
        auth: &target.auth,
        legacy_crypto: target.legacy_crypto,
        connect_timeout: target.connect_timeout,
        followed,
    };
    let through = last_via(&target.jump_hosts, &jumps);
    let connected = connect(hop, through, &known_hosts, &asker, prompter).await;
    let (connection, agreed) = match connected {
        Ok(connected) => connected,
        Err(error) => return Err(blame_jump_hosts(&target.jump_hosts, &jumps, error).await),
    };
    algorithms.push(agreed);
    Ok((connection, jumps, algorithms))
}

/// Why reaching the hop after `jumps` failed with `error`: a jump host's
/// failure if one is to blame, else `error`. The first jump host (in
/// connect order) whose connection has ended (#118); or, when the hop's
/// wait timed out, the first that does not answer a ping within its own
/// connect timeout (#133): a jump host that went silent makes the next
/// hop's wait end before its keepalives could tell.
async fn blame_jump_hosts(
    jump_hosts: &[JumpHost],
    jumps: &[Handle<Client>],
    error: OpenError,
) -> OpenError {
    if let Some(lost) = lost_jump_host(jump_hosts, jumps) {
        return lost;
    }
    if !matches!(error, OpenError::TimedOut { .. }) {
        return error;
    }
    for (jump, connection) in jump_hosts.iter().zip(jumps) {
        // A ping to a hop crosses the ones before it, which answered.
        let answered = tokio::time::timeout(jump.connect_timeout, connection.send_ping()).await;
        if connection.is_closed() {
            return lost_jump_host(jump_hosts, jumps).unwrap_or(error);
        }
        if answered.is_err() {
            return OpenError::JumpHost {
                name: jump.name.clone(),
                error: Box::new(OpenError::Connect(format!(
                    "it stopped answering while the session was being opened (no answer \
                     within {} s)",
                    jump.connect_timeout.as_secs_f64()
                ))),
            };
        }
    }
    error
}

/// When reaching the device failed because a jump host's connection ended
/// on the way (the device's runs through it): that jump host's failure,
/// the first in connect order whose connection has ended (#118).
fn lost_jump_host(jump_hosts: &[JumpHost], jumps: &[Handle<Client>]) -> Option<OpenError> {
    let (jump, _) = jump_hosts
        .iter()
        .zip(jumps)
        .find(|(_, connection)| connection.is_closed())?;
    Some(OpenError::JumpHost {
        name: jump.name.clone(),
        error: Box::new(OpenError::Connect(
            "its connection ended while the session logged in to the device".into(),
        )),
    })
}

/// [`connect_device`], then starts the shell there.
async fn open_shell(
    target: &SshTarget,
    known_hosts: KnownHosts,
    asker: HostKeyAsker,
    size: TerminalSize,
    prompter: &mut Prompter,
) -> Result<Shell, OpenError> {
    let (connection, jumps, algorithms) =
        connect_device(target, known_hosts, asker, prompter, None).await?;

    let started = within(
        target.connect_timeout,
        prompter.user_wait(),
        start_shell(&connection, size),
    )
    .await;
    let (channel, early_output) = match started {
        Ok(started) => started,
        Err(error) => return Err(blame_jump_hosts(&target.jump_hosts, &jumps, error).await),
    };

    Ok(Shell {
        connection,
        jumps,
        channel,
        early_output,
        algorithms,
    })
}

/// Connects to each jump host in turn, each through the one before: the
/// connections, and what each agreed on.
async fn connect_jumps(
    jump_hosts: &[JumpHost],
    known_hosts: &KnownHosts,
    asker: &SharedAsker,
    prompter: &mut Prompter,
) -> Result<(Vec<Handle<Client>>, Vec<NegotiatedAlgorithms>), OpenError> {
    let mut jumps: Vec<Handle<Client>> = Vec::with_capacity(jump_hosts.len());
    let mut algorithms = Vec::with_capacity(jump_hosts.len() + 1);
    for jump in jump_hosts {
        let hop = Hop {
            jump_host: Some(&jump.name),
            host: &jump.host,
            port: jump.port,
            username: &jump.username,
            auth: &jump.auth,
            legacy_crypto: jump.legacy_crypto,
            connect_timeout: jump.connect_timeout,
            followed: None,
        };
        let through = last_via(jump_hosts, &jumps);
        prompter.jump_host = Some(jump.name.clone());
        let connected = connect(hop, through, known_hosts, asker, prompter).await;
        let (connection, agreed) = match connected {
            Ok(connected) => connected,
            Err(error) => {
                let own = OpenError::JumpHost {
                    name: jump.name.clone(),
                    error: Box::new(error.clone()),
                };
                // Unless a jump host before it is to blame.
                let blamed = blame_jump_hosts(jump_hosts, &jumps, error).await;
                return Err(match blamed {
                    blamed @ OpenError::JumpHost { .. } => blamed,
                    _ => own,
                });
            }
        };
        jumps.push(connection);
        algorithms.push(agreed);
    }
    prompter.jump_host = None;
    Ok((jumps, algorithms))
}

/// Open connections to a chain of jump hosts, through the last of which
/// TCP connections are forwarded (the tunnels of [`crate::tunnel`]).
/// Dropping it disconnects them.
pub(crate) struct JumpConnections {
    /// The jump hosts' names, in connect order.
    names: Vec<String>,
    connections: Vec<Handle<Client>>,
}

/// Connects to the chain `jump_hosts` (at least one), asking `asker` and
/// `prompter` what a terminal session would ask the user.
pub(super) async fn open_jump_chain(
    jump_hosts: &[JumpHost],
    known_hosts: &KnownHosts,
    asker: HostKeyAsker,
    mut prompter: Prompter,
) -> Result<JumpConnections, OpenError> {
    let asker = Arc::new(Mutex::new(asker));
    let (connections, _) = connect_jumps(jump_hosts, known_hosts, &asker, &mut prompter).await?;
    Ok(JumpConnections {
        names: jump_hosts.iter().map(|jump| jump.name.clone()).collect(),
        connections,
    })
}

impl JumpConnections {
    /// A channel to `host:port` as the last jump host reaches them,
    /// refused if it does not answer within `timeout`.
    pub(crate) async fn forward(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<Channel<client::Msg>, OpenError> {
        let (Some(last), Some(name)) = (self.connections.last(), self.names.last()) else {
            return Err(OpenError::Connect("there is no jump host".into()));
        };
        let opening = last.channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0);
        match tokio::time::timeout(timeout, opening).await {
            Ok(Ok(channel)) => Ok(channel),
            Ok(Err(error)) => Err(OpenError::Connect(format!(
                "the jump host {name} could not reach it ({})",
                forwarding_failure(&error)
            ))),
            Err(_) => Err(OpenError::TimedOut { after: timeout }),
        }
    }

    /// The first jump host whose connection has ended, if any.
    pub(crate) fn lost(&self) -> Option<&str> {
        self.names
            .iter()
            .zip(&self.connections)
            .find(|(_, connection)| connection.is_closed())
            .map(|(name, _)| name.as_str())
    }
}

/// Opens the shell channel with a pty, and waits for the server to confirm
/// the shell, keeping anything it prints in the meantime.
async fn start_shell(
    connection: &Handle<Client>,
    size: TerminalSize,
) -> Result<(Channel<client::Msg>, Vec<u8>), OpenError> {
    let channel = connection
        .channel_open_session()
        .await
        .map_err(|error| match error {
            // Not a refusal: the connection ended (#118).
            _ if connection.is_closed() => OpenError::Protocol(CLOSED_BEFORE_SHELL.into()),
            error => protocol(error),
        })?;
    shell_on(channel, size)
        .await
        .map_err(|refusal| match refusal {
            ShellRefusal::Refused(reason) => OpenError::Protocol(reason),
            ShellRefusal::Failed(error) => error,
        })
}

/// [`start_shell`] for a second channel on `connection`: a device that
/// will not open one, or will not start a shell on it, refused it
/// ([`OpenError::ChannelRefused`]).
async fn start_second_shell(
    connection: &Handle<Client>,
    size: TerminalSize,
) -> Result<OpenedChannel, OpenError> {
    let channel = match connection.channel_open_session().await {
        Ok(channel) => channel,
        Err(russh::Error::ChannelOpenFailure(reason)) => {
            return Err(OpenError::ChannelRefused {
                reason: format!("it answered {reason:?}"),
            })
        }
        Err(error) => return Err(protocol(error)),
    };
    match shell_on(channel, size).await {
        Ok((channel, early_output)) => Ok(OpenedChannel {
            channel,
            early_output,
        }),
        Err(ShellRefusal::Refused(reason)) => Err(OpenError::ChannelRefused { reason }),
        Err(ShellRefusal::Failed(error)) => Err(error),
    }
}

/// What a device that hung up between the login and the shell reports.
const CLOSED_BEFORE_SHELL: &str = "the server closed the connection before starting a shell";

/// Why a shell did not start on a channel.
enum ShellRefusal {
    /// The server said no, or closed the channel instead.
    Refused(String),
    Failed(OpenError),
}

/// Asks for a pty and a shell on `channel` and waits for the server to
/// confirm the shell, keeping anything it prints in the meantime.
async fn shell_on(
    mut channel: Channel<client::Msg>,
    size: TerminalSize,
) -> Result<(Channel<client::Msg>, Vec<u8>), ShellRefusal> {
    let failed = |error| ShellRefusal::Failed(protocol(error));
    channel
        .request_pty(false, TERM, size.cols, size.rows, 0, 0, &[])
        .await
        .map_err(failed)?;
    channel.request_shell(true).await.map_err(failed)?;

    let mut early_output = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok((channel, early_output)),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                early_output.extend_from_slice(&data);
            }
            Some(ChannelMsg::Failure) => {
                return Err(ShellRefusal::Refused(
                    "the server refused to start a shell".into(),
                ));
            }
            Some(ChannelMsg::Close) => {
                return Err(ShellRefusal::Refused(
                    "the server closed the channel instead of starting a shell".into(),
                ));
            }
            Some(_) => {}
            None => {
                return Err(ShellRefusal::Failed(OpenError::Protocol(
                    CLOSED_BEFORE_SHELL.into(),
                )));
            }
        }
    }
}

/// The last jump host connected so far, to reach the next hop through.
fn last_via<'a>(jump_hosts: &'a [JumpHost], connections: &'a [Handle<Client>]) -> Option<Via<'a>> {
    let last = connections.len().checked_sub(1)?;
    Some(Via {
        name: &jump_hosts[last].name,
        connection: &connections[last],
    })
}

/// Connects to `hop` (directly, or through the jump host `via`), checks
/// its host key and logs in, within the hop's connect timeout. Also gives
/// the algorithms agreed on.
async fn connect(
    hop: Hop<'_>,
    via: Option<Via<'_>>,
    known_hosts: &KnownHosts,
    asker: &SharedAsker,
    prompter: &mut Prompter,
) -> Result<(Handle<Client>, NegotiatedAlgorithms), OpenError> {
    let user = prompter.user_wait();
    within(
        hop.connect_timeout,
        user,
        connect_and_log_in(hop, via, known_hosts, asker, prompter),
    )
    .await
}

/// [`connect`] without the timeout.
async fn connect_and_log_in(
    hop: Hop<'_>,
    via: Option<Via<'_>>,
    known_hosts: &KnownHosts,
    asker: &SharedAsker,
    prompter: &mut Prompter,
) -> Result<(Handle<Client>, NegotiatedAlgorithms), OpenError> {
    let login = login(hop.auth).await?;
    let config = Arc::new(client::Config {
        preferred: algorithms::preferred(hop.legacy_crypto),
        // A link that goes silent is lost once the device has answered no
        // keepalive for about three connect timeouts.
        keepalive_interval: Some(hop.connect_timeout),
        keepalive_max: KEEPALIVES_UNANSWERED,
        ..client::Config::default()
    });
    let negotiated = Arc::default();
    let ended = EndedWhy::default();
    let client = Client {
        host: hop.host.to_owned(),
        port: hop.port,
        known_hosts: known_hosts.clone(),
        asker: asker.clone(),
        negotiated: Arc::clone(&negotiated),
        followed: hop.followed,
        ended: Arc::clone(&ended),
        banners: prompter.banners.clone().map(|banners| HopBanners {
            banners,
            jump_host: hop.jump_host.map(str::to_owned),
        }),
    };
    // What the server sends first, for reporting a failed negotiation.
    let first_bytes = FirstBytes::new();
    let connected = async {
        let connected = match via {
            None => {
                let socket = tcp::connect(hop.host, hop.port, hop.connect_timeout).await?;
                let stream = Recording::new(socket, first_bytes.clone());
                client::connect_stream(config, stream, client).await
            }
            Some(via) => {
                let channel = via
                    .connection
                    .channel_open_direct_tcpip(hop.host, u32::from(hop.port), "127.0.0.1", 0)
                    .await
                    .map_err(|error| {
                        OpenError::Connect(format!(
                            "the jump host {} could not reach it ({})",
                            via.name,
                            forwarding_failure(&error)
                        ))
                    })?;
                let stream = Recording::new(channel.into_stream(), first_bytes.clone());
                client::connect_stream(config, stream, client).await
            }
        };
        match connected {
            Ok(connection) => Ok(connection),
            Err(ClientError::Refused(error)) => Err(error),
            // Not a byte from the device: it hung up before its banner.
            Err(ClientError::Ssh(russh::Error::IO(_) | russh::Error::Disconnect))
                if first_bytes.is_empty() =>
            {
                Err(OpenError::ClosedBeforeLogin)
            }
            Err(ClientError::Ssh(russh::Error::IO(error))) => Err(handshake_broke(&error)),
            Err(ClientError::Ssh(russh::Error::NoCommonAlgo { kind, theirs, .. })) => Err(
                negotiation_failure(kind, theirs, &first_bytes.take(), hop.legacy_crypto),
            ),
            Err(ClientError::Ssh(error)) => Err(protocol(error)),
        }
    }
    .await;
    // Needed only for a failed negotiation: stop keeping what arrives.
    drop(first_bytes.take());
    let mut connection = connected?;
    let agreed = negotiated
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|names| NegotiatedAlgorithms::from_names(hop.jump_host.map(str::to_owned), names))
        .ok_or_else(|| OpenError::Protocol("the key exchange did not finish".into()))?;
    // With no username, it is asked for with the password, before the
    // device is asked for a login: the username cannot change after that.
    // With one, `authenticate` asks for the password once the device has
    // answered a first login request, which is when it sends its banner.
    let asked;
    let (username, login) = match login {
        Login::Ask if hop.username.is_empty() => {
            asked = ask_login(hop.username, prompter).await?;
            (asked.0.as_str(), Login::Password(&asked.1))
        }
        login => (hop.username, login),
    };
    authenticate(&mut connection, username, login, prompter, &ended)
        .await
        .map_err(|error| match error {
            // A request that could not be sent or answered (#118).
            OpenError::Protocol(_) if connection.is_closed() => ended_during_login(&ended),
            error => error,
        })?;
    Ok((connection, agreed))
}

/// Why a connection ended, once it has (told by its handler).
type EndedWhy = Arc<std::sync::Mutex<Option<Lost>>>;

/// What a connection that ended while the session logged in reports, by
/// why it ended (if russh said).
fn ended_during_login(ended: &EndedWhy) -> OpenError {
    let ended = ended
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    OpenError::Connect(match ended {
        Some(Lost::Broken(reason)) => format!("the connection broke during the login ({reason})"),
        Some(Lost::NotAnswering) => "the device stopped answering during the login".into(),
        Some(Lost::Closed { message }) if !message.trim().is_empty() => format!(
            "the device closed the connection during the login ({})",
            message.trim()
        ),
        _ => "the device closed the connection during the login".into(),
    })
}

/// What a failed negotiation reports: `russh`'s failure for the first kind
/// of algorithm nothing was in common for (`theirs` being the device's list
/// of that kind), and the device's whole offer from what it sent first, if
/// that could be read.
fn negotiation_failure(
    kind: russh::AlgorithmKind,
    theirs: Vec<String>,
    first_bytes: &[u8],
    legacy_crypto: bool,
) -> OpenError {
    let kind = match kind {
        russh::AlgorithmKind::Kex => AlgorithmKind::KeyExchange,
        russh::AlgorithmKind::Key => AlgorithmKind::HostKey,
        russh::AlgorithmKind::Cipher => AlgorithmKind::Cipher,
        russh::AlgorithmKind::Mac => AlgorithmKind::Mac,
        // Every device takes `none`, which is always offered.
        russh::AlgorithmKind::Compression => {
            return OpenError::Protocol(format!(
                "no compression in common with the device (it offered {})",
                theirs.join(", ")
            ))
        }
    };
    let offered = algorithms::parse_offer(first_bytes).unwrap_or_else(|| {
        // Only the one list is known.
        let mut offered = algorithms::AlgorithmLists::default();
        match kind {
            AlgorithmKind::KeyExchange => offered.key_exchange = theirs,
            AlgorithmKind::HostKey => offered.host_key = theirs,
            AlgorithmKind::Cipher => offered.cipher = theirs,
            AlgorithmKind::Mac => offered.mac = theirs,
        }
        offered
    });
    OpenError::Negotiation(NegotiationFailure::new(kind, offered, legacy_crypto))
}

/// Asks the user for the password, and for the username if `username` is
/// empty: the username and password to log in with.
async fn ask_login(username: &str, prompter: &mut Prompter) -> Result<(String, String), OpenError> {
    let mut questions = Vec::new();
    if username.is_empty() {
        questions.push(LoginQuestion {
            text: "Username: ".into(),
            echo: true,
        });
    }
    questions.push(LoginQuestion {
        text: "Password: ".into(),
        echo: false,
    });
    let mut answers = prompter
        .ask(LoginPrompt {
            name: String::new(),
            instructions: String::new(),
            questions,
        })
        .await
        .ok_or(OpenError::AuthenticationFailed)?;
    let password = answers.pop().unwrap_or_default();
    let username = answers
        .pop()
        .map_or_else(|| username.to_owned(), |given| given.trim().to_owned());
    Ok((username, password))
}

/// Why a jump host did not forward a connection, in words.
fn forwarding_failure(error: &russh::Error) -> String {
    match error {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => {
            "the connection failed".into()
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            "it does not allow forwarding".into()
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::Other { reason, .. }) => {
            reason.clone()
        }
        russh::Error::ChannelOpenFailure(failure) => format!("{failure:?}"),
        other => other.to_string(),
    }
}

/// What a broken connection during the SSH handshake reports.
fn handshake_broke(error: &io::Error) -> OpenError {
    match error.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted => {
            OpenError::Connect("the device closed the connection during the SSH handshake".into())
        }
        _ => OpenError::Connect(error.to_string()),
    }
}

/// Runs `work`, unless it waits on the network for `timeout` first: that
/// is [`OpenError::TimedOut`].
async fn within<T>(
    timeout: Duration,
    user: watch::Receiver<bool>,
    work: impl Future<Output = Result<T, OpenError>>,
) -> Result<T, OpenError> {
    tokio::select! {
        done = work => done,
        () = deadline(user, timeout) => Err(OpenError::TimedOut { after: timeout }),
    }
}

/// Resolves once `timeout` has passed without the user being waited for
/// (`user` true while they are asked something): the clock stops while
/// they are, and starts again from zero after they answer.
async fn deadline(mut user: watch::Receiver<bool>, timeout: Duration) {
    loop {
        // Gone means nobody can be asked any more: only the clock counts.
        if user.wait_for(|waiting| !*waiting).await.is_err() {
            return tokio::time::sleep(timeout).await;
        }
        tokio::select! {
            () = tokio::time::sleep(timeout) => return,
            changed = user.changed() => {
                if changed.is_err() {
                    return tokio::time::sleep(timeout).await;
                }
            }
        }
    }
}

fn protocol(error: russh::Error) -> OpenError {
    OpenError::Protocol(error.to_string())
}

async fn resize(
    channel: &ChannelWriteHalf<client::Msg>,
    size: TerminalSize,
) -> Result<(), russh::Error> {
    channel.window_change(size.cols, size.rows, 0, 0).await
}

/// Disconnects from this side: from the device, then from each jump host,
/// the nearest first.
async fn close(connection: &Handle<Client>, jumps: &[Handle<Client>]) -> CloseReason {
    for connection in std::iter::once(connection).chain(jumps.iter().rev()) {
        let _ = connection
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
    CloseReason::ClosedLocally
}

/// A connection to the device, through its jump hosts, that is logged in
/// and has opened no channel: a file connection's.
pub(super) struct Connected {
    /// The device's connection first, then the jump hosts', the nearest
    /// first: the order they are closed in.
    connections: Vec<Handle<Client>>,
    /// The jump hosts' names, in connect order.
    jump_names: Vec<String>,
    /// What each hop agreed on, jump hosts first.
    pub(super) algorithms: Vec<NegotiatedAlgorithms>,
    /// Why the device's connection ended, once it has.
    pub(super) ended: watch::Receiver<Option<Lost>>,
    /// The channels the device has ended in order.
    channels_ended: ChannelsEnded,
    /// The device's connect timeout: the wait for a channel.
    pub(super) connect_timeout: Duration,
}

/// [`open_shell`] without the shell: [`connect_device`], watching why the
/// device's connection ends.
pub(super) async fn open_connection(
    target: &SshTarget,
    known_hosts: KnownHosts,
    asker: HostKeyAsker,
    prompter: &mut Prompter,
) -> Result<Connected, OpenError> {
    let (ended_tx, ended) = watch::channel(None);
    let channels_ended = ChannelsEnded::default();
    let followed = Followed {
        ended: ended_tx,
        channels_ended: channels_ended.clone(),
    };
    let (connection, jumps, algorithms) =
        connect_device(target, known_hosts, asker, prompter, Some(followed)).await?;
    let mut connections = vec![connection];
    connections.extend(jumps.into_iter().rev());
    Ok(Connected {
        connections,
        jump_names: target.jump_hosts.iter().map(|j| j.name.clone()).collect(),
        algorithms,
        ended,
        channels_ended,
        connect_timeout: target.connect_timeout,
    })
}

/// What a request that never reached the device reports.
const CONNECTION_LOST: &str = "the connection was lost";

/// Why a subsystem did not start on a new channel.
pub(crate) enum SubsystemRefusal {
    /// The device said no.
    Refused,
    /// The device closed the channel in order instead of answering.
    Closed,
    /// The channel ended without a word from the device: the device's
    /// doing only if the connection is still up.
    Ended,
    /// The connection failed, or the device did not answer.
    Failed(String),
}

impl Connected {
    /// A channel running the subsystem `name` (`sftp`), once the device has
    /// accepted it.
    pub(super) async fn subsystem(
        &self,
        name: &str,
    ) -> Result<Channel<client::Msg>, SubsystemRefusal> {
        let starting =
            async {
                let mut channel = self.connections[0].channel_open_session().await.map_err(
                    |error| match error {
                        russh::Error::ChannelOpenFailure(reason) => SubsystemRefusal::Failed(
                            format!("the device refused to open a channel ({reason:?})"),
                        ),
                        _ => SubsystemRefusal::Failed(CONNECTION_LOST.into()),
                    },
                )?;
                channel
                    .request_subsystem(true, name)
                    .await
                    .map_err(|_| SubsystemRefusal::Failed(CONNECTION_LOST.into()))?;
                loop {
                    match channel.wait().await {
                        Some(ChannelMsg::Success) => return Ok(channel),
                        Some(ChannelMsg::Failure) => return Err(SubsystemRefusal::Refused),
                        Some(ChannelMsg::Close | ChannelMsg::Eof) => {
                            return Err(SubsystemRefusal::Closed)
                        }
                        Some(_) => {}
                        None => {
                            // Read in this order: once the connection is
                            // closed, what the device ended is final.
                            let closed = self.connections[0].is_closed();
                            return Err(if self.channel_ended_in_order(channel.id()) {
                                // An exit status and nothing more.
                                SubsystemRefusal::Closed
                            } else if closed {
                                SubsystemRefusal::Failed(CONNECTION_LOST.into())
                            } else {
                                SubsystemRefusal::Ended
                            });
                        }
                    }
                }
            };
        match tokio::time::timeout(self.connect_timeout, starting).await {
            Ok(started) => started,
            Err(_) => Err(SubsystemRefusal::Failed(
                OpenError::TimedOut {
                    after: self.connect_timeout,
                }
                .to_string(),
            )),
        }
    }

    /// A channel running `command` (`scp -t flash:x`). Whether the device
    /// accepted it is left to what comes on the channel: some devices never
    /// answer the request, and a refusal arrives there as `Failure`.
    pub(super) async fn exec(&self, command: &str) -> Result<Channel<client::Msg>, String> {
        let starting = async {
            let channel = self.connections[0]
                .channel_open_session()
                .await
                .map_err(|error| match error {
                    russh::Error::ChannelOpenFailure(reason) => {
                        format!("the device refused to open a channel ({reason:?})")
                    }
                    _ => CONNECTION_LOST.into(),
                })?;
            channel
                .exec(true, command)
                .await
                .map_err(|_| String::from(CONNECTION_LOST))?;
            Ok(channel)
        };
        match tokio::time::timeout(self.connect_timeout, starting).await {
            Ok(started) => started,
            Err(_) => Err(OpenError::TimedOut {
                after: self.connect_timeout,
            }
            .to_string()),
        }
    }

    /// Whether the device's connection is still up.
    pub(super) fn is_open(&self) -> bool {
        !self.connections.is_empty() && !self.connections[0].is_closed()
    }

    /// Whether the device has ended `channel` in order (an exit status, an
    /// EOF or a close on it). Once the connection has ended this is final:
    /// everything the device sent before was noted first.
    pub(super) fn channel_ended_in_order(&self, channel: ChannelId) -> bool {
        self.channels_ended.contains(channel)
    }

    /// The first jump host whose connection has ended, if any.
    pub(super) fn lost_jump_host(&self) -> Option<&str> {
        // The jump hosts' connections follow the device's, nearest first.
        let jumps = self.connections.iter().skip(1).rev();
        self.jump_names
            .iter()
            .zip(jumps)
            .find(|(_, connection)| connection.is_closed())
            .map(|(name, _)| name.as_str())
    }

    /// Closing every connection, the device's first, then the jump
    /// hosts', the nearest first: `None` if they were already. None are
    /// left to close afterwards.
    pub(super) fn closing(&mut self) -> Option<impl Future<Output = ()> + Send + 'static> {
        let connections = std::mem::take(&mut self.connections);
        if connections.is_empty() {
            return None;
        }
        Some(async move {
            for connection in connections {
                let _ = connection
                    .disconnect(Disconnect::ByApplication, "", "en")
                    .await;
            }
        })
    }
}
