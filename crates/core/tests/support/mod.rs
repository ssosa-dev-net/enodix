//! Session test harness: an SSH server that runs inside the test process.
//!
//! The spec's session tests use a containerised OpenSSH server; this harness
//! is the fallback for machines without Docker. It speaks the real SSH
//! protocol (key exchange, authentication, channels, pty and window-change
//! requests) through `russh`'s server side, with a small scripted shell
//! behind it instead of a real one.
//!
//! The scripted shell echoes input like a tty (Enter is CR, LF or CR LF)
//! and understands:
//!
//! | line         | effect                                            |
//! |--------------|---------------------------------------------------|
//! | `size`       | prints the pty size as `<cols>x<rows>`            |
//! | `colors`     | prints [`COLOR_OUTPUT`]                           |
//! | `fullscreen` | prints [`FULLSCREEN_OUTPUT`]                      |
//! | `lines <n>`  | prints `line 1` to `line <n>`, one per row          |
//! | `flood`      | prints [`FLOOD_LINE`] without end until Ctrl+C    |
//! | `later <s> <text>` | prompts at once, prints `<text>` `<s>` seconds later |
//! | `exit [n]`   | ends the shell with exit status `n` (default 0)   |
//! | `drop`       | drops the connection without ending the shell     |
//! | `freeze`     | the connection stops answering anything (silent)  |
//! | `raw <hex>...` | sends each argument's bytes (hex) as a data message of its own, then the prompt |
//! | `slowly <ms> <text>` | sends `<text>` a byte at a time, `<ms>` apart, then the prompt |
//! | `burst <n>`  | prints `line 1` to `line <n>` and ends the shell (exit status 0) in the same breath |
//!
//! [`TestSshServer::quirks`] makes a server misbehave on purpose (#118):
//! close or go silent at a stage of the login ([`ServerQuirks::misbehave_at`]),
//! send a login banner or output before confirming the shell, read input
//! slowly. [`TestSshServer::start_tuned`] changes its russh configuration
//! (rekeying limits, say).
//!
//! A server started with [`TestSshServer::start_device`] runs the shell in
//! device mode, like a network device's CLI: its prompt is `<hostname># `
//! ([`device_prompt`]) instead of [`PROMPT`], and besides the lines above it
//! understands `show hostname` (prints the hostname) and `echo <text>`
//! (prints the text); any other line prints [`UNKNOWN_COMMAND`] and the line.
//! Device mode also behaves like a device a job has to cope with:
//!
//! | line                  | effect                                          |
//! |-----------------------|-------------------------------------------------|
//! | `lines <n>`           | paged: [`PAGE_LENGTH`] lines, then [`MORE`]; any key but `q` shows the next page |
//! | `terminal length <n>` | pages of `n` lines; `0` turns paging off        |
//! | `enable`              | asks [`ENABLE_QUESTION`] (input not echoed); [`ENABLE_SECRET`] gives the `#` prompt, a wrong one asks again, three wrong print [`BAD_SECRETS`] |
//! | `disable`             | back to the unprivileged prompt                 |
//! | `hang`                | never answers (Ctrl+C gives the prompt back)    |
//! | `pause <ms>`          | prints the prompt after `<ms>` milliseconds     |
//! | `drip <n> <ms>`       | prints `line 1` to `line <n>`, one every `<ms>` milliseconds, then the prompt |
//! | `halt <ms> <text>`    | prints `<text>`, nothing for `<ms>`, then the line break, [`HALT_REST`] and the prompt |
//! | `log <ms> <text>`     | the prompt, then after `<ms>` the text on a line of its own and the prompt again |
//! | `prompt <name>` / `rename <name>` | the prompt shows `<name>` from now on (no name: the hostname) |
//! | `show running-config` | prints [`device_config`]: two `! ` header lines (the second a [`CONFIG_TIME`] line that changes every time), `hostname <hostname>`, the lines `configure` added, `end`; unprivileged, [`INVALID_INPUT`] (as IOS) |
//! | `configure <line>`    | adds `<line>` to the configuration (shared by every connection to the server; also [`TestSshServer::configure`]) |
//!
//! [`TestSshServer::start_device`] starts privileged (`<hostname># `);
//! [`TestSshServer::start_unprivileged_device`] starts at `<hostname>> `
//! ([`unprivileged_prompt`]) and needs `enable`. How `enable` answers is
//! [`TestSshServer::set_enable`]'s ([`EnableMode`]: asking for the secret,
//! or as IOS with no enable password, or with a notice);
//! [`TestSshServer::set_config_pace`] shows the configuration a line at a
//! time, [`TestSshServer::set_config_stall`] pauses it half way through a
//! line; [`TestSshServer::set_shell_start`] replaces each shell's first
//! prompt with pieces sent after pauses (a banner, a log message);
//! [`TestSshServer::ask_password_at_login`] makes every shell ask
//! [`LOGIN_QUESTION`] in-band before its first prompt.
//! [`TestSshServer::connections_open`] and
//! [`TestSshServer::most_connections_open`] count the connections that
//! opened a session channel, while they last.
//! [`TestSshServer::logins`] counts accepted logins, and
//! [`TestSshServer::refuse_second_channel`] makes the server refuse every
//! session channel after a connection's first.
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
//!
//! [`telnet`] has the telnet server (a scripted device behind real telnet
//! option negotiation); [`serial`] a virtual serial port pair; [`http`] a
//! stand-in for the release server (the update check); [`tcp`] a plain TCP
//! service (a device's second port, for the tunnels' extra ports).
#![allow(dead_code)]

pub mod agent;
pub mod algorithms;
pub mod credentials;
pub mod http;
pub mod jobs;
pub mod keychain;
pub mod memory;
pub mod processes;
pub mod proxy;
pub mod python;
pub mod raw;
pub mod scp;
pub mod serial;
pub mod sftp;
pub mod tcp;
pub mod telnet;
pub mod workspaces;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::sync::OnceLock;

use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::{
    HostKeyQuestion, LoginPrompt, NegotiatedAlgorithms, OutputOptions, SessionEvent, SessionEvents,
    SessionManager, SessionState, SshAuth, SshTarget, DEFAULT_CONNECT_TIMEOUT,
};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{
    Auth, ChannelOpenHandle, Handler, Msg, Response, Server, Session, VerifiedPublicKey,
};
use russh::{Channel, ChannelId, ChannelOpenFailure, MethodKind, MethodSet, Preferred, Pty};
use tokio::net::{TcpListener, TcpStream};

pub const USERNAME: &str = "engineer";
pub const PASSWORD: &str = "correct horse";

/// A user who logs in with [`PASSWORD`] and then a one-time code.
///
/// After the password method accepts the password, keyboard-interactive
/// asks [`CODE_QUESTION`]. Keyboard-interactive on its own asks
/// [`PASSWORD_QUESTION`] first, then [`CODE_QUESTION`]: two rounds.
pub const OTP_USERNAME: &str = "engineer-otp";
/// A user whose login the server never answers, like a device whose
/// authentication backend hangs.
/// A user who logs in with the client key and then [`PASSWORD`], as with
/// OpenSSH's `AuthenticationMethods publickey,password`: the key is
/// accepted as a first step, and only the password method is offered next.
pub const KEY_THEN_PASSWORD_USERNAME: &str = "engineer-key-pw";
pub const STALLING_USERNAME: &str = "engineer-stalled";
/// A user whose keyboard-interactive login asks [`THREE_QUESTIONS`], one
/// round each, in a row; the answers are [`THREE_ANSWERS`].
pub const THREE_QUESTIONS_USERNAME: &str = "engineer-3q";
pub const THREE_QUESTIONS: [&str; 3] = ["Site: ", "Password: ", "Token: "];
pub const THREE_ANSWERS: [&str; 3] = ["madrid", PASSWORD, ONE_TIME_CODE];
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

/// What `program` sends: a full-screen program's screen and modes.
pub const PROGRAM_ON: &[u8] = b"\x1b[?1049h\x1b[?2004h\x1b[?1h\x1b[2J\x1b[Hfull screen program\r\n";

/// What `program off` sends: those modes off, back to the main screen.
pub const PROGRAM_OFF: &[u8] = b"\x1b[?1l\x1b[?2004l\x1b[?1049l";

/// One line of `flood` output.
pub const FLOOD_LINE: &[u8] = b"flood flood flood flood flood flood flood\r\n";

/// What a device-mode shell prints before a line it does not know.
pub const UNKNOWN_COMMAND: &str = "% Unknown command: ";

/// The (privileged) prompt of a device-mode shell called `hostname`.
pub fn device_prompt(hostname: &str) -> Vec<u8> {
    format!("{hostname}# ").into_bytes()
}

/// The unprivileged prompt of a device-mode shell called `hostname`.
pub fn unprivileged_prompt(hostname: &str) -> Vec<u8> {
    format!("{hostname}> ").into_bytes()
}

/// The secret `enable` takes on a device-mode shell.
pub const ENABLE_SECRET: &str = "enable sesame";
/// What `enable` asks on a device-mode shell.
pub const ENABLE_QUESTION: &str = "Password: ";
/// An enable secret a device-mode shell denies at once, back at the
/// unprivileged prompt, without asking again.
pub const DENIED_SECRET: &str = "denied at once";
/// What a device-mode shell prints for [`DENIED_SECRET`].
pub const ACCESS_DENIED: &str = "% Access denied";
/// What a device-mode shell prints after three wrong enable secrets.
pub const BAD_SECRETS: &str = "% Bad secrets";
/// What a device-mode shell with [`EnableMode::NotSet`] prints for
/// `enable`, staying at the unprivileged prompt (as IOS with no enable
/// password).
pub const NO_ENABLE_PASSWORD: &str = "% No password set";
/// What a device-mode shell prints, below a `^` marker line, for `show
/// running-config` at the unprivileged prompt (as IOS).
pub const INVALID_INPUT: &str = "% Invalid input detected at '^' marker.";
/// What a shell asks in-band right after login, before its first prompt,
/// once [`TestSshServer::ask_password_at_login`] is set ([`PASSWORD`]
/// answers it).
pub const LOGIN_QUESTION: &str = "Password: ";

/// How a device-mode shell answers `enable` at its unprivileged prompt.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EnableMode {
    /// Asks [`ENABLE_QUESTION`] for [`ENABLE_SECRET`].
    #[default]
    Secret,
    /// Prints [`NO_ENABLE_PASSWORD`] and stays unprivileged.
    NotSet,
    /// Prints the text and gives the privileged prompt, asking nothing.
    Notice(String),
}
/// How many lines a device-mode shell shows before [`MORE`] until
/// `terminal length` says otherwise.
pub const PAGE_LENGTH: u32 = 24;
/// What a device-mode shell shows at the end of a page of output.
pub const MORE: &str = " --More-- ";
/// How a device-mode shell wipes [`MORE`] before the next page.
const ERASE_MORE: &[u8] = b"\r\x1b[K";

/// The start of the line of `show running-config` output that changes on
/// every call (EOS's ignored-lines pattern matches it).
pub const CONFIG_TIME: &str = "! Time: ";

/// What a device-mode shell called `hostname`, with the lines `added`
/// configured, shows for `show running-config` (LF line breaks; the device
/// sends CRLF), its `n`th time.
pub fn device_config(hostname: &str, added: &[String], n: u64) -> String {
    let mut text =
        format!("! Command: show running-config\n{CONFIG_TIME}{n}\nhostname {hostname}\n");
    for line in added {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str("end\n");
    text
}

/// The line `halt` shows after the line it paused in.
pub const HALT_REST: &str = "rest of the answer";

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

/// A DSA host key (OpenSSH format, no passphrase), for servers restricted
/// to `ssh-dss`. A throwaway test key.
pub const DSA_HOST_KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/keys/dsa_host");

/// The host keys of every test server started in this process.
pub fn known_hosts() -> KnownHosts {
    static KNOWN_HOSTS: OnceLock<KnownHosts> = OnceLock::new();
    KNOWN_HOSTS.get_or_init(KnownHosts::in_memory).clone()
}

/// A session manager that trusts every test server's host key.
/// A window's end of a session shown through a relay: each event it is
/// delivered goes to the channel; gone once the receiver is dropped.
#[allow(dead_code)]
pub struct ToChannel(pub tokio::sync::mpsc::UnboundedSender<enodix_core::session::SessionEvent>);

impl enodix_core::session::Consumer for ToChannel {
    fn deliver(
        &mut self,
        event: enodix_core::session::SessionEvent,
    ) -> Result<(), enodix_core::session::ConsumerGone> {
        self.0
            .send(event)
            .map_err(|_| enodix_core::session::ConsumerGone)
    }
}

pub fn manager() -> SessionManager {
    SessionManager::new(known_hosts())
}

/// Like [`manager`], with the given output options.
pub fn manager_with_output_options(options: OutputOptions) -> SessionManager {
    SessionManager::with_output_options(known_hosts(), options)
}

/// What each client of a server sent first, one entry per connection.
type Received = Arc<Mutex<Vec<Arc<Mutex<Vec<u8>>>>>>;

pub struct TestSshServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    host_key: PublicKey,
    /// An Ed25519 client key the server accepts for [`USERNAME`].
    client_key: PrivateKey,
    forwarded: Arc<AtomicUsize>,
    /// The first bytes each client sent, one entry per connection.
    received: Received,
    /// How a device-mode shell starts.
    device: Option<Device>,
    open: Arc<OpenConnections>,
    /// How many logins it accepted, and whether it refuses second channels.
    logins: Arc<AtomicUsize>,
    one_channel: Arc<AtomicBool>,
    /// The tasks serving its connections, to cut them.
    serving: Arc<Mutex<Vec<russh::server::Handle>>>,
    /// A duplicate of each socket it accepted whose session is still on,
    /// to sever them.
    accepted: Arc<Mutex<HashMap<usize, std::net::TcpStream>>>,
    /// How it answers a request for the `sftp` subsystem.
    sftp: Arc<Mutex<sftp::SftpMode>>,
    sftp_quirks: Arc<sftp::SftpQuirks>,
    /// How it answers `scp -t` and `scp -f`, and how many run now.
    scp: Arc<Mutex<scp::ScpMode>>,
    scp_running: scp::Running,
    /// How many shells its clients started.
    shells: Arc<AtomicUsize>,
    /// How many connections it has now, logged in or not.
    alive: Arc<AtomicUsize>,
    /// How it misbehaves on purpose.
    quirks: Arc<ServerQuirks>,
}

/// A stage of a connection at which a server can misbehave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// When the client tries to log in (password, key or keyboard-interactive).
    Login,
    /// When the logged-in client opens its session channel.
    ChannelOpen,
    /// When the client asks for the shell.
    ShellRequest,
    /// When the client asks for a subsystem (`sftp`).
    Subsystem,
}

/// What a server does at a [`Stage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Misbehaviour {
    /// Drops the connection there.
    Close,
    /// Stops answering anything on that connection, keeping it open.
    Stall,
    /// Takes this long to answer, then goes on (a slow device).
    Pause(Duration),
}

/// How a test server misbehaves on purpose; see [`TestSshServer::quirks`].
#[derive(Default)]
pub struct ServerQuirks {
    misbehave: Mutex<HashMap<Stage, Misbehaviour>>,
    auth_banner: Mutex<Option<String>>,
    shell_banner: Mutex<Option<Vec<u8>>>,
    input_delay: Mutex<Option<Duration>>,
    refuse_forwarding: AtomicBool,
}

impl ServerQuirks {
    /// From now on, the server refuses to forward connections, as a jump
    /// host whose administrator does not allow it (`AllowTcpForwarding
    /// no`).
    pub fn refuse_forwarding(&self) {
        self.refuse_forwarding.store(true, Ordering::SeqCst);
    }

    /// From now on, every connection misbehaves as `how` at `stage`.
    pub fn misbehave_at(&self, stage: Stage, how: Misbehaviour) {
        self.misbehave.lock().unwrap().insert(stage, how);
    }

    /// From now on, every connection takes `pause` to answer at `stage`.
    pub fn pause_at(&self, stage: Stage, pause: Duration) {
        self.misbehave_at(stage, Misbehaviour::Pause(pause));
    }

    /// From now on, the server sends `banner` before the login (the SSH
    /// authentication banner, a legal warning on most devices).
    pub fn auth_banner(&self, banner: &str) {
        *self.auth_banner.lock().unwrap() = Some(banner.to_owned());
    }

    /// From now on, a shell prints `output` before the server confirms it
    /// (as some devices print their banner).
    pub fn output_before_shell(&self, output: &[u8]) {
        *self.shell_banner.lock().unwrap() = Some(output.to_vec());
    }

    /// From now on, the server takes `delay` over every piece of input it
    /// reads, its whole connection waiting meanwhile: a slow device.
    pub fn read_input_slowly(&self, delay: Duration) {
        *self.input_delay.lock().unwrap() = Some(delay);
    }

    /// What to do at `stage`, if anything.
    async fn at(&self, stage: Stage) -> Result<(), russh::Error> {
        let misbehaviour = self.misbehave.lock().unwrap().get(&stage).copied();
        match misbehaviour {
            Some(Misbehaviour::Close) => Err(russh::Error::Disconnect),
            Some(Misbehaviour::Stall) => std::future::pending().await,
            Some(Misbehaviour::Pause(pause)) => {
                tokio::time::sleep(pause).await;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

/// Changes a test server's russh configuration before it starts.
pub type Tune = Box<dyn FnOnce(&mut russh::server::Config) + Send>;

/// How a device-mode shell starts.
#[derive(Debug, Clone)]
struct Device {
    hostname: String,
    /// Whether it starts at the privileged prompt.
    privileged: bool,
    /// Its configuration: the lines `configure` added, shared by every
    /// connection (and kept when the host key is replaced).
    config: Arc<Mutex<Vec<String>>>,
    /// How many times `show running-config` ran.
    shown: Arc<AtomicU64>,
    /// A line shown above every prompt, as SR Linux's
    /// `--{ running }--[  ]--`.
    banner: Arc<Mutex<Option<String>>>,
    /// How `enable` answers at the unprivileged prompt.
    enable: Arc<Mutex<EnableMode>>,
    /// The time between the lines of `show running-config` (none: all at
    /// once).
    config_pace: Arc<Mutex<Option<Duration>>>,
    /// Whether each shell asks [`LOGIN_QUESTION`] before its first prompt,
    /// and every line typed at that question.
    login_question: Arc<AtomicBool>,
    typed_at_login: Arc<Mutex<Vec<String>>>,
    /// Whether the enable question echoes what is typed at it (a console
    /// that does: the secret shows).
    echo_secret: Arc<AtomicBool>,
    /// A configured line after which `show running-config` says nothing
    /// for a while, before that line's break.
    config_stall: Arc<Mutex<Option<(String, Duration)>>>,
    /// What every shell shows in place of its first prompt.
    shell_start: Arc<Mutex<Option<Pieces>>>,
}

/// Output in pieces, each sent after its pause.
type Pieces = Vec<(Duration, Vec<u8>)>;

/// The connections that opened a session channel, while they last.
#[derive(Default)]
struct OpenConnections {
    now: AtomicUsize,
    most: AtomicUsize,
}

impl OpenConnections {
    fn opened(&self) {
        let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
    }

    fn closed(&self) {
        self.now.fetch_sub(1, Ordering::SeqCst);
    }
}

impl TestSshServer {
    /// Starts a server on a free loopback port.
    pub async fn start() -> Self {
        Self::start_on(0).await
    }

    /// Starts a server on a free loopback port whose shell runs in device
    /// mode as `hostname` (see the module documentation).
    pub async fn start_device(hostname: &str) -> Self {
        Self::start_device_on(0, hostname).await
    }

    /// [`Self::start_device`] on the given loopback port.
    pub async fn start_device_on(port: u16, hostname: &str) -> Self {
        let device = Device::new(hostname, true);
        Self::start_with(port, None, Preferred::default(), Some(device)).await
    }

    /// Like [`Self::start_device`], but the shell starts at the
    /// unprivileged prompt ([`unprivileged_prompt`]): `enable` with
    /// [`ENABLE_SECRET`] gives the privileged one.
    pub async fn start_unprivileged_device(hostname: &str) -> Self {
        Self::start_unprivileged_device_on(0, hostname).await
    }

    /// [`Self::start_unprivileged_device`] on the given loopback port.
    pub async fn start_unprivileged_device_on(port: u16, hostname: &str) -> Self {
        let device = Device::new(hostname, false);
        Self::start_with(port, None, Preferred::default(), Some(device)).await
    }

    /// Starts a server on a free loopback port that offers only `methods`
    /// at first (as a device that only takes keyboard-interactive does).
    pub async fn start_offering(methods: &[MethodKind]) -> Self {
        Self::start_with(
            0,
            Some(MethodSet::from(methods)),
            Preferred::default(),
            None,
        )
        .await
    }

    /// Starts a server on a free loopback port that offers only the
    /// algorithms in `preferred`, as an old device does. Its host key has
    /// the type of `preferred.key`'s first entry (Ed25519, RSA or DSA).
    pub async fn start_with_algorithms(preferred: Preferred) -> Self {
        Self::start_with(0, None, preferred, None).await
    }

    /// An old device: [`Self::start_with_algorithms`] whose shell runs in
    /// device mode as `hostname`.
    pub async fn start_device_with_algorithms(hostname: &str, preferred: Preferred) -> Self {
        Self::start_with(0, None, preferred, Some(Device::new(hostname, true))).await
    }

    /// [`Self::start_with_algorithms`] on the given loopback port.
    pub async fn start_on_with_algorithms(port: u16, preferred: Preferred) -> Self {
        Self::start_with(port, None, preferred, None).await
    }

    /// What the last client to connect offered in its key-exchange init.
    pub fn client_offer(&self) -> Option<algorithms::Offer> {
        let received = self.received.lock().unwrap();
        let last = received.last()?.lock().unwrap();
        algorithms::parse_offer(&last)
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
    /// host key (and new client keys), as when a device is replaced. The
    /// shell keeps its mode.
    pub async fn replace_with_new_host_key(&mut self) {
        self.task.abort();
        // The listener is closed once the task has ended.
        let _ = (&mut self.task).await;
        let device = self.device.clone();
        *self = Self::start_with(self.port, None, Preferred::default(), device).await;
    }

    /// Starts a server on the given loopback port (0 picks a free one).
    pub async fn start_on(port: u16) -> Self {
        Self::start_with(port, None, Preferred::default(), None).await
    }

    /// Starts a server on a free loopback port whose russh configuration
    /// `tune` changes (its rekeying limits, its inactivity timeout, ...).
    pub async fn start_tuned(
        tune: impl FnOnce(&mut russh::server::Config) + Send + 'static,
    ) -> Self {
        Self::start_configured(0, None, Preferred::default(), None, Some(Box::new(tune))).await
    }

    /// How the server misbehaves on purpose (it behaves until told).
    pub fn quirks(&self) -> &ServerQuirks {
        &self.quirks
    }

    async fn start_with(
        port: u16,
        methods: Option<MethodSet>,
        preferred: Preferred,
        device: Option<Device>,
    ) -> Self {
        Self::start_configured(port, methods, preferred, device, None).await
    }

    async fn start_configured(
        port: u16,
        methods: Option<MethodSet>,
        preferred: Preferred,
        device: Option<Device>,
        tune: Option<Tune>,
    ) -> Self {
        let client_key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let rsa_key = russh::keys::load_secret_key(RSA_CLIENT_KEY, None).unwrap();
        let authorized_keys = Arc::new(vec![
            client_key.public_key().clone(),
            rsa_key.public_key().clone(),
        ]);
        let host_key = match preferred.key.first() {
            Some(Algorithm::Rsa { .. }) => {
                russh::keys::load_secret_key(RSA_CLIENT_KEY, None).unwrap()
            }
            Some(Algorithm::Dsa) => russh::keys::load_secret_key(DSA_HOST_KEY, None).unwrap(),
            _ => PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap(),
        };
        let defaults = russh::server::Config::default();
        let mut config = russh::server::Config {
            methods: methods.unwrap_or(defaults.methods.clone()),
            auth_rejection_time: Duration::ZERO,
            // A small receive window, as on network devices: input larger
            // than this has to wait for the server to make room.
            window_size: 16 * 1024,
            keys: vec![host_key.clone()],
            preferred,
            ..defaults
        };
        if let Some(tune) = tune {
            tune(&mut config);
        }
        let config = Arc::new(config);
        let quirks = Arc::<ServerQuirks>::default();
        let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        known_hosts()
            .remember("127.0.0.1", port, host_key.public_key())
            .unwrap();
        let forwarded = Arc::new(AtomicUsize::new(0));
        let received = Received::default();
        let counter = forwarded.clone();
        let connections = received.clone();
        let open = Arc::<OpenConnections>::default();
        let logins = Arc::<AtomicUsize>::default();
        let one_channel = Arc::<AtomicBool>::default();
        let serving = Arc::<Mutex<Vec<russh::server::Handle>>>::default();
        let tasks = serving.clone();
        let accepted = Arc::<Mutex<HashMap<usize, std::net::TcpStream>>>::default();
        let sockets = accepted.clone();
        let sftp = Arc::<Mutex<sftp::SftpMode>>::default();
        let sftp_quirks = Arc::<sftp::SftpQuirks>::default();
        let scp = Arc::<Mutex<scp::ScpMode>>::default();
        let scp_running = scp::Running::default();
        let shells = Arc::<AtomicUsize>::default();
        let alive = Arc::<AtomicUsize>::default();
        let mut server = ScriptedServer {
            authorized_keys,
            forwarded: counter,
            device: device.clone(),
            open: open.clone(),
            logins: logins.clone(),
            one_channel: one_channel.clone(),
            sftp: sftp.clone(),
            sftp_quirks: sftp_quirks.clone(),
            sftp_serving: SftpServing::default(),
            scp: scp.clone(),
            scp_running: scp_running.clone(),
            shells: shells.clone(),
            alive: alive.clone(),
            quirks: quirks.clone(),
        };
        let task = tokio::spawn(async move {
            // `run_on_socket`, but keeping what each client sends first.
            let mut accepted_total = 0;
            while let Ok((socket, address)) = listener.accept().await {
                let _ = socket.set_nodelay(true);
                // A duplicate of the socket, to sever it from outside. It
                // is dropped when the session ends: held, it would keep
                // the connection open after the server closed its side.
                let number = accepted_total;
                accepted_total += 1;
                let socket = match socket.into_std() {
                    Ok(socket) => {
                        if let Ok(duplicate) = socket.try_clone() {
                            sockets.lock().unwrap().insert(number, duplicate);
                        }
                        TcpStream::from_std(socket).unwrap()
                    }
                    Err(_) => continue,
                };
                let sockets = sockets.clone();
                let bytes = Arc::<Mutex<Vec<u8>>>::default();
                connections.lock().unwrap().push(bytes.clone());
                let handler = server.new_client(Some(address));
                let config = config.clone();
                let tasks = tasks.clone();
                tokio::spawn(async move {
                    let stream = algorithms::Recording::new(socket, bytes);
                    if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                        tasks.lock().unwrap().push(session.handle());
                        let _ = session.await;
                    }
                    sockets.lock().unwrap().remove(&number);
                });
            }
        });
        Self {
            port,
            task,
            host_key: host_key.public_key().clone(),
            client_key,
            forwarded,
            received,
            device,
            open,
            logins,
            one_channel,
            serving,
            accepted,
            sftp,
            sftp_quirks,
            scp,
            scp_running,
            shells,
            alive,
            quirks,
        }
    }

    /// From now on, serves `folder` over SFTP (see [`sftp`]).
    pub fn serve_sftp(&self, folder: &std::path::Path) {
        *self.sftp.lock().unwrap() = sftp::SftpMode::Serve(folder.to_path_buf());
    }

    /// How its SFTP side misbehaves (see [`sftp::SftpQuirks`]).
    pub fn sftp_quirks(&self) -> &sftp::SftpQuirks {
        &self.sftp_quirks
    }

    /// From now on, refuses the `sftp` subsystem (the default).
    pub fn refuse_sftp(&self) {
        *self.sftp.lock().unwrap() = sftp::SftpMode::Refuse;
    }

    /// From now on, serves `folder` over SCP (see [`scp`]), with no quirks.
    pub fn serve_scp(&self, folder: &std::path::Path) {
        *self.scp.lock().unwrap() = scp::ScpMode {
            root: Some(folder.to_path_buf()),
            quirks: Vec::new(),
        };
    }

    /// From now on, SCP also behaves as `quirk` says.
    pub fn scp_quirk(&self, quirk: scp::ScpQuirk) {
        self.scp.lock().unwrap().quirks.push(quirk);
    }

    /// How many SCP transfers are running on the server now.
    pub fn scp_running(&self) -> usize {
        self.scp_running.load(Ordering::SeqCst)
    }

    /// From now on, accepts the `sftp` subsystem and closes the channel at
    /// once, before SFTP's first reply, and keeps the connection (cEOS for
    /// an account below privilege 15; IOS-XE also ends the connection:
    /// [`Self::close_sftp_and_hang_up`]).
    pub fn close_sftp_at_once(&self) {
        *self.sftp.lock().unwrap() = sftp::SftpMode::CloseAtOnce;
    }

    /// From now on, accepts the `sftp` subsystem, closes the channel at
    /// once and then the whole connection, as IOS-XE really does: by the
    /// time a client looks at why SFTP did not start, the connection is
    /// gone too.
    pub fn close_sftp_and_hang_up(&self) {
        *self.sftp.lock().unwrap() = sftp::SftpMode::CloseAndHangUp;
    }
    /// How many shells its clients have asked for.
    pub fn shell_requests(&self) -> usize {
        self.shells.load(Ordering::SeqCst)
    }

    /// How many connections it has now (from accepting one until it ends),
    /// whatever they opened.
    pub fn connections_alive(&self) -> usize {
        self.alive.load(Ordering::SeqCst)
    }

    /// Cuts every connection made to it so far (their sockets close), as
    /// when the server goes down; it still accepts new ones.
    pub async fn cut_connections(&self) {
        let handles: Vec<_> = self.serving.lock().unwrap().drain(..).collect();
        for handle in handles {
            let _ = handle
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "going down".into(),
                    "en".into(),
                )
                .await;
        }
    }

    /// Severs every connection made to it so far, without a word: their
    /// sockets are shut down, as when the network between goes. Unlike
    /// [`Self::cut_connections`], this works when the server's side of a
    /// connection is stuck (a channel nobody reads).
    pub fn sever_connections(&self) {
        let sockets: Vec<_> = self.accepted.lock().unwrap().drain().collect();
        for (_, socket) in sockets {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }

    /// How many logins this server has accepted (each connection logs in
    /// at most once).
    pub fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }

    /// From now on, refuses every session channel after a connection's
    /// first (as devices that allow one shell per connection do).
    pub fn refuse_second_channel(&self) {
        self.one_channel.store(true, Ordering::SeqCst);
    }

    /// The prompt its shell starts with: [`PROMPT`], or a device's.
    pub fn prompt(&self) -> Vec<u8> {
        let privileged = self.device.as_ref().is_none_or(|d| d.privileged);
        prompt_for(&self.device, privileged)
    }

    /// How many connections to this server have a session channel open (a
    /// shell) now: from their first session channel until they end.
    pub fn connections_open(&self) -> usize {
        self.open.now.load(Ordering::SeqCst)
    }

    /// The most connections [`Self::connections_open`] has counted at once.
    pub fn most_connections_open(&self) -> usize {
        self.open.most.load(Ordering::SeqCst)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// From now on, a device-mode server shows a blank line and `banner`
    /// above every prompt (a two-line prompt, as SR Linux's).
    pub fn set_prompt_banner(&self, banner: &str) {
        let device = self.device.as_ref().expect("a device-mode server");
        *device.banner.lock().unwrap() = Some(banner.to_string());
    }

    /// From now on, a device-mode server answers `enable` at its
    /// unprivileged prompt as `mode` says.
    pub fn set_enable(&self, mode: EnableMode) {
        let device = self.device.as_ref().expect("a device-mode server");
        *device.enable.lock().unwrap() = mode;
    }

    /// From now on, a device-mode server shows `show running-config` one
    /// line every `pace`, as a large configuration over a slow link.
    pub fn set_config_pace(&self, pace: Duration) {
        let device = self.device.as_ref().expect("a device-mode server");
        *device.config_pace.lock().unwrap() = Some(pace);
    }

    /// From now on, a device-mode server's `show running-config` says
    /// nothing for `pause` right after the configured line `line`, before
    /// its line break (a device building a large configuration, or a slow
    /// link, half way through a line).
    pub fn set_config_stall(&self, line: &str, pause: Duration) {
        let device = self.device.as_ref().expect("a device-mode server");
        *device.config_stall.lock().unwrap() = Some((line.to_string(), pause));
    }

    /// From now on, every shell of a device-mode server shows `pieces`, each
    /// after its pause, in place of its first prompt (a login banner, a log
    /// message after the prompt): the last piece should end with
    /// [`Self::prompt`].
    pub fn set_shell_start(&self, pieces: Pieces) {
        let device = self.device.as_ref().expect("a device-mode server");
        *device.shell_start.lock().unwrap() = Some(pieces);
    }

    /// From now on, every shell of a device-mode server asks
    /// [`LOGIN_QUESTION`] in-band before its first prompt (as a device or
    /// console server with a second login), until [`PASSWORD`] is typed.
    pub fn ask_password_at_login(&self) {
        let device = self.device.as_ref().expect("a device-mode server");
        device.login_question.store(true, Ordering::SeqCst);
    }

    /// From now on, the enable question echoes what is typed at it.
    pub fn echo_enable_secret(&self) {
        if let Some(device) = &self.device {
            device.echo_secret.store(true, Ordering::SeqCst);
        }
    }

    /// Every line typed at [`LOGIN_QUESTION`] so far.
    pub fn typed_at_login(&self) -> Vec<String> {
        let device = self.device.as_ref().expect("a device-mode server");
        device.typed_at_login.lock().unwrap().clone()
    }

    /// Adds `line` to a device-mode server's configuration, as `configure
    /// <line>` does.
    pub fn configure(&self, line: &str) {
        let device = self.device.as_ref().expect("a device-mode server");
        device.config.lock().unwrap().push(line.to_string());
    }

    /// What a device-mode server would show for its `n`th `show
    /// running-config` now, as plain text with LF line breaks.
    pub fn running_config(&self, n: u64) -> String {
        let device = self.device.as_ref().expect("a device-mode server");
        device_config(&device.hostname, &device.config.lock().unwrap(), n)
    }

    /// How many connections clients have made to this server.
    pub fn connections(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    /// How many connections clients have opened through this server, as
    /// through a jump host (`direct-tcpip` channels it forwarded).
    pub fn forwarded(&self) -> usize {
        self.forwarded.load(Ordering::Relaxed)
    }

    /// A target that logs in to this server successfully.
    pub fn target(&self) -> SshTarget {
        target_at("127.0.0.1", self.port)
    }
}

impl Drop for TestSshServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A loopback port where connections are accepted and then left without a
/// word, like a device whose SSH server hangs. Stops listening when dropped.
pub struct SilentServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl SilentServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        Self { port, task }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for SilentServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A loopback port where a TCP connection is never made, as at an address
/// whose device is down behind a firewall that drops the SYN (#140): a
/// listener that never accepts, its backlog filled by a connection of its
/// own, so the system leaves further connection requests unanswered.
/// Linux drops them; Windows refuses them, but only after about 2 s of
/// retries, so a connect timeout well below that (1 s) sees the silence.
pub struct UnansweringPort {
    port: u16,
    _listener: tokio::net::TcpListener,
    _filling: Vec<tokio::net::TcpStream>,
}

impl UnansweringPort {
    pub async fn start() -> Self {
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind(([127, 0, 0, 1], 0).into()).unwrap();
        let listener = socket.listen(1).unwrap();
        let address = listener.local_addr().unwrap();
        // Connect until one is not answered: the backlog is full then
        // (how many it takes is the system's choice).
        let mut filling = Vec::new();
        for _ in 0..64 {
            let attempt =
                tokio::time::timeout(Duration::from_millis(200), TcpStream::connect(address)).await;
            match attempt {
                Ok(Ok(connection)) => filling.push(connection),
                Ok(Err(error)) => panic!("filling the backlog: {error}"),
                Err(_) => {
                    return Self {
                        port: address.port(),
                        _listener: listener,
                        _filling: filling,
                    }
                }
            }
        }
        panic!("the backlog of a listener never filled");
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// A loopback port where every connection is closed as soon as it is
/// accepted, before the SSH banner: a device with all its lines in use.
/// Stops listening when dropped.
pub struct HangUpServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl HangUpServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        Self { port, task }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for HangUpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A loopback port nothing listens on.
pub async fn closed_port() -> u16 {
    // Bind a free port, then stop listening on it.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    listener.local_addr().unwrap().port()
}

/// A target that logs in as [`USERNAME`] with [`PASSWORD`] at `host:port`,
/// with the default connect timeout.
pub fn target_at(host: &str, port: u16) -> SshTarget {
    SshTarget {
        host: host.into(),
        port,
        username: USERNAME.into(),
        auth: SshAuth::Password(PASSWORD.into()),
        jump_hosts: Vec::new(),
        legacy_crypto: false,
        connect_timeout: DEFAULT_CONNECT_TIMEOUT,
    }
}

struct ScriptedServer {
    authorized_keys: Arc<Vec<PublicKey>>,
    forwarded: Arc<AtomicUsize>,
    device: Option<Device>,
    open: Arc<OpenConnections>,
    logins: Arc<AtomicUsize>,
    one_channel: Arc<AtomicBool>,
    sftp: Arc<Mutex<sftp::SftpMode>>,
    sftp_quirks: Arc<sftp::SftpQuirks>,
    sftp_serving: SftpServing,
    scp: Arc<Mutex<scp::ScpMode>>,
    scp_running: scp::Running,
    shells: Arc<AtomicUsize>,
    alive: Arc<AtomicUsize>,
    quirks: Arc<ServerQuirks>,
}

/// The connections serving SFTP now, by connection number: the scripted
/// shell's `show sftp` counts them and `cut sftp` cuts them, so the smoke
/// test can see a Files tab's connection go and cut it.
type SftpServing = Arc<Mutex<HashMap<usize, russh::server::Handle>>>;

impl Server for ScriptedServer {
    type Handler = ScriptedShell;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> ScriptedShell {
        self.alive.fetch_add(1, Ordering::SeqCst);
        ScriptedShell {
            authorized_keys: self.authorized_keys.clone(),
            forwarded: self.forwarded.clone(),
            channels: HashMap::new(),
            interactive: Interactive::Start,
            key_accepted: false,
            device: self.device.clone(),
            open: self.open.clone(),
            counted: false,
            logins: self.logins.clone(),
            one_channel: self.one_channel.clone(),
            session_channels: 0,
            unclaimed: HashMap::new(),
            sftp: self.sftp.clone(),
            sftp_quirks: self.sftp_quirks.clone(),
            sftp_serving: self.sftp_serving.clone(),
            scp: self.scp.clone(),
            scp_running: self.scp_running.clone(),
            connection: {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                NEXT.fetch_add(1, Ordering::SeqCst)
            },
            shells: self.shells.clone(),
            alive: self.alive.clone(),
            quirks: self.quirks.clone(),
            three_asked: 0,
        }
    }
}

impl Device {
    fn new(hostname: &str, privileged: bool) -> Self {
        Self {
            hostname: hostname.into(),
            privileged,
            config: Arc::default(),
            shown: Arc::default(),
            banner: Arc::default(),
            enable: Arc::default(),
            config_pace: Arc::default(),
            login_question: Arc::default(),
            typed_at_login: Arc::default(),
            echo_secret: Arc::default(),
            config_stall: Arc::default(),
            shell_start: Arc::default(),
        }
    }

    /// `show running-config`, counting it.
    fn running_config(&self) -> String {
        let n = self.shown.fetch_add(1, Ordering::SeqCst) + 1;
        device_config(&self.hostname, &self.config.lock().unwrap(), n)
    }

    fn prompt(&self, privileged: bool) -> Vec<u8> {
        let prompt = if privileged {
            device_prompt(&self.hostname)
        } else {
            unprivileged_prompt(&self.hostname)
        };
        match self.banner.lock().unwrap().as_deref() {
            None => prompt,
            Some(banner) => [format!("\r\n{banner}\r\n").into_bytes(), prompt].concat(),
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
    /// [`KEY_THEN_PASSWORD_USERNAME`]'s key was accepted (the first step).
    key_accepted: bool,
    /// How the shell starts, in device mode.
    device: Option<Device>,
    /// The server's count of open connections.
    open: Arc<OpenConnections>,
    /// Whether this connection is in `open`.
    counted: bool,
    /// The server's count of accepted logins.
    logins: Arc<AtomicUsize>,
    /// Whether the server refuses a connection's second session channel.
    one_channel: Arc<AtomicBool>,
    /// How many session channels this connection has opened.
    session_channels: usize,
    /// Session channels that have asked for neither a pty nor a shell yet:
    /// one may still become an SFTP session. A shell's is dropped (it is
    /// answered through the session, and a channel nobody reads would
    /// block the connection).
    unclaimed: HashMap<ChannelId, Channel<Msg>>,
    sftp: Arc<Mutex<sftp::SftpMode>>,
    sftp_quirks: Arc<sftp::SftpQuirks>,
    sftp_serving: SftpServing,
    /// This connection's number in `sftp_serving`.
    connection: usize,
    scp: Arc<Mutex<scp::ScpMode>>,
    scp_running: scp::Running,
    shells: Arc<AtomicUsize>,
    alive: Arc<AtomicUsize>,
    quirks: Arc<ServerQuirks>,
    /// How many of [`THREE_QUESTIONS`] this login has asked.
    three_asked: usize,
}

impl Drop for ScriptedShell {
    fn drop(&mut self) {
        if self.counted {
            self.open.closed();
        }
        self.sftp_serving.lock().unwrap().remove(&self.connection);
        self.alive.fetch_sub(1, Ordering::SeqCst);
    }
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

struct ShellState {
    cols: u32,
    rows: u32,
    line: Vec<u8>,
    /// Set while a flood is running; setting the flag stops it.
    flood: Option<Arc<AtomicBool>>,
    /// In device mode: at the privileged prompt.
    privileged: bool,
    /// In device mode: lines per page of `lines` output (0: no paging).
    page_length: u32,
    /// In device mode: what typed input goes to instead of a line.
    waiting: Waiting,
    /// The last byte typed was CR (so a LF right after it ends no line).
    after_cr: bool,
    /// In device mode: the name its prompt shows instead of the device's
    /// hostname (`prompt <name>`: a prompt that changes, as in config mode).
    prompt_name: Option<String>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            cols: 0,
            rows: 0,
            line: Vec::new(),
            flood: None,
            privileged: true,
            page_length: PAGE_LENGTH,
            waiting: Waiting::Line,
            after_cr: false,
            prompt_name: None,
        }
    }
}

/// What a device-mode shell does with typed input.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Waiting {
    /// A command line (as in every mode).
    Line,
    /// The enable secret (not echoed), the `tries`th time it is asked.
    Secret { typed: Vec<u8>, tries: u32 },
    /// The answer to [`LOGIN_QUESTION`] (not echoed).
    Login { typed: Vec<u8> },
    /// A key at [`MORE`]: `lines` output goes on with line `next`, up to
    /// line `last`.
    More { next: u32, last: u32 },
    /// Nothing: `hang` (Ctrl+C ends it) or `pause`.
    Nothing,
}

/// Sends [`FLOOD_LINE`] until `stop` is set, then the interrupt and a prompt.
fn start_flood(channel: ChannelId, session: &Session, prompt: Vec<u8>) -> Arc<AtomicBool> {
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
        let _ = handle.data(channel, [INTERRUPTED, &prompt].concat()).await;
    });
    stop
}

impl ScriptedShell {
    /// [`THREE_QUESTIONS_USERNAME`]'s login: the next question, the
    /// acceptance after the right third answer, or a refusal.
    fn three_questions(&mut self, response: Option<Response<'_>>) -> Auth {
        let answer = response
            .and_then(|mut answers| answers.next())
            .map(|answer| String::from_utf8_lossy(&answer).into_owned());
        let asked = self.three_asked;
        match (asked, answer) {
            (0, None) => {}
            (n @ 1..=3, Some(answer)) if answer == THREE_ANSWERS[n - 1] => {
                if n == 3 {
                    self.three_asked = 0;
                    return Auth::Accept;
                }
            }
            _ => {
                self.three_asked = 0;
                return Auth::reject();
            }
        }
        let question = THREE_QUESTIONS[asked];
        self.three_asked += 1;
        Auth::Partial {
            name: String::new().into(),
            instructions: String::new().into(),
            prompts: vec![(question.into(), asked == 0)].into(),
        }
    }

    /// What the shell prints on `channel` when it waits for a line.
    fn prompt(&self, channel: ChannelId) -> Vec<u8> {
        let state = self.channels.get(&channel);
        let privileged = state.is_none_or(|s| s.privileged);
        match (state.and_then(|s| s.prompt_name.as_deref()), &self.device) {
            (Some(name), Some(_)) if privileged => device_prompt(name),
            (Some(name), Some(_)) => unprivileged_prompt(name),
            _ => prompt_for(&self.device, privileged),
        }
    }

    fn run_line(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), russh::Error> {
        let device = self.device.is_some();
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
            // A full-screen program that stays (#84): the alternate screen,
            // bracketed paste and application cursor keys; `program off`
            // leaves it.
            Some("program") => {
                let output: &[u8] = if words.next() == Some("off") {
                    PROGRAM_OFF
                } else {
                    PROGRAM_ON
                };
                session.data(channel, output)?;
            }
            // A cursor-position and a device-attributes request, which a
            // terminal answers by itself (the answers arrive as input).
            Some("query") => session.data(channel, &b"\x1b[6n\x1b[c"[..])?,
            // Mouse reporting (normal tracking, SGR encoding) on or off.
            Some("mouse") => {
                let mode = if words.next() == Some("off") {
                    b'l'
                } else {
                    b'h'
                };
                session.data(
                    channel,
                    [b"\x1b[?1000".as_slice(), &[mode], b"\x1b[?1006", &[mode]].concat(),
                )?;
            }
            Some("lines") => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                if device && state.page_length > 0 && count > state.page_length {
                    let page = state.page_length;
                    state.waiting = Waiting::More {
                        next: page + 1,
                        last: count,
                    };
                    session.data(channel, numbered_range(1, page))?;
                    session.data(channel, MORE.as_bytes().to_vec())?;
                    return Ok(());
                }
                session.data(channel, numbered_lines(count))?;
            }
            Some("flood") => {
                let prompt = self.prompt(channel);
                let state = self.channels.entry(channel).or_default();
                state.flood = Some(start_flood(channel, session, prompt));
                return Ok(());
            }
            Some("exit") => {
                let status = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                session.exit_status_request(channel, status)?;
                session.eof(channel)?;
                session.close(channel)?;
                return Ok(());
            }
            Some("later") => {
                let seconds: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(1);
                let text = format!("{}\r\n", words.collect::<Vec<_>>().join(" "));
                let handle = session.handle();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    let _ = handle.data(channel, text.into_bytes()).await;
                });
            }
            Some("drop") => return Err(russh::Error::Disconnect),
            // Each argument a data message of its own: a character split
            // across reads, bytes that are not UTF-8.
            Some("raw") => {
                for word in words {
                    session.data(channel, decode_hex(word))?;
                }
            }
            // A console that answers a byte at a time.
            Some("slowly") => {
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(10);
                let text = words.collect::<Vec<_>>().join(" ").into_bytes();
                let prompt = self.prompt(channel);
                let handle = session.handle();
                tokio::spawn(async move {
                    for byte in text {
                        tokio::time::sleep(Duration::from_millis(millis)).await;
                        if handle.data(channel, vec![byte]).await.is_err() {
                            return;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(millis)).await;
                    let _ = handle
                        .data(channel, [b"\r\n".as_slice(), &prompt].concat())
                        .await;
                });
                return Ok(());
            }
            // Output and the end of the shell in one breath: the device
            // closes with data still in flight.
            Some("burst") => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                session.data(channel, numbered_lines(count))?;
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
                return Ok(());
            }
            // Holds SFTP transfers from a byte offset on (the smoke test's
            // transfer that stays running), and lets them go again.
            Some("hold") if line.split_whitespace().nth(1) == Some("sftp") => {
                let offset: u64 = line
                    .split_whitespace()
                    .nth(2)
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                self.sftp_quirks.hold_from(offset);
                session.data(channel, format!("sftp held from {offset}\r\n").into_bytes())?;
            }
            Some("release") if line.split_whitespace().nth(1) == Some("sftp") => {
                self.sftp_quirks.release();
                session.data(channel, b"sftp released\r\n".to_vec())?;
            }
            // SCP refused as IOS does before `ip scp server enable`, or
            // served again.
            Some("scp") if matches!(line.split_whitespace().nth(1), Some("off" | "on")) => {
                let off = line.split_whitespace().nth(1) == Some("off");
                let mut mode = self.scp.lock().unwrap();
                mode.quirks
                    .retain(|quirk| *quirk != scp::ScpQuirk::AdministrativelyDisabled);
                if off {
                    mode.quirks.push(scp::ScpQuirk::AdministrativelyDisabled);
                }
                drop(mode);
                let text: &[u8] = if off { b"scp off\r\n" } else { b"scp on\r\n" };
                session.data(channel, text.to_vec())?;
            }
            // The connections serving SFTP (the Files tab's): how many, or
            // cut them as a device going down would.
            Some("show") if line.split_whitespace().nth(1) == Some("sftp") => {
                let count = self.sftp_serving.lock().unwrap().len();
                session.data(
                    channel,
                    format!(
                        "sftp connections: {count}

"
                    )
                    .into_bytes(),
                )?;
            }
            Some("cut") if words.next() == Some("sftp") => {
                let handles: Vec<_> = self
                    .sftp_serving
                    .lock()
                    .unwrap()
                    .drain()
                    .map(|(_, h)| h)
                    .collect();
                session.data(
                    channel,
                    format!(
                        "cut {} sftp connections

",
                        handles.len()
                    )
                    .into_bytes(),
                )?;
                for handle in handles {
                    tokio::spawn(async move {
                        let _ = handle
                            .disconnect(
                                russh::Disconnect::ByApplication,
                                "going down".into(),
                                "en".into(),
                            )
                            .await;
                    });
                }
            }
            Some("show") if words.next() == Some("hostname") && device => {
                if let Some(device) = &self.device {
                    session.data(channel, format!("{}\r\n", device.hostname).into_bytes())?;
                }
            }
            Some("show")
                if device
                    && !state.privileged
                    && line.split_whitespace().nth(1) == Some("running-config") =>
            {
                session.data(
                    channel,
                    format!("                ^\r\n{INVALID_INPUT}\r\n\r\n").into_bytes(),
                )?;
            }
            Some("show") if device && line.split_whitespace().nth(1) == Some("running-config") => {
                if let Some(device) = &self.device {
                    let config = device.running_config().replace('\n', "\r\n");
                    let stall = device.config_stall.lock().unwrap().clone();
                    let stalled = stall.and_then(|(line, pause)| {
                        let at = config.find(&format!("\n{line}\r\n"))? + 1 + line.len();
                        Some((at, pause))
                    });
                    if let Some((at, pause)) = stalled {
                        // Up to the end of that line, then its break and
                        // the rest after the pause.
                        let rest = [&config.as_bytes()[at..], &self.prompt(channel)].concat();
                        session.data(channel, config.as_bytes()[..at].to_vec())?;
                        drip_lines(channel, session, vec![rest], pause);
                        return Ok(());
                    }
                    let pace = *device.config_pace.lock().unwrap();
                    if let Some(pace) = pace {
                        // A line at a time, then the prompt.
                        let mut lines: Vec<Vec<u8>> = config
                            .split_inclusive('\n')
                            .map(|line| line.as_bytes().to_vec())
                            .collect();
                        lines.push(self.prompt(channel));
                        drip_lines(channel, session, lines, pace);
                        return Ok(());
                    }
                    session.data(channel, config.into_bytes())?;
                }
            }
            Some("configure") if device => {
                let text = line
                    .trim_start()
                    .strip_prefix("configure")
                    .unwrap_or("")
                    .trim();
                if let Some(device) = &self.device {
                    device.config.lock().unwrap().push(text.to_string());
                }
            }
            Some("echo") if device => {
                let text = line.trim_start().strip_prefix("echo").unwrap_or("").trim();
                session.data(channel, format!("{text}\r\n").into_bytes())?;
            }
            Some("terminal") if device && words.next() == Some("length") => {
                match words.next().and_then(|n| n.parse().ok()) {
                    Some(length) => state.page_length = length,
                    None => session.data(channel, unknown(&line))?,
                }
            }
            Some("enable")
                if device
                    && !state.privileged
                    && self
                        .device
                        .as_ref()
                        .is_some_and(|d| *d.enable.lock().unwrap() != EnableMode::Secret) =>
            {
                let mode = self
                    .device
                    .as_ref()
                    .map(|d| d.enable.lock().unwrap().clone())
                    .unwrap_or_default();
                match mode {
                    EnableMode::NotSet => {
                        session.data(channel, format!("{NO_ENABLE_PASSWORD}\r\n").into_bytes())?
                    }
                    EnableMode::Notice(text) => {
                        session.data(channel, format!("{text}\r\n").into_bytes())?;
                        state.privileged = true;
                    }
                    EnableMode::Secret => unreachable!("asks for the secret"),
                }
            }
            Some("enable") if device && !state.privileged => {
                state.waiting = Waiting::Secret {
                    typed: Vec::new(),
                    tries: 1,
                };
                session.data(channel, ENABLE_QUESTION.as_bytes().to_vec())?;
                return Ok(());
            }
            Some("enable") if device => {}
            Some("disable") if device => state.privileged = false,
            Some("hang") if device => {
                state.waiting = Waiting::Nothing;
                return Ok(());
            }
            Some("drip") if device => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let mut lines: Vec<Vec<u8>> = (1..=count)
                    .map(|n| format!("line {n}\r\n").into_bytes())
                    .collect();
                lines.push(self.prompt(channel));
                drip_lines(channel, session, lines, Duration::from_millis(millis));
                return Ok(());
            }
            // The prompt changes to show `name` (no name: the hostname
            // again), as a device entering or leaving config mode.
            // (`rename` does the same, as a command a job does not know to
            // change the prompt.)
            Some("prompt" | "rename") if device => {
                state.prompt_name = words.next().map(str::to_string);
            }
            // Shows `text` (the rest of the line), says nothing for `ms`,
            // then the line's end, [`HALT_REST`] and the prompt: a table
            // line that ends like a prompt, and a device that pauses there.
            Some("halt") if device => {
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let text = words.collect::<Vec<_>>().join(" ");
                let prompt = self.prompt(channel);
                let rest = [format!("\r\n{HALT_REST}\r\n").as_bytes(), &prompt].concat();
                session.data(channel, text.into_bytes())?;
                drip_lines(channel, session, vec![rest], Duration::from_millis(millis));
                return Ok(());
            }
            // The prompt, then after `ms` a log message on a line of its
            // own and the prompt again (as a device logging to the
            // terminal).
            Some("log") if device => {
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let text = words.collect::<Vec<_>>().join(" ");
                let prompt = self.prompt(channel);
                let message = [format!("\r\n{text}\r\n").as_bytes(), &prompt].concat();
                drip_lines(
                    channel,
                    session,
                    vec![message],
                    Duration::from_millis(millis),
                );
            }
            // A console that answers a byte at a time, `ms` apart, prompt
            // included.
            Some("crawl") if device => {
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(10);
                let text = words.collect::<Vec<_>>().join(" ");
                let prompt = self.prompt(channel);
                let bytes = [text.as_bytes(), b"\r\n", &prompt].concat();
                let lines = bytes.into_iter().map(|byte| vec![byte]).collect();
                drip_lines(channel, session, lines, Duration::from_millis(millis));
                return Ok(());
            }
            // Shows `before`, says nothing for `ms`, then shows `after`, the
            // line's end and the prompt: a device that answers after
            // seconds, half way through a line.
            Some("stall") if device => {
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let before = words.next().unwrap_or("").as_bytes().to_vec();
                let after = words.collect::<Vec<_>>().join(" ");
                let prompt = self.prompt(channel);
                let rest = [after.as_bytes(), b"\r\n", &prompt].concat();
                session.data(channel, before)?;
                drip_lines(channel, session, vec![rest], Duration::from_millis(millis));
                return Ok(());
            }
            Some("pause") if device => {
                let millis: u64 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                state.waiting = Waiting::Nothing;
                let prompt = self.prompt(channel);
                let handle = session.handle();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(millis)).await;
                    let _ = handle.data(channel, prompt).await;
                });
                return Ok(());
            }
            Some(_) if device => session.data(channel, unknown(&line))?,
            _ => {}
        }
        session.data(channel, self.prompt(channel))?;
        Ok(())
    }

    /// Takes a byte typed while a device-mode shell waits for something
    /// other than a line.
    fn take_waiting(
        &mut self,
        channel: ChannelId,
        byte: u8,
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        let Some(state) = self.channels.get_mut(&channel) else {
            return Ok(());
        };
        match &mut state.waiting {
            Waiting::Line | Waiting::Nothing => {}
            Waiting::Secret { typed, tries } if byte == b'\r' => {
                session.data(channel, &b"\r\n"[..])?;
                if typed == ENABLE_SECRET.as_bytes() {
                    state.privileged = true;
                } else if typed == DENIED_SECRET.as_bytes() {
                    session.data(channel, format!("{ACCESS_DENIED}\r\n").into_bytes())?;
                } else if *tries < 3 {
                    *tries += 1;
                    typed.clear();
                    session.data(channel, ENABLE_QUESTION.as_bytes().to_vec())?;
                    return Ok(());
                } else {
                    session.data(channel, format!("{BAD_SECRETS}\r\n").into_bytes())?;
                }
                state.waiting = Waiting::Line;
                session.data(channel, prompt_for(&self.device, state.privileged))?;
            }
            Waiting::Secret { typed, .. } => {
                typed.push(byte);
                if self
                    .device
                    .as_ref()
                    .is_some_and(|d| d.echo_secret.load(Ordering::SeqCst))
                {
                    session.data(channel, vec![byte])?;
                }
            }
            Waiting::Login { typed } if byte == b'\r' => {
                let answer = String::from_utf8_lossy(typed).into_owned();
                typed.clear();
                if let Some(device) = &self.device {
                    device.typed_at_login.lock().unwrap().push(answer.clone());
                }
                session.data(channel, &b"\r\n"[..])?;
                if answer == PASSWORD {
                    state.waiting = Waiting::Line;
                    session.data(channel, prompt_for(&self.device, state.privileged))?;
                } else {
                    session.data(channel, LOGIN_QUESTION.as_bytes().to_vec())?;
                }
            }
            Waiting::Login { typed } => typed.push(byte),
            Waiting::More { next, last } => {
                let (next, last) = (*next, *last);
                session.data(channel, ERASE_MORE)?;
                let end = next.saturating_add(state.page_length.max(1) - 1);
                if byte == b'q' || end >= last {
                    if byte != b'q' {
                        session.data(channel, numbered_range(next, last))?;
                    }
                    state.waiting = Waiting::Line;
                    session.data(channel, prompt_for(&self.device, state.privileged))?;
                } else {
                    session.data(channel, numbered_range(next, end))?;
                    session.data(channel, MORE.as_bytes().to_vec())?;
                    state.waiting = Waiting::More {
                        next: end + 1,
                        last,
                    };
                }
            }
        }
        Ok(())
    }
}

/// Sends `lines` on `channel` one at a time, `pace` apart (the first after
/// `pace`).
fn drip_lines(channel: ChannelId, session: &Session, lines: Vec<Vec<u8>>, pace: Duration) {
    let handle = session.handle();
    tokio::spawn(async move {
        for line in lines {
            tokio::time::sleep(pace).await;
            if handle.data(channel, line).await.is_err() {
                return;
            }
        }
    });
}

/// What the shell prints when it waits for a line: [`PROMPT`], or a
/// device's (privileged or not).
fn prompt_for(device: &Option<Device>, privileged: bool) -> Vec<u8> {
    match device {
        None => PROMPT.to_vec(),
        Some(device) => device.prompt(privileged),
    }
}

/// The bytes a word of hex digits stands for (`c3a9`); bad digits are
/// dropped.
fn decode_hex(word: &str) -> Vec<u8> {
    word.as_bytes()
        .chunks(2)
        .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// `bytes` as the words of hex digits the scripted shell's `raw` takes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What a device-mode shell prints for a line it does not know.
fn unknown(line: &str) -> Vec<u8> {
    format!("{UNKNOWN_COMMAND}{}\r\n", line.trim()).into_bytes()
}

impl Handler for ScriptedShell {
    type Error = russh::Error;

    async fn auth_succeeded(&mut self, _session: &mut Session) -> Result<(), Self::Error> {
        self.logins.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn authentication_banner(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.quirks.auth_banner.lock().unwrap().clone())
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.quirks.at(Stage::Login).await?;
        if user == STALLING_USERNAME {
            return std::future::pending().await;
        }
        if user == USERNAME && password == PASSWORD {
            Ok(Auth::Accept)
        } else if user == KEY_THEN_PASSWORD_USERNAME && password == PASSWORD {
            // Only after the key: a password alone is not enough.
            if std::mem::take(&mut self.key_accepted) {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::reject())
            }
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
        self.quirks.at(Stage::Login).await?;
        if user == THREE_QUESTIONS_USERNAME {
            return Ok(self.three_questions(response));
        }
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
        self.quirks.at(Stage::Login).await?;
        let known = self
            .authorized_keys
            .iter()
            .any(|authorized| authorized.key_data() == key.public_key.key_data());
        if user == USERNAME && known {
            Ok(Auth::Accept)
        } else if user == KEY_THEN_PASSWORD_USERNAME && known {
            self.key_accepted = true;
            Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::Password][..])),
                partial_success: true,
            })
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
        self.quirks.at(Stage::ChannelOpen).await?;
        self.session_channels += 1;
        if self.session_channels > 1 && self.one_channel.load(Ordering::SeqCst) {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let privileged = self.device.as_ref().is_none_or(|d| d.privileged);
        self.channels.insert(
            channel.id(),
            ShellState {
                privileged,
                ..ShellState::default()
            },
        );
        if !self.counted {
            self.counted = true;
            self.open.opened();
        }
        self.unclaimed.insert(channel.id(), channel);
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
        if self.quirks.refuse_forwarding.load(Ordering::SeqCst) {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let Ok(port) = u16::try_from(port_to_connect) else {
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        // Connected on the side, as OpenSSH's sshd does: the connection
        // keeps answering meanwhile (keepalives, other channels), even
        // while the address never answers (#140).
        let host = host_to_connect.to_owned();
        let forwarded = self.forwarded.clone();
        tokio::spawn(async move {
            match TcpStream::connect((host.as_str(), port)).await {
                Ok(mut socket) => {
                    reply.accept().await;
                    forwarded.fetch_add(1, Ordering::Relaxed);
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut socket).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
        });
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
        self.unclaimed.remove(&channel);
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
        self.quirks.at(Stage::ShellRequest).await?;
        self.unclaimed.remove(&channel);
        self.shells.fetch_add(1, Ordering::SeqCst);
        let early = self.quirks.shell_banner.lock().unwrap().clone();
        if let Some(early) = early {
            session.data(channel, early)?;
        }
        session.channel_success(channel)?;
        if self
            .device
            .as_ref()
            .is_some_and(|d| d.login_question.load(Ordering::SeqCst))
        {
            self.channels.entry(channel).or_default().waiting =
                Waiting::Login { typed: Vec::new() };
            return session.data(channel, LOGIN_QUESTION.as_bytes().to_vec());
        }
        let start = self
            .device
            .as_ref()
            .and_then(|d| d.shell_start.lock().unwrap().clone());
        if let Some(pieces) = start {
            let handle = session.handle();
            tokio::spawn(async move {
                for (pause, piece) in pieces {
                    tokio::time::sleep(pause).await;
                    if handle.data(channel, piece).await.is_err() {
                        return;
                    }
                }
            });
            return Ok(());
        }
        session.data(channel, self.prompt(channel))
    }

    /// SFTP as [`TestSshServer::serve_sftp`] and its siblings say; any
    /// other subsystem (NETCONF) is refused at once, so a client that asks
    /// for one fails instead of waiting for an answer.
    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.quirks.at(Stage::Subsystem).await?;
        let mode = self.sftp.lock().unwrap().clone();
        let claimed = self.unclaimed.remove(&channel);
        match (name, mode, claimed) {
            ("sftp", sftp::SftpMode::Serve(root), Some(claimed)) => {
                // Its data is the SFTP server's, not a shell's.
                self.channels.remove(&channel);
                session.channel_success(channel)?;
                self.sftp_serving
                    .lock()
                    .unwrap()
                    .insert(self.connection, session.handle());
                let server = sftp::FolderServer::new(root, self.sftp_quirks.clone());
                russh_sftp::server::run(claimed.into_stream(), server).await;
            }
            ("sftp", sftp::SftpMode::CloseAtOnce, _) => {
                self.channels.remove(&channel);
                session.channel_success(channel)?;
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            ("sftp", sftp::SftpMode::CloseAndHangUp, _) => {
                self.channels.remove(&channel);
                session.channel_success(channel)?;
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
                session.disconnect(russh::Disconnect::ByApplication, "", "en")?;
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
    }

    /// `scp -t` / `scp -f` as [`TestSshServer::serve_scp`] and
    /// [`TestSshServer::scp_quirk`] say; any other command is refused.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mode = self.scp.lock().unwrap().clone();
        let offered = mode.root.is_some() || !mode.quirks.is_empty();
        let command = String::from_utf8_lossy(data).into_owned();
        match (
            scp::parse(&command),
            offered,
            self.unclaimed.remove(&channel),
        ) {
            (Some((to_server, path)), true, Some(claimed)) => {
                // Its data is SCP's, not a shell's.
                self.channels.remove(&channel);
                session.channel_success(channel)?;
                tokio::spawn(scp::serve(
                    claimed,
                    to_server,
                    path,
                    mode,
                    session.handle(),
                    self.scp_running.clone(),
                ));
            }
            _ => session.channel_failure(channel)?,
        }
        Ok(())
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
        let delay = *self.quirks.input_delay.lock().unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        for &byte in data {
            // Enter is CR, LF or CR LF, as a tty's line discipline takes
            // it (scripts such as Netmiko's send LF).
            let state = self.channels.entry(channel).or_default();
            let after_cr = std::mem::replace(&mut state.after_cr, byte == b'\r');
            let byte = match byte {
                b'\n' if after_cr => continue,
                b'\n' => b'\r',
                other => other,
            };
            if byte == CTRL_C {
                let state = self.channels.entry(channel).or_default();
                state.line.clear();
                state.waiting = Waiting::Line;
                match state.flood.take() {
                    // The flood prints the interrupt and prompt as it stops.
                    Some(stop) => stop.store(true, Ordering::Relaxed),
                    None => session.data(channel, [INTERRUPTED, &self.prompt(channel)].concat())?,
                }
            } else if self
                .channels
                .get(&channel)
                .is_some_and(|s| s.waiting != Waiting::Line)
            {
                self.take_waiting(channel, byte, session)?;
            } else if self
                .channels
                .get(&channel)
                .is_some_and(|s| s.flood.is_some())
            {
                // Input typed during a flood is not echoed, like a busy device.
            } else if byte == b'\r' && self.channels[&channel].line == b"freeze" {
                // The whole connection stops answering, keepalives
                // included, like a link that went silent.
                return std::future::pending().await;
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

/// What the scripted shell prints for `lines <count>` (unpaged).
pub fn numbered_lines(count: u32) -> Vec<u8> {
    numbered_range(1, count)
}

/// `line <first>` to `line <last>`, one per row.
pub fn numbered_range(first: u32, last: u32) -> Vec<u8> {
    (first..=last)
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
                // Announced just before `Connected`; tests about it read it
                // with `next_algorithms`.
                Some(SessionEvent::Algorithms(_)) => {}
                Some(SessionEvent::LogFailed(failure)) => {
                    panic!("expected a state change, got the log failure {failure:?}")
                }
                Some(SessionEvent::Backup(outcome)) => {
                    panic!("expected a state change, got the backup outcome {outcome:?}")
                }
                None => panic!("session events ended while waiting for a state change"),
            }
        }
    })
    .await
    .expect("timed out waiting for a state change")
}

/// Waits for the algorithms the session negotiated (announced just before
/// `Connected`), discarding output on the way.
pub async fn next_algorithms(events: &mut SessionEvents) -> Vec<NegotiatedAlgorithms> {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Algorithms(algorithms)) => return algorithms,
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                other => panic!("expected the negotiated algorithms, got {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for the negotiated algorithms")
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

/// Collects (and acknowledges) output until at least `bytes` bytes have
/// come, and returns them.
pub async fn output_of_at_least(events: &mut SessionEvents, bytes: usize) -> Vec<u8> {
    let mut received = Vec::new();
    tokio::time::timeout(WAIT, async {
        while received.len() < bytes {
            match events.next().await {
                Some(SessionEvent::Output(batch)) => {
                    events.acknowledge(batch.len());
                    received.extend_from_slice(&batch);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("only {} of {bytes} bytes came", received.len()));
    received
}

/// Collects (and acknowledges) the output until the session closes, within
/// `bound`: what arrived, and why it closed. Panics on anything else, or if
/// the session is still open after `bound`.
pub async fn output_until_closed(
    events: &mut SessionEvents,
    bound: Duration,
) -> (Vec<u8>, enodix_core::session::CloseReason) {
    let mut received = Vec::new();
    let closed = tokio::time::timeout(bound, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    received.extend_from_slice(&bytes);
                }
                Some(SessionEvent::State(SessionState::Closed(reason))) => return reason,
                Some(SessionEvent::State(_) | SessionEvent::Algorithms(_)) => {}
                other => panic!("expected output or the end, got {other:?}"),
            }
        }
    })
    .await;
    match closed {
        Ok(reason) => (received, reason),
        Err(_) => panic!(
            "still open after {bound:?}; received {} bytes ending {:?}",
            received.len(),
            String::from_utf8_lossy(&received[received.len().saturating_sub(200)..])
        ),
    }
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
