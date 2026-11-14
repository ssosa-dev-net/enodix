//! The SSH transport: an interactive shell over `russh`.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{
    load_secret_key, Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh::{
    AgentAuthError, Channel, ChannelMsg, ChannelOpenFailure, ChannelWriteHalf, Disconnect,
    MethodKind, MethodSet,
};

use super::algorithms::{self, AlgorithmKind, NegotiationFailure};
use super::recording::{FirstBytes, Recording};
use super::tcp;
use super::{
    agent, AgentError, CloseReason, Command, HostKeyAsker, HostKeyDecision, HostKeyQuestion,
    JumpHost, Link, LoginPrompt, LoginQuestion, NegotiatedAlgorithms, OpenError, Prompter,
    SessionState, SshAuth, SshTarget, TerminalSize, TERM,
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
    let (mut prompter, asker, replies) = link.askers();
    let opened = tokio::select! {
        opened = open_shell(&target, known_hosts, asker, requested_size, &mut prompter) => opened,
        () = link.close_requested(&replies) => return CloseReason::ClosedLocally,
    };
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

    let (mut incoming, outgoing) = channel.split();

    link.algorithms(algorithms);
    link.state(SessionState::Connected);
    if link.size != requested_size && resize(&outgoing, link.size).await.is_err() {
        return CloseReason::ConnectionLost;
    }
    if !early_output.is_empty() && !link.output(early_output) {
        return close(&connection, &jumps).await;
    }

    // Input is sent one piece at a time while output keeps being read. A
    // piece larger than the remote window waits for the device to make room,
    // and the device may not make room until its own output (such as the echo
    // of that input) has been read.
    let mut unsent: VecDeque<Vec<u8>> = VecDeque::new();
    let mut sending: Option<Sending<'_>> = None;
    let mut exit_status = None;
    let flow = link.flow();
    loop {
        if sending.is_none() {
            if let Some(bytes) = unsent.pop_front() {
                sending = Some(Box::pin(outgoing.data_bytes(bytes)));
            }
        }
        tokio::select! {
            sent = finish(&mut sending) => {
                if sent.is_err() {
                    return CloseReason::ConnectionLost;
                }
            }
            command = link.command() => match command {
                Command::Write(bytes) => unsent.push_back(bytes),
                Command::Resize(size) => {
                    if resize(&outgoing, size).await.is_err() {
                        return CloseReason::ConnectionLost;
                    }
                }
                // Logged in already: nothing asked for these.
                Command::Answer(_) | Command::HostKey(_) => {}
                Command::Close => return close(&connection, &jumps).await,
            },
            // Output is read only while the consumer is less than a window
            // behind (flow control); commands are taken regardless.
            message = async {
                flow.room_to_read().await;
                incoming.wait().await
            } => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if !link.output(data.to_vec()) {
                        return close(&connection, &jumps).await;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(ChannelMsg::Close) => {
                    close(&connection, &jumps).await;
                    return CloseReason::RemoteExited { exit_status };
                }
                Some(_) => {}
                // The channel ended without the remote closing it: the
                // connection is gone, unless the shell had already exited.
                None if exit_status.is_some() => return CloseReason::RemoteExited { exit_status },
                None => return CloseReason::ConnectionLost,
            },
        }
    }
}

/// A piece of input on its way to the device.
type Sending<'a> = Pin<Box<dyn Future<Output = Result<(), russh::Error>> + Send + 'a>>;

/// Resolves when the piece being sent, if any, has been sent; never if there
/// is none. Dropping this future does not abandon the piece.
async fn finish(sending: &mut Option<Sending<'_>>) -> Result<(), russh::Error> {
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
        // `connect` asks before authenticating; if anything else still
        // gets here, keyboard-interactive asks the user instead.
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
}

/// The jump host a hop is reached through.
struct Via<'a> {
    name: &'a str,
    connection: &'a Handle<Client>,
}

/// Connects to each jump host in turn, each through the one before, then
/// to the target through the last, and starts the shell there.
async fn open_shell(
    target: &SshTarget,
    known_hosts: KnownHosts,
    asker: HostKeyAsker,
    size: TerminalSize,
    prompter: &mut Prompter,
) -> Result<Shell, OpenError> {
    let asker = Arc::new(Mutex::new(asker));
    // The connections to the jump hosts so far, one per jump host.
    let mut jumps: Vec<Handle<Client>> = Vec::with_capacity(target.jump_hosts.len());
    let mut algorithms = Vec::with_capacity(target.jump_hosts.len() + 1);
    for jump in &target.jump_hosts {
        let hop = Hop {
            jump_host: Some(&jump.name),
            host: &jump.host,
            port: jump.port,
            username: &jump.username,
            auth: &jump.auth,
            legacy_crypto: jump.legacy_crypto,
            connect_timeout: jump.connect_timeout,
        };
        let through = last_via(&target.jump_hosts, &jumps);
        prompter.jump_host = Some(jump.name.clone());
        let (connection, agreed) = connect(hop, through, &known_hosts, &asker, prompter)
            .await
            .map_err(|error| OpenError::JumpHost {
                name: jump.name.clone(),
                error: Box::new(error),
            })?;
        jumps.push(connection);
        algorithms.push(agreed);
    }
    prompter.jump_host = None;
    let hop = Hop {
        jump_host: None,
        host: &target.host,
        port: target.port,
        username: &target.username,
        auth: &target.auth,
        legacy_crypto: target.legacy_crypto,
        connect_timeout: target.connect_timeout,
    };
    let through = last_via(&target.jump_hosts, &jumps);
    let (connection, agreed) = connect(hop, through, &known_hosts, &asker, prompter).await?;
    algorithms.push(agreed);

    let (channel, early_output) = within(
        target.connect_timeout,
        prompter.user_wait(),
        start_shell(&connection, size),
    )
    .await?;

    Ok(Shell {
        connection,
        jumps,
        channel,
        early_output,
        algorithms,
    })
}

/// Opens the shell channel with a pty, and waits for the server to confirm
/// the shell, keeping anything it prints in the meantime.
async fn start_shell(
    connection: &Handle<Client>,
    size: TerminalSize,
) -> Result<(Channel<client::Msg>, Vec<u8>), OpenError> {
    let mut channel = connection.channel_open_session().await.map_err(protocol)?;
    channel
        .request_pty(false, TERM, size.cols, size.rows, 0, 0, &[])
        .await
        .map_err(protocol)?;
    channel.request_shell(true).await.map_err(protocol)?;

    let mut early_output = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok((channel, early_output)),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                early_output.extend_from_slice(&data);
            }
            Some(ChannelMsg::Failure) => {
                return Err(OpenError::Protocol(
                    "the server refused to start a shell".into(),
                ));
            }
            Some(_) => {}
            None => {
                return Err(OpenError::Protocol(
                    "the server closed the connection before starting a shell".into(),
                ));
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
    let client = Client {
        host: hop.host.to_owned(),
        port: hop.port,
        known_hosts: known_hosts.clone(),
        asker: asker.clone(),
        negotiated: Arc::clone(&negotiated),
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
    let asked;
    let (username, login) = match login {
        Login::Ask => {
            asked = ask_login(hop.username, prompter).await?;
            (asked.0.as_str(), Login::Password(&asked.1))
        }
        login => (hop.username, login),
    };
    authenticate(&mut connection, username, login, prompter).await?;
    Ok((connection, agreed))
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
