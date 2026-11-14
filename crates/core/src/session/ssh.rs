//! The SSH transport: an interactive shell over `russh`.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{
    load_secret_key, Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh::{
    AgentAuthError, Channel, ChannelMsg, ChannelWriteHalf, Disconnect, MethodKind, MethodSet,
};

use super::{
    agent, AgentError, CloseReason, Command, HostKeyAsker, HostKeyDecision, HostKeyQuestion, Link,
    LoginPrompt, LoginQuestion, OpenError, Prompter, SessionState, SshAuth, SshTarget,
    TerminalSize,
};
use crate::known_hosts::{HostKey, HostKeyStatus, KnownHosts, KnownHostsError};

/// Terminal type requested for the remote pty; xterm.js emulates xterm.
const TERM: &str = "xterm-256color";

/// How long to wait for the TCP connection and SSH handshake together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The client side of the connection: checks the server's host key.
struct Client {
    /// The host and port as the target names them: what keys are known by.
    host: String,
    port: u16,
    known_hosts: KnownHosts,
    asker: HostKeyAsker,
    /// True while the user is asked about the key: the handshake then
    /// waits for them, not for the network, and the timeout is paused.
    asking: watch::Sender<bool>,
}

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
        self.asking.send_replace(true);
        let decision = self.asker.ask(question).await;
        self.asking.send_replace(false);
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
}

/// An open shell: the connection, its channel, and anything the shell sent
/// before it was confirmed.
struct Shell {
    connection: Handle<Client>,
    channel: Channel<client::Msg>,
    early_output: Vec<u8>,
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
        channel,
        early_output,
    } = match opened {
        Ok(shell) => shell,
        Err(error) => return CloseReason::Failed(error),
    };

    let (mut incoming, outgoing) = channel.split();

    link.state(SessionState::Connected);
    if link.size != requested_size && resize(&outgoing, link.size).await.is_err() {
        return CloseReason::ConnectionLost;
    }
    if !early_output.is_empty() && !link.output(early_output) {
        return close(&connection).await;
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
                Command::Close => return close(&connection).await,
            },
            // Output is read only while the consumer is less than a window
            // behind (flow control); commands are taken regardless.
            message = async {
                flow.room_to_read().await;
                incoming.wait().await
            } => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if !link.output(data.to_vec()) {
                        return close(&connection).await;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(ChannelMsg::Close) => {
                    close(&connection).await;
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
}

/// Reads the private key, or reaches the agent, if the target logs in with
/// one, before anything goes over the network.
async fn login(auth: &SshAuth) -> Result<Login<'_>, OpenError> {
    match auth {
        SshAuth::Password(password) => Ok(Login::Password(password)),
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

async fn open_shell(
    target: &SshTarget,
    known_hosts: KnownHosts,
    asker: HostKeyAsker,
    size: TerminalSize,
    prompter: &mut Prompter,
) -> Result<Shell, OpenError> {
    let login = login(&target.auth).await?;
    let config = Arc::new(client::Config::default());
    let address = (target.host.as_str(), target.port);
    let (asking, asking_rx) = watch::channel(false);
    let client = Client {
        host: target.host.clone(),
        port: target.port,
        known_hosts,
        asker,
        asking,
    };
    let mut connection = tokio::select! {
        connected = client::connect(config, address, client) => match connected {
            Ok(connection) => connection,
            Err(ClientError::Refused(error)) => return Err(error),
            Err(ClientError::Ssh(russh::Error::IO(error))) => {
                return Err(OpenError::Connect(error.to_string()))
            }
            Err(ClientError::Ssh(error)) => return Err(protocol(error)),
        },
        () = handshake_deadline(asking_rx) => return Err(OpenError::Connect("timed out".into())),
    };

    authenticate(&mut connection, &target.username, login, prompter).await?;

    let mut channel = connection.channel_open_session().await.map_err(protocol)?;
    channel
        .request_pty(false, TERM, size.cols, size.rows, 0, 0, &[])
        .await
        .map_err(protocol)?;
    channel.request_shell(true).await.map_err(protocol)?;

    // Wait for the server to confirm the shell, keeping anything it prints
    // in the meantime.
    let mut early_output = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => break,
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

    Ok(Shell {
        connection,
        channel,
        early_output,
    })
}

/// Resolves when the connection and handshake have taken
/// [`CONNECT_TIMEOUT`], not counting the time the user takes to decide on a
/// host key: the clock stops while `asking` is true and starts again from
/// zero after the decision.
async fn handshake_deadline(mut asking: watch::Receiver<bool>) {
    loop {
        // Gone means the handshake is over: nothing to time out.
        if asking.wait_for(|asking| !*asking).await.is_err() {
            return std::future::pending().await;
        }
        tokio::select! {
            () = tokio::time::sleep(CONNECT_TIMEOUT) => return,
            changed = asking.changed() => {
                if changed.is_err() {
                    return std::future::pending().await;
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

/// Disconnects from this side.
async fn close(connection: &Handle<Client>) -> CloseReason {
    let _ = connection
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    CloseReason::ClosedLocally
}
