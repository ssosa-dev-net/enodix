//! Session test harness: an SSH server that runs inside the test process.
//!
//! The spec's session tests use a containerised OpenSSH server; this harness
//! is the fallback for machines without Docker. It speaks the real SSH
//! protocol (key exchange, authentication, channels, pty and window-change
//! requests) through `russh`'s server side, with a small scripted shell
//! behind it instead of a real one.
//!
//! The scripted shell echoes input like a tty and understands:
//!
//! | line         | effect                                            |
//! |--------------|---------------------------------------------------|
//! | `size`       | prints the pty size as `<cols>x<rows>`            |
//! | `colors`     | prints [`COLOR_OUTPUT`]                           |
//! | `fullscreen` | prints [`FULLSCREEN_OUTPUT`]                      |
//! | `lines <n>`  | prints `line 1` to `line <n>`, one per row          |
//! | `flood`      | prints [`FLOOD_LINE`] without end until Ctrl+C    |
//! | `exit [n]`   | ends the shell with exit status `n` (default 0)   |
//! | `drop`       | drops the connection without ending the shell     |
//!
//! Every server also forwards `direct-tcpip` channels to the address asked
//! for, so it can be a jump host; [`TestSshServer::forwarded`] counts them.
//!
//! Ctrl+C prints [`INTERRUPTED`] and a new prompt, stopping a flood. A flood
//! is sent as fast as the client's SSH window allows, like a device dumping
//! a huge table.
//!
//! The output helpers acknowledge every byte they receive, as the interface
//! does once the terminal has consumed it.
//! `examples/test_ssh_server.rs` runs the same server standalone, for trying
//! the app by hand and for the end-to-end smoke test.
//!
//! Each server has its own random Ed25519 host key. Every server started in
//! the process is trusted in [`known_hosts`], which [`manager`] uses, so
//! tests about something else are not asked about host keys; host-key tests
//! use a `KnownHosts` of their own.
#![allow(dead_code)]

pub mod agent;
pub mod workspaces;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use std::sync::OnceLock;

use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::{
    HostKeyQuestion, LoginPrompt, OutputOptions, SessionEvent, SessionEvents, SessionManager,
    SessionState, SshAuth, SshTarget,
};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{
    Auth, ChannelOpenHandle, Handler, Msg, Response, Server, Session, VerifiedPublicKey,
};
use russh::{Channel, ChannelId, ChannelOpenFailure, MethodKind, MethodSet, Pty};
use tokio::net::{TcpListener, TcpStream};

pub const USERNAME: &str = "engineer";
pub const PASSWORD: &str = "correct horse";

/// A user who logs in with [`PASSWORD`] and then a one-time code.
///
/// After the password method accepts the password, keyboard-interactive
/// asks [`CODE_QUESTION`]. Keyboard-interactive on its own asks
/// [`PASSWORD_QUESTION`] first, then [`CODE_QUESTION`]: two rounds.
pub const OTP_USERNAME: &str = "engineer-otp";
pub const ONE_TIME_CODE: &str = "424242";
pub const PASSWORD_QUESTION: &str = "Password: ";
pub const CODE_QUESTION: &str = "Verification code: ";
/// The title and instructions of the code round.
pub const CODE_NAME: &str = "Two-factor login";
pub const CODE_INSTRUCTIONS: &str = "Enter the code from your authenticator app.";
pub const PROMPT: &[u8] = b"test-ssh$ ";

/// The byte a terminal sends for Ctrl+C.
pub const CTRL_C: u8 = 0x03;

/// SGR colour sequences, as a device would send them.
pub const COLOR_OUTPUT: &[u8] = b"\x1b[1;31mdown\x1b[0m \x1b[38;5;46mup\x1b[0m\r\n";

/// What a full-screen program sends: alternate screen, clear, cursor
/// addressing, a UTF-8 box-drawing character, and back to the main screen.
pub const FULLSCREEN_OUTPUT: &[u8] =
    b"\x1b[?1049h\x1b[2J\x1b[H\x1b[5;10H\xe2\x94\x82 top\x1b[?25l\x1b[?25h\x1b[?1049l";

/// One line of `flood` output.
pub const FLOOD_LINE: &[u8] = b"flood flood flood flood flood flood flood\r\n";

/// What Ctrl+C prints before the next prompt.
pub const INTERRUPTED: &[u8] = b"^C\r\n";

/// How long a test waits for something the server should send.
pub const WAIT: Duration = Duration::from_secs(10);

/// An RSA client key (OpenSSH format, no passphrase) that the server
/// accepts for [`USERNAME`]. RSA keys are slow to generate, so it is a file.
pub const RSA_CLIENT_KEY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/keys/rsa_client"
);

/// The host keys of every test server started in this process.
pub fn known_hosts() -> KnownHosts {
    static KNOWN_HOSTS: OnceLock<KnownHosts> = OnceLock::new();
    KNOWN_HOSTS.get_or_init(KnownHosts::in_memory).clone()
}

/// A session manager that trusts every test server's host key.
pub fn manager() -> SessionManager {
    SessionManager::new(known_hosts())
}

/// Like [`manager`], with the given output options.
pub fn manager_with_output_options(options: OutputOptions) -> SessionManager {
    SessionManager::with_output_options(known_hosts(), options)
}

pub struct TestSshServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    host_key: PublicKey,
    /// An Ed25519 client key the server accepts for [`USERNAME`].
    client_key: PrivateKey,
    forwarded: Arc<AtomicUsize>,
}

impl TestSshServer {
    /// Starts a server on a free loopback port.
    pub async fn start() -> Self {
        Self::start_on(0).await
    }

    /// Starts a server on a free loopback port that offers only `methods`
    /// at first (as a device that only takes keyboard-interactive does).
    pub async fn start_offering(methods: &[MethodKind]) -> Self {
        Self::start_with(0, Some(MethodSet::from(methods))).await
    }

    /// The Ed25519 client key the server accepts for [`USERNAME`].
    pub fn client_key(&self) -> &PrivateKey {
        &self.client_key
    }

    /// Writes the Ed25519 client key the server accepts to `file`, in
    /// OpenSSH format, encrypted with `passphrase` if one is given.
    pub fn write_client_key(&self, file: &std::path::Path, passphrase: Option<&str>) {
        let key = match passphrase {
            Some(passphrase) => self
                .client_key
                .encrypt(&mut rand::rng(), passphrase)
                .unwrap(),
            None => self.client_key.clone(),
        };
        let text = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap();
        std::fs::write(file, text.as_bytes()).unwrap();
    }

    /// The server's host key.
    pub fn host_key(&self) -> &PublicKey {
        &self.host_key
    }

    /// Stops this server and starts another on the same port, with a new
    /// host key (and new client keys), as when a device is replaced.
    pub async fn replace_with_new_host_key(&mut self) {
        self.task.abort();
        // The listener is closed once the task has ended.
        let _ = (&mut self.task).await;
        *self = Self::start_on(self.port).await;
    }

    /// Starts a server on the given loopback port (0 picks a free one).
    pub async fn start_on(port: u16) -> Self {
        Self::start_with(port, None).await
    }

    async fn start_with(port: u16, methods: Option<MethodSet>) -> Self {
        let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let rsa_key = russh::keys::load_secret_key(RSA_CLIENT_KEY, None).unwrap();
        let authorized_keys = Arc::new(vec![
            client_key.public_key().clone(),
            rsa_key.public_key().clone(),
        ]);
        let host_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let defaults = russh::server::Config::default();
        let config = Arc::new(russh::server::Config {
            methods: methods.unwrap_or(defaults.methods.clone()),
            auth_rejection_time: Duration::ZERO,
            // A small receive window, as on network devices: input larger
            // than this has to wait for the server to make room.
            window_size: 16 * 1024,
            keys: vec![host_key.clone()],
            ..defaults
        });
        let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        known_hosts()
            .remember("127.0.0.1", port, host_key.public_key())
            .unwrap();
        let forwarded = Arc::new(AtomicUsize::new(0));
        let counter = forwarded.clone();
        let task = tokio::spawn(async move {
            let mut server = ScriptedServer {
                authorized_keys,
                forwarded: counter,
            };
            let _ = server.run_on_socket(config, &listener).await;
        });
        Self {
            port,
            task,
            host_key: host_key.public_key().clone(),
            client_key,
            forwarded,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// How many connections clients have opened through this server, as
    /// through a jump host (`direct-tcpip` channels it forwarded).
    pub fn forwarded(&self) -> usize {
        self.forwarded.load(Ordering::Relaxed)
    }

    /// A target that logs in to this server successfully.
    pub fn target(&self) -> SshTarget {
        SshTarget {
            host: "127.0.0.1".into(),
            port: self.port,
            username: USERNAME.into(),
            auth: SshAuth::Password(PASSWORD.into()),
            jump_hosts: Vec::new(),
        }
    }
}

impl Drop for TestSshServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct ScriptedServer {
    authorized_keys: Arc<Vec<PublicKey>>,
    forwarded: Arc<AtomicUsize>,
}

impl Server for ScriptedServer {
    type Handler = ScriptedShell;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> ScriptedShell {
        ScriptedShell {
            authorized_keys: self.authorized_keys.clone(),
            forwarded: self.forwarded.clone(),
            channels: HashMap::new(),
            interactive: Interactive::Start,
        }
    }
}

struct ScriptedShell {
    /// The public keys [`USERNAME`] may log in with.
    authorized_keys: Arc<Vec<PublicKey>>,
    /// How many connections this server has forwarded (`direct-tcpip`).
    forwarded: Arc<AtomicUsize>,
    /// Pty size and pending input line, per channel.
    channels: HashMap<ChannelId, ShellState>,
    /// Where [`OTP_USERNAME`]'s login has got to.
    interactive: Interactive,
}

/// The login of [`OTP_USERNAME`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interactive {
    Start,
    /// The password method accepted the password.
    PasswordAccepted,
    AskedPassword,
    AskedCode,
}

fn ask(name: &'static str, instructions: &'static str, question: &'static str, echo: bool) -> Auth {
    Auth::Partial {
        name: name.into(),
        instructions: instructions.into(),
        prompts: vec![(question.into(), echo)].into(),
    }
}

fn ask_code() -> Auth {
    ask(CODE_NAME, CODE_INSTRUCTIONS, CODE_QUESTION, true)
}

#[derive(Default)]
struct ShellState {
    cols: u32,
    rows: u32,
    line: Vec<u8>,
    /// Set while a flood is running; setting the flag stops it.
    flood: Option<Arc<AtomicBool>>,
}

/// Sends [`FLOOD_LINE`] until `stop` is set, then the interrupt and a prompt.
fn start_flood(channel: ChannelId, session: &Session) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = session.handle();
    let flooding = stop.clone();
    tokio::spawn(async move {
        let chunk = FLOOD_LINE.repeat(200);
        while !flooding.load(Ordering::Relaxed) {
            if handle.data(channel, chunk.clone()).await.is_err() {
                return;
            }
        }
        let _ = handle.data(channel, [INTERRUPTED, PROMPT].concat()).await;
    });
    stop
}

impl ScriptedShell {
    fn run_line(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), russh::Error> {
        let state = self.channels.entry(channel).or_default();
        let line = String::from_utf8_lossy(&std::mem::take(&mut state.line)).into_owned();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("size") => {
                let size = format!("{}x{}\r\n", state.cols, state.rows);
                session.data(channel, size.into_bytes())?;
            }
            Some("colors") => session.data(channel, COLOR_OUTPUT)?,
            Some("fullscreen") => session.data(channel, FULLSCREEN_OUTPUT)?,
            Some("lines") => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                session.data(channel, numbered_lines(count))?;
            }
            Some("flood") => {
                state.flood = Some(start_flood(channel, session));
                return Ok(());
            }
            Some("exit") => {
                let status = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                session.exit_status_request(channel, status)?;
                session.eof(channel)?;
                session.close(channel)?;
                return Ok(());
            }
            Some("drop") => return Err(russh::Error::Disconnect),
            _ => {}
        }
        session.data(channel, PROMPT)?;
        Ok(())
    }
}

impl Handler for ScriptedShell {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == USERNAME && password == PASSWORD {
            Ok(Auth::Accept)
        } else if user == OTP_USERNAME && password == PASSWORD {
            self.interactive = Interactive::PasswordAccepted;
            Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::KeyboardInteractive][..])),
                partial_success: true,
            })
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if user != OTP_USERNAME {
            return Ok(Auth::reject());
        }
        let answer = response
            .and_then(|mut answers| answers.next())
            .map(|answer| String::from_utf8_lossy(&answer).into_owned());
        let (next, auth) = match (self.interactive, answer.as_deref()) {
            (Interactive::Start, None) => (
                Interactive::AskedPassword,
                ask("", "", PASSWORD_QUESTION, false),
            ),
            (Interactive::PasswordAccepted, None) => (Interactive::AskedCode, ask_code()),
            (Interactive::AskedPassword, Some(PASSWORD)) => (Interactive::AskedCode, ask_code()),
            (Interactive::AskedCode, Some(ONE_TIME_CODE)) => (Interactive::Start, Auth::Accept),
            _ => (Interactive::Start, Auth::reject()),
        };
        self.interactive = next;
        Ok(auth)
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        let known = self
            .authorized_keys
            .iter()
            .any(|authorized| authorized.key_data() == key.public_key.key_data());
        if user == USERNAME && known {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), ShellState::default());
        reply.accept().await;
        Ok(())
    }

    /// Forwards the channel to `host:port` over TCP, as a bastion does for
    /// a client that jumps through it.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Ok(port) = u16::try_from(port_to_connect) else {
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        match TcpStream::connect((host_to_connect, port)).await {
            Ok(mut socket) => {
                reply.accept().await;
                self.forwarded.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut socket).await;
                });
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let state = self.channels.entry(channel).or_default();
        state.cols = col_width;
        state.rows = row_height;
        session.channel_success(channel)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let state = self.channels.entry(channel).or_default();
        state.cols = col_width;
        state.rows = row_height;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, PROMPT)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Data on a forwarded channel is the forwarding task's, not a
        // shell's (russh passes it to both).
        if !self.channels.contains_key(&channel) {
            return Ok(());
        }
        for &byte in data {
            if byte == CTRL_C {
                let state = self.channels.entry(channel).or_default();
                state.line.clear();
                match state.flood.take() {
                    // The flood prints the interrupt and prompt as it stops.
                    Some(stop) => stop.store(true, Ordering::Relaxed),
                    None => session.data(channel, [INTERRUPTED, PROMPT].concat())?,
                }
            } else if self
                .channels
                .get(&channel)
                .is_some_and(|s| s.flood.is_some())
            {
                // Input typed during a flood is not echoed, like a busy device.
            } else if byte == b'\r' {
                session.data(channel, &b"\r\n"[..])?;
                self.run_line(channel, session)?;
            } else {
                session.data(channel, vec![byte])?;
                self.channels.entry(channel).or_default().line.push(byte);
            }
        }
        Ok(())
    }
}

/// What the scripted shell prints for `lines <count>`.
pub fn numbered_lines(count: u32) -> Vec<u8> {
    (1..=count)
        .flat_map(|n| format!("line {n}\r\n").into_bytes())
        .collect()
}

/// Waits for the next state change, discarding output on the way.
pub async fn next_state(events: &mut SessionEvents) -> SessionState {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::State(state)) => return state,
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                Some(SessionEvent::Prompt(prompt)) => {
                    panic!("expected a state change, got the login prompt {prompt:?}")
                }
                Some(SessionEvent::HostKey(question)) => {
                    panic!("expected a state change, got the host-key question {question:?}")
                }
                None => panic!("session events ended while waiting for a state change"),
            }
        }
    })
    .await
    .expect("timed out waiting for a state change")
}

/// Waits for the server's next login questions.
pub async fn next_prompt(events: &mut SessionEvents) -> LoginPrompt {
    let event = tokio::time::timeout(WAIT, events.next())
        .await
        .expect("timed out waiting for a login prompt");
    match event {
        Some(SessionEvent::Prompt(prompt)) => prompt,
        other => panic!("expected a login prompt, got {other:?}"),
    }
}

/// Waits for the question whether to trust the server's host key.
pub async fn next_host_key_question(events: &mut SessionEvents) -> HostKeyQuestion {
    let event = tokio::time::timeout(WAIT, events.next())
        .await
        .expect("timed out waiting for a host-key question");
    match event {
        Some(SessionEvent::HostKey(question)) => question,
        other => panic!("expected a host-key question, got {other:?}"),
    }
}

/// Collects output until it ends with `expected`, and returns all of it.
///
/// Panics if a state change arrives first or nothing matching arrives in time.
pub async fn output_ending_with(events: &mut SessionEvents, expected: &[u8]) -> Vec<u8> {
    output_batches_ending_with(events, expected).await.concat()
}

/// Like [`output_ending_with`], but returns each output event separately.
pub async fn output_batches_ending_with(
    events: &mut SessionEvents,
    expected: &[u8],
) -> Vec<Vec<u8>> {
    let mut batches = Vec::new();
    let mut received = Vec::new();
    let result = tokio::time::timeout(WAIT, async {
        while !received.ends_with(expected) {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    received.extend_from_slice(&bytes);
                    batches.push(bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await;
    if result.is_err() {
        panic!(
            "timed out waiting for output ending with {:?}; received {:?}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&received)
        );
    }
    batches
}

/// Collects output without acknowledging it until none arrives for `quiet`.
pub async fn unacknowledged_output(events: &mut SessionEvents, quiet: Duration) -> Vec<u8> {
    let mut received = Vec::new();
    loop {
        match tokio::time::timeout(quiet, events.next()).await {
            Ok(Some(SessionEvent::Output(bytes))) => received.extend_from_slice(&bytes),
            Ok(other) => panic!("expected output, got {other:?}"),
            Err(_) => return received,
        }
    }
}
