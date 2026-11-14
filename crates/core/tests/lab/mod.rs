//! The containerlab device suite's lab and checks (`tests/containerlab.rs`).
//!
//! A lab file (YAML, never committed: see `lab/lab.example.yaml` and
//! `docs/containerlab.md`) names the devices of your containerlab
//! lab, how to reach them (directly, or through the containerlab server as
//! an SSH jump host) and the logins. Passwords may be in the file or, better,
//! in environment variables it names.
//!
//! The checks open real sessions through the core's public API, the way the
//! app does: a workspace (in a temp folder) with one host file per device,
//! credential profiles in a [`MemoryStore`], targets from
//! [`Profiles::profile_target`], host keys in a [`KnownHosts`] of their own.
//! Each check returns what went wrong instead of panicking, so one device's
//! failure does not hide another's.
//!
//! Device output is matched after removing terminal control sequences
//! ([`plain`]): network operating systems redraw their prompts with cursor
//! movements and colours. Prompts are regular expressions matched against
//! the end of the output received so far.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::logging::{LogFolder, SessionLog};
use netterm_core::session::{
    CloseReason, HostKeyDecision, NegotiatedAlgorithms, OpenError, OutputOptions, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAuth, SshTarget,
    TelnetTarget, TerminalSize, DEFAULT_TELNET_PORT,
};
use netterm_core::workspace::{HostId, Workspace};
use regex::Regex;
use serde::Deserialize;
use tempfile::TempDir;

/// Names the lab file. When it is not set, `lab/lab.yaml` at the repository
/// root is used if it exists.
pub const CONFIG_ENV: &str = "NETTERM_LAB_CONFIG";
/// The device password when a device sets neither `password`,
/// `password_env` nor `agent`.
pub const PASSWORD_ENV: &str = "NETTERM_LAB_PASSWORD";
/// The wrong password the wrong-password check tries on a device that logs
/// in with the agent.
const WRONG_PASSWORD: &str = "netterm-lab-wrong-password";

/// How long output must stay quiet after it ends with the prompt.
pub const SETTLE: Duration = Duration::from_millis(300);

/// What the suite prints when no lab is configured.
pub const SKIP_MESSAGE: &str = "containerlab suite SKIPPED: no lab configured. \
Set NETTERM_LAB_CONFIG to a lab file, or create lab/lab.yaml (not committed) \
from lab/lab.example.yaml; see docs/containerlab.md.";

/// The terminal size sessions open with.
pub const SIZE: TerminalSize = TerminalSize {
    cols: 120,
    rows: 40,
};

/// The lab file used when [`CONFIG_ENV`] is not set.
pub fn default_config_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lab/lab.yaml")
}

/// The lab file to use: the one `env_value` names (it must exist), else
/// `default` if it exists, else `None` (the suite is skipped).
pub fn locate(env_value: Option<&str>, default: &Path) -> Result<Option<PathBuf>, String> {
    match env_value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(named) => {
            let path = PathBuf::from(named);
            if path.is_file() {
                Ok(Some(path))
            } else {
                Err(format!(
                    "{CONFIG_ENV} names {}, which is not a file",
                    path.display()
                ))
            }
        }
        None => Ok(default.is_file().then(|| default.to_path_buf())),
    }
}

/// The lab this process is configured for: `Ok(None)` when there is none
/// (skip), an error when one is configured but cannot be used (fail).
pub fn from_environment() -> Result<Option<Lab>, String> {
    let env_value = std::env::var(CONFIG_ENV).ok();
    let Some(path) = locate(env_value.as_deref(), &default_config_path())? else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read the lab file {}: {error}", path.display()))?;
    Lab::parse(&text, &|name| std::env::var(name).ok())
        .map(Some)
        .map_err(|error| format!("the lab file {} is not usable: {error}", path.display()))
}

/// A device's operating system, for the defaults of its checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Nokia SR Linux.
    Srlinux,
    /// Arista cEOS.
    Ceos,
    /// FRR in a Linux container with an SSH server (`lab/frr/Dockerfile`).
    Frr,
    /// Anything else: every check setting must be given.
    Other,
}

/// What the defaults of a platform are. Verified against SR Linux 24.10.1,
/// cEOS 4.36.0.1F and FRR 10.2.1 in `lab/netterm-test.clab.yml`; a lab file
/// can override each.
struct Defaults {
    username: Option<&'static str>,
    prompt: Option<&'static str>,
    setup: &'static [&'static str],
    command: Option<&'static str>,
    expect: Option<&'static str>,
    large_command: Option<&'static str>,
    large_min_bytes: usize,
    size_command: Option<&'static str>,
    size_output: Option<&'static str>,
    exit: Option<&'static str>,
}

impl Platform {
    fn defaults(self) -> Defaults {
        match self {
            Self::Srlinux => Defaults {
                username: Some("admin"),
                // `A:srl1# ` (the line before it is `--{ running }--[  ]--`).
                prompt: Some(r"[A-Z]:\S+#"),
                setup: &[],
                command: Some("show version"),
                expect: Some(r"(?i)software version"),
                // Megabytes, but generated slowly enough that the device
                // rarely fills the flow-control window.
                large_command: Some("info detail from state /"),
                large_min_bytes: 600_000,
                size_command: Some("bash stty size"),
                size_output: Some("{rows} {cols}"),
                exit: Some("quit"),
            },
            Self::Ceos => Defaults {
                username: Some("admin"),
                // `ceos1>` or `ceos1#`.
                prompt: Some(r"[\w.\-]+(\(\S+\))?[>#]"),
                setup: &["terminal length 0"],
                command: Some("show version"),
                expect: Some(r"(?i)arista"),
                // Needs privilege 15 (the topology's `aaa authorization exec
                // default local`).
                large_command: Some("show running-config all"),
                large_min_bytes: 32 * 1024,
                size_command: Some("bash stty size"),
                size_output: Some("{rows} {cols}"),
                exit: Some("exit"),
            },
            Self::Frr => Defaults {
                username: Some("root"),
                prompt: Some(r"[$#]"),
                setup: &[],
                command: Some("vtysh -c 'show version'"),
                expect: Some(r"FRRouting"),
                large_command: Some("seq 1 100000"),
                large_min_bytes: 500_000,
                size_command: Some("stty size"),
                size_output: Some("{rows} {cols}"),
                exit: Some("exit"),
            },
            Self::Other => Defaults {
                username: None,
                prompt: None,
                setup: &[],
                command: None,
                expect: None,
                large_command: None,
                large_min_bytes: 0,
                size_command: None,
                size_output: None,
                exit: None,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LabFile {
    /// Seconds to wait for each answer from a device (default 30).
    timeout: Option<u64>,
    jump_host: Option<JumpFile>,
    devices: Vec<DeviceFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JumpFile {
    address: String,
    port: Option<u16>,
    username: String,
    password: Option<String>,
    password_env: Option<String>,
    key_file: Option<PathBuf>,
    passphrase_env: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceFile {
    name: String,
    platform: Platform,
    address: String,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    /// Log in with the keys of this computer's SSH agent instead.
    agent: Option<bool>,
    /// Also check telnet, on this port (`true` = 23).
    telnet: Option<TelnetSetting>,
    /// Where telnet is reached directly, not through the jump host (such
    /// as the containerlab server, publishing the device's port 23).
    telnet_address: Option<String>,
    prompt: Option<String>,
    setup: Option<Vec<String>>,
    command: Option<String>,
    expect: Option<String>,
    large_command: Option<String>,
    large_min_bytes: Option<usize>,
    size_command: Option<String>,
    size_output: Option<String>,
    exit: Option<String>,
    login_prompt: Option<String>,
    password_prompt: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TelnetSetting {
    Enabled(bool),
    Port(u16),
}

/// The lab: its devices and how to reach them.
#[derive(Debug)]
pub struct Lab {
    /// How long to wait for each answer from a device.
    pub timeout: Duration,
    /// The containerlab server, when the devices are reached through it.
    pub jump_host: Option<LabJumpHost>,
    pub devices: Vec<Device>,
}

pub struct LabJumpHost {
    pub address: String,
    pub port: u16,
    pub username: String,
    pub auth: ProfileAuth,
}

impl std::fmt::Debug for LabJumpHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.username, self.address, self.port)
    }
}

/// One device and how to check it.
pub struct Device {
    /// Also its host file's stem in the suite's workspace.
    pub name: String,
    pub platform: Platform,
    pub address: String,
    pub port: u16,
    pub username: String,
    /// `None`: the device logs in with the SSH agent's keys.
    pub password: Option<String>,
    /// The telnet port, when telnet is checked too.
    pub telnet_port: Option<u16>,
    /// Telnet's own address, reached directly; `None`: `address`, which
    /// behind a jump host means telnet is not checked (it is not tunnelled).
    pub telnet_address: Option<String>,
    /// Matched against the end of the output: the device is ready for a line.
    pub prompt: Regex,
    /// Lines sent after logging in, each waiting for the prompt (such as
    /// `terminal length 0`).
    pub setup: Vec<String>,
    pub command: String,
    /// Must match somewhere in the command's output.
    pub expect: Regex,
    /// A command with a lot of output, and the least it must print.
    pub large: Option<(String, usize)>,
    /// A command that prints the terminal size, and its output as a
    /// template with `{cols}` and `{rows}`.
    pub size: Option<(String, String)>,
    /// The line that ends the session from the device's side.
    pub exit: String,
    /// Telnet: the device asking for the username, then the password.
    pub login_prompt: Regex,
    pub password_prompt: Regex,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({:?}, {}@{}:{})",
            self.name, self.platform, self.username, self.address, self.port
        )
    }
}

impl Device {
    /// The device's host id in the suite's workspace.
    pub fn host_id(&self) -> HostId {
        HostId::new(format!("{}.yaml", self.name))
    }
}

/// Compiles a prompt: `pattern`, then optional blanks, at the very end.
fn prompt_regex(pattern: &str) -> Result<Regex, String> {
    Regex::new(&format!(r"(?:{pattern})[ \t]*\z"))
        .map_err(|error| format!("bad prompt pattern {pattern:?}: {error}"))
}

impl Lab {
    /// Reads a lab file. `env` looks up environment variables (the
    /// passwords a file names).
    pub fn parse(text: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Lab, String> {
        let file: LabFile = serde_norway::from_str(text).map_err(|error| error.to_string())?;
        if file.devices.is_empty() {
            return Err("it lists no devices".into());
        }
        let secret = |value: Option<String>, named: Option<String>, fallback: &str, what: &str| {
            if let Some(value) = value {
                return Ok(value);
            }
            let variable = named.unwrap_or_else(|| fallback.to_owned());
            env(&variable)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{what}: set the environment variable {variable}"))
        };
        let jump_host = match file.jump_host {
            None => None,
            Some(jump) => {
                // The agent (this computer's own keys) unless a password
                // or a key file is given.
                let auth = if let Some(key_file) = jump.key_file {
                    ProfileAuth::PrivateKey {
                        key_file,
                        passphrase: match jump.passphrase_env {
                            None => None,
                            Some(variable) => Some(env(&variable).ok_or_else(|| {
                                format!(
                                    "the jump host's key: set the environment variable {variable}"
                                )
                            })?),
                        },
                    }
                } else if jump.password.is_some() || jump.password_env.is_some() {
                    ProfileAuth::Password {
                        password: secret(
                            jump.password,
                            jump.password_env,
                            "",
                            "the jump host has no password",
                        )?,
                    }
                } else {
                    ProfileAuth::Agent
                };
                Some(LabJumpHost {
                    address: jump.address,
                    port: jump.port.unwrap_or(22),
                    username: jump.username,
                    auth,
                })
            }
        };
        let mut devices: Vec<Device> = Vec::new();
        for device in file.devices {
            let name = device.name;
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!(
                    "device name {name:?}: use letters, digits, '-' and '_' only"
                ));
            }
            // `_*` files are the workspace's own; the jump host has its name.
            if name.starts_with('_') || name.eq_ignore_ascii_case(JUMP_PROFILE) {
                return Err(format!("device name {name:?} is reserved by the suite"));
            }
            if device.address.is_empty()
                || device
                    .address
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
            {
                return Err(format!("device {name}: bad address {:?}", device.address));
            }
            if devices.iter().any(|d| d.name.eq_ignore_ascii_case(&name)) {
                return Err(format!("device {name} is listed twice"));
            }
            let defaults = device.platform.defaults();
            let required = |value: Option<String>, default: Option<&str>, key: &str| {
                value
                    .or_else(|| default.map(str::to_owned))
                    .ok_or_else(|| format!("device {name}: set `{key}` (platform `other`)"))
            };
            let username = required(device.username, defaults.username, "username")?;
            let prompt = prompt_regex(&required(device.prompt, defaults.prompt, "prompt")?)
                .map_err(|error| format!("device {name}: {error}"))?;
            let command = required(device.command, defaults.command, "command")?;
            let expect = required(device.expect, defaults.expect, "expect")?;
            let expect = Regex::new(&expect)
                .map_err(|error| format!("device {name}: bad `expect` pattern: {error}"))?;
            let exit = required(device.exit, defaults.exit, "exit")?;
            let password = if device.agent == Some(true) {
                if device.password.is_some() || device.password_env.is_some() {
                    return Err(format!(
                        "device {name}: set `agent` or a password, not both"
                    ));
                }
                None
            } else {
                Some(secret(
                    device.password,
                    device.password_env,
                    PASSWORD_ENV,
                    &format!("device {name} has no password"),
                )?)
            };
            let large = device
                .large_command
                .or_else(|| defaults.large_command.map(str::to_owned))
                .map(|command| {
                    let least = device.large_min_bytes.unwrap_or(defaults.large_min_bytes);
                    (command, least)
                });
            let size_command = device
                .size_command
                .or_else(|| defaults.size_command.map(str::to_owned));
            let size_output = device
                .size_output
                .or_else(|| defaults.size_output.map(str::to_owned));
            let size = match (size_command, size_output) {
                (Some(command), Some(output)) => Some((command, output)),
                (None, None) => None,
                _ => {
                    return Err(format!(
                        "device {name}: set both `size_command` and `size_output`, or neither"
                    ))
                }
            };
            let telnet_port = match device.telnet {
                None | Some(TelnetSetting::Enabled(false)) => None,
                Some(TelnetSetting::Enabled(true)) => Some(DEFAULT_TELNET_PORT),
                Some(TelnetSetting::Port(port)) => Some(port),
            };
            let login_prompt = prompt_regex(
                device
                    .login_prompt
                    .as_deref()
                    .unwrap_or(r"(?i)(login|username):"),
            )
            .map_err(|error| format!("device {name}: {error}"))?;
            let password_prompt = prompt_regex(
                device
                    .password_prompt
                    .as_deref()
                    .unwrap_or(r"(?i)password:"),
            )
            .map_err(|error| format!("device {name}: {error}"))?;
            devices.push(Device {
                platform: device.platform,
                address: device.address,
                port: device.port.unwrap_or(22),
                username,
                password,
                telnet_port,
                telnet_address: device.telnet_address,
                prompt,
                setup: device
                    .setup
                    .unwrap_or_else(|| defaults.setup.iter().map(|s| (*s).to_owned()).collect()),
                command,
                expect,
                large,
                size,
                exit,
                login_prompt,
                password_prompt,
                name,
            });
        }
        Ok(Lab {
            timeout: Duration::from_secs(file.timeout.unwrap_or(30).max(1)),
            jump_host,
            devices,
        })
    }
}

/// The profile name of the jump host in the suite's workspace.
const JUMP_PROFILE: &str = "clab-server";
/// The jump host's host id in the suite's workspace.
const JUMP_HOST_FILE: &str = "clab-server.yaml";

/// A lab made ready to check: a workspace with one host per device (and the
/// jump host), credential profiles for each, and temp folders for host keys
/// and logs.
pub struct LabRun {
    pub lab: Lab,
    temp: TempDir,
    workspace: Workspace,
    store: MemoryStore,
}

impl LabRun {
    pub fn new(lab: Lab) -> Result<LabRun, String> {
        let temp = TempDir::new().map_err(|error| error.to_string())?;
        let root = temp.path().join("containerlab");
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        let write = |file: &str, text: String| {
            fs::write(root.join(file), text).map_err(|error| error.to_string())
        };
        if let Some(jump) = &lab.jump_host {
            write("_folder.yaml", format!("jump_host: {JUMP_HOST_FILE}\n"))?;
            write(
                JUMP_HOST_FILE,
                format!(
                    "address: {}\nport: {}\ncredential_profile: {JUMP_PROFILE}\nrole: server\n",
                    yaml_text(&jump.address),
                    jump.port
                ),
            )?;
        }
        for device in &lab.devices {
            write(
                &format!("{}.yaml", device.name),
                format!(
                    "address: {}\nport: {}\ncredential_profile: {}\n",
                    yaml_text(&device.address),
                    device.port,
                    device.name
                ),
            )?;
        }
        let workspace = Workspace::open(&root).map_err(|error| error.to_string())?;
        if !workspace.problems().is_empty() {
            return Err(format!(
                "the suite's workspace has problems: {:?}",
                workspace.problems()
            ));
        }
        let store = MemoryStore::new();
        let scope = workspace.credential_scope();
        if let Some(jump) = &lab.jump_host {
            let profile = CredentialProfile {
                name: JUMP_PROFILE.into(),
                username: jump.username.clone(),
                auth: jump.auth.clone(),
                enable_secret: None,
            };
            store.put(&scope, &profile).map_err(|error| error.0)?;
        }
        for device in &lab.devices {
            let profile = CredentialProfile {
                name: device.name.clone(),
                username: device.username.clone(),
                auth: match &device.password {
                    Some(password) => ProfileAuth::Password {
                        password: password.clone(),
                    },
                    None => ProfileAuth::Agent,
                },
                enable_secret: None,
            };
            store.put(&scope, &profile).map_err(|error| error.0)?;
        }
        Ok(LabRun {
            lab,
            temp,
            workspace,
            store,
        })
    }

    /// The SSH target the app would use for `device`, jump host included.
    pub fn ssh_target(&self, device: &Device) -> Result<SshTarget, String> {
        Profiles::new(&self.store, &self.workspace)
            .profile_target(&device.host_id())
            .map_err(|error| format!("no SSH target: {error}"))
    }

    /// A session manager with an empty host-key store of its own.
    pub fn manager(&self) -> SessionManager {
        SessionManager::new(KnownHosts::in_memory())
    }

    fn timeout(&self) -> Duration {
        self.lab.timeout
    }

    fn log(&self, device: &Device) -> Result<SessionLog, String> {
        LogFolder::new(self.temp.path().join("logs"))
            .host_log(&self.workspace, &device.host_id())
            .ok_or_else(|| "logging is off for the device".to_owned())
    }
}

/// `text` as a YAML double-quoted scalar; [`Lab::parse`] refuses addresses
/// with characters this would not quote correctly.
fn yaml_text(text: &str) -> String {
    format!("{:?}", text)
}

/// What [`run_checks`] found.
pub struct Results {
    /// How many (device, check) pairs were checked (passed or failed).
    pub checked: usize,
    /// The failures, as `device: check: reason` lines.
    pub failures: Vec<String>,
}

/// Runs every check on every device, printing each result.
pub async fn run_checks(run: &LabRun, checks: &[Check]) -> Results {
    let mut results = Results {
        checked: 0,
        failures: Vec::new(),
    };
    for device in &run.lab.devices {
        for check in checks {
            let outcome = check.run(run, device).await;
            match &outcome {
                Outcome::Passed(note) => {
                    results.checked += 1;
                    eprintln!(
                        "[lab] {} / {}: ok{}",
                        device.name,
                        check.name(),
                        note_suffix(note)
                    )
                }
                Outcome::NotApplicable(why) => {
                    eprintln!(
                        "[lab] {} / {}: not checked ({why})",
                        device.name,
                        check.name()
                    )
                }
                Outcome::Failed(why) => {
                    results.checked += 1;
                    eprintln!("[lab] {} / {}: FAILED: {why}", device.name, check.name());
                    results
                        .failures
                        .push(format!("{}: {}: {why}", device.name, check.name()));
                }
            }
        }
    }
    results
}

fn note_suffix(note: &str) -> String {
    if note.is_empty() {
        String::new()
    } else {
        format!(" ({note})")
    }
}

/// The result of one check on one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed(String),
    NotApplicable(String),
    Failed(String),
}

impl From<Result<String, String>> for Outcome {
    fn from(result: Result<String, String>) -> Self {
        match result {
            Ok(note) => Outcome::Passed(note),
            Err(why) => Outcome::Failed(why),
        }
    }
}

/// The checks the suite makes on each device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// Log in with the profile's password, run the command, see its
    /// output, end the session from the device's side.
    LoginAndCommand,
    /// The first connection asks about the host key (the jump host's too);
    /// once remembered, the next connection does not.
    HostKeys,
    /// A wrong password is reported as an authentication failure.
    WrongPassword,
    /// The device sees the terminal size, and a resize.
    Resize,
    /// A large output arrives whole while the reader stalls (flow control).
    LargeOutput,
    /// The session log holds the command output as plain text.
    SessionLog,
    /// Telnet login (typed in the terminal) and the command.
    Telnet,
}

pub const ALL_CHECKS: &[Check] = &[
    Check::LoginAndCommand,
    Check::HostKeys,
    Check::WrongPassword,
    Check::Resize,
    Check::LargeOutput,
    Check::SessionLog,
    Check::Telnet,
];

impl Check {
    pub fn name(self) -> &'static str {
        match self {
            Check::LoginAndCommand => "ssh login and command",
            Check::HostKeys => "host keys",
            Check::WrongPassword => "wrong password",
            Check::Resize => "terminal size",
            Check::LargeOutput => "large output with flow control",
            Check::SessionLog => "session log",
            Check::Telnet => "telnet",
        }
    }

    pub async fn run(self, run: &LabRun, device: &Device) -> Outcome {
        match self {
            Check::LoginAndCommand => login_and_command(run, device).await.into(),
            Check::HostKeys => host_keys(run, device).await.into(),
            Check::WrongPassword => wrong_password(run, device).await.into(),
            Check::Resize => match &device.size {
                None => Outcome::NotApplicable("no size_command for this device".into()),
                Some((command, output)) => resize(run, device, command, output).await.into(),
            },
            Check::LargeOutput => match &device.large {
                None => Outcome::NotApplicable("no large_command for this device".into()),
                Some((command, least)) => large_output(run, device, command, *least).await.into(),
            },
            Check::SessionLog => session_log(run, device).await.into(),
            Check::Telnet => match device.telnet_port {
                None => Outcome::NotApplicable("telnet is not enabled for this device".into()),
                Some(_) if run.lab.jump_host.is_some() && device.telnet_address.is_none() => {
                    Outcome::NotApplicable(
                        "telnet is not tunnelled through the jump host (set telnet_address)".into(),
                    )
                }
                Some(port) => match &device.password {
                    None => Outcome::NotApplicable(
                        "the device logs in with the agent; telnet needs a password".into(),
                    ),
                    Some(password) => telnet(run, device, port, password).await.into(),
                },
            },
        }
    }
}

async fn login_and_command(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.manager();
    let target = run.ssh_target(device)?;
    let hops = target.jump_hosts.len();
    let mut session = Conversation::open_ssh(&manager, target, None, run.timeout()).await?;
    let algorithms = session.algorithms.clone();
    if algorithms.len() != hops + 1 {
        return Err(format!(
            "expected the algorithms of {} connections, got {algorithms:?}",
            hops + 1
        ));
    }
    session.ready(device).await?;
    let output = session.run(&device.command, &device.prompt).await?;
    if !device.expect.is_match(&output) {
        return Err(format!(
            "the output of {:?} does not match {:?}: {output:?}",
            device.command,
            device.expect.as_str()
        ));
    }
    session.exit(&device.exit).await?;
    let used = algorithms.last().expect("one connection at least");
    Ok(format!(
        "{}, {}, {}, {}{}",
        used.key_exchange,
        used.host_key,
        used.cipher,
        used.mac,
        if used.legacy.is_empty() {
            String::new()
        } else {
            format!(", legacy: {}", used.legacy.join(" "))
        }
    ))
}

async fn host_keys(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.manager();
    let target = run.ssh_target(device)?;
    let mut expected: Vec<(String, u16)> = target
        .jump_hosts
        .iter()
        .map(|jump| (jump.host.clone(), jump.port))
        .collect();
    expected.push((target.host.clone(), target.port));

    let mut first = Conversation::open_ssh(&manager, target.clone(), None, run.timeout()).await?;
    let asked = first.asked.clone();
    first.close().await?;
    if asked != expected {
        return Err(format!(
            "the first connection should ask about {expected:?}, it asked about {asked:?}"
        ));
    }
    let mut second = Conversation::open_ssh(&manager, target, None, run.timeout()).await?;
    let asked = second.asked.clone();
    second.ready(device).await?;
    second.close().await?;
    if !asked.is_empty() {
        return Err(format!("the second connection asked again about {asked:?}"));
    }
    Ok(String::new())
}

async fn wrong_password(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.manager();
    let mut target = run.ssh_target(device)?;
    let wrong = match &device.password {
        Some(password) => format!("{password}-not-this-one"),
        None => WRONG_PASSWORD.to_owned(),
    };
    target.auth = SshAuth::Password(wrong.clone());
    match Conversation::connect(
        &manager,
        SessionTarget::Ssh(target),
        None,
        run.timeout(),
        &wrong,
    )
    .await
    {
        Ok(mut session) => {
            session.close().await?;
            Err("the session connected with a wrong password".into())
        }
        Err(Failure::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))) => {
            Ok(String::new())
        }
        Err(other) => Err(format!("expected an authentication failure, got: {other}")),
    }
}

async fn resize(
    run: &LabRun,
    device: &Device,
    command: &str,
    template: &str,
) -> Result<String, String> {
    let manager = run.manager();
    let mut session =
        Conversation::open_ssh(&manager, run.ssh_target(device)?, None, run.timeout()).await?;
    session.ready(device).await?;
    for size in [
        SIZE,
        TerminalSize {
            cols: 132,
            rows: 43,
        },
    ] {
        if size != SIZE {
            manager
                .resize(session.id, size)
                .map_err(|error| format!("resize: {error:?}"))?;
        }
        let expected = template
            .replace("{cols}", &size.cols.to_string())
            .replace("{rows}", &size.rows.to_string());
        let output = session.run(command, &device.prompt).await?;
        if !output.contains(&expected) {
            return Err(format!(
                "at {}x{}, {command:?} printed {output:?}, not {expected:?}",
                size.cols, size.rows
            ));
        }
    }
    session.close().await?;
    Ok(String::new())
}

async fn large_output(
    run: &LabRun,
    device: &Device,
    command: &str,
    least: usize,
) -> Result<String, String> {
    let manager = run.manager();
    let window = OutputOptions::default().window;
    // A batch can be read on top of a full window.
    let batch_size = OutputOptions::default().batch_size;
    let mut session =
        Conversation::open_ssh(&manager, run.ssh_target(device)?, None, run.timeout()).await?;
    session.ready(device).await?;
    session.send_line(command)?;
    // Stop reading, as a terminal that cannot keep up: the session must
    // stop at about the flow-control window.
    let stalled = session.stall(Duration::from_secs(3)).await?;
    if stalled > window + batch_size {
        return Err(format!(
            "{stalled} bytes arrived unacknowledged; flow control should stop at about {window}"
        ));
    }
    let started = std::time::Instant::now();
    let output = session.wait_for(&device.prompt).await?;
    let elapsed = started.elapsed();
    session.close().await?;
    if output.len() < least {
        return Err(format!(
            "{command:?} printed {} bytes, expected at least {least}",
            output.len()
        ));
    }
    let exercised = if stalled >= window {
        ""
    } else {
        "; the output fits in the window, so flow control was not exercised (set a larger large_command)"
    };
    Ok(format!(
        "{} bytes of text, {stalled} held while stalled, the rest in {:.1} s{exercised}",
        output.len(),
        elapsed.as_secs_f64()
    ))
}

async fn session_log(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.manager();
    let log = run.log(device)?;
    let folder = log.folder().to_path_buf();
    let mut session =
        Conversation::open_ssh(&manager, run.ssh_target(device)?, Some(log), run.timeout()).await?;
    session.ready(device).await?;
    session.run(&device.command, &device.prompt).await?;
    session.exit(&device.exit).await?;
    let files: Vec<PathBuf> = fs::read_dir(&folder)
        .map_err(|error| format!("no log folder {}: {error}", folder.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .collect();
    let [file] = files.as_slice() else {
        return Err(format!(
            "expected one log file in {}, found {files:?}",
            folder.display()
        ));
    };
    let text = fs::read_to_string(file).map_err(|error| error.to_string())?;
    if text.contains('\x1b') {
        return Err("the log holds terminal control sequences".into());
    }
    if !device.expect.is_match(&text) {
        return Err(format!(
            "the log does not match {:?}: {text:?}",
            device.expect.as_str()
        ));
    }
    Ok(format!("{} bytes", text.len()))
}

async fn telnet(
    run: &LabRun,
    device: &Device,
    port: u16,
    password: &str,
) -> Result<String, String> {
    let manager = run.manager();
    let target = SessionTarget::Telnet(TelnetTarget {
        host: device
            .telnet_address
            .clone()
            .unwrap_or_else(|| device.address.clone()),
        port,
        connect_timeout: run.timeout(),
    });
    let mut session = Conversation::connect(&manager, target, None, run.timeout(), password)
        .await
        .map_err(|failure| failure.to_string())?;
    session.wait_for(&device.login_prompt).await?;
    session.send_line(&device.username)?;
    session.wait_for(&device.password_prompt).await?;
    session.send_line(password)?;
    session.wait_for(&device.prompt).await?;
    for line in &device.setup {
        session.run(line, &device.prompt).await?;
    }
    let output = session.run(&device.command, &device.prompt).await?;
    if !device.expect.is_match(&output) {
        return Err(format!(
            "the output of {:?} does not match {:?}: {output:?}",
            device.command,
            device.expect.as_str()
        ));
    }
    session.exit(&device.exit).await?;
    Ok(String::new())
}

/// Why a session did not connect.
#[derive(Debug)]
pub enum Failure {
    Closed(CloseReason),
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Closed(reason) => write!(f, "closed: {reason} ({reason:?})"),
            Failure::Other(why) => f.write_str(why),
        }
    }
}

/// A session being driven like a user at a terminal.
pub struct Conversation<'a> {
    manager: &'a SessionManager,
    pub id: SessionId,
    events: SessionEvents,
    timeout: Duration,
    /// Output received and not yet returned by a wait.
    received: Vec<u8>,
    /// The (host, port) of every host-key question, in order; each was
    /// answered with "trust and remember".
    pub asked: Vec<(String, u16)>,
    pub algorithms: Vec<NegotiatedAlgorithms>,
}

impl<'a> Conversation<'a> {
    /// Opens an SSH session and waits for it to connect; any failure is
    /// an error.
    pub async fn open_ssh(
        manager: &'a SessionManager,
        target: SshTarget,
        log: Option<SessionLog>,
        timeout: Duration,
    ) -> Result<Conversation<'a>, String> {
        let password = match &target.auth {
            SshAuth::Password(password) => password.clone(),
            _ => String::new(),
        };
        Self::connect(manager, SessionTarget::Ssh(target), log, timeout, &password)
            .await
            .map_err(|failure| format!("did not connect: {failure}"))
    }

    /// Opens a session and waits until it is connected. Host keys are
    /// trusted and remembered; hidden login questions (keyboard-interactive)
    /// are answered with `password`, at most three rounds.
    pub async fn connect(
        manager: &'a SessionManager,
        target: SessionTarget,
        log: Option<SessionLog>,
        timeout: Duration,
        password: &str,
    ) -> Result<Conversation<'a>, Failure> {
        let (id, events) = manager.open_logged(target, SIZE, log);
        let mut session = Conversation {
            manager,
            id,
            events,
            timeout,
            received: Vec::new(),
            asked: Vec::new(),
            algorithms: Vec::new(),
        };
        let mut rounds = 0;
        loop {
            match session.next_event().await.map_err(Failure::Other)? {
                SessionEvent::State(SessionState::Connecting) => {}
                SessionEvent::State(SessionState::Connected) => return Ok(session),
                SessionEvent::State(SessionState::Closed(reason)) => {
                    return Err(Failure::Closed(reason))
                }
                SessionEvent::HostKey(question) => {
                    session.asked.push((question.host.clone(), question.port));
                    manager
                        .decide_host_key(id, HostKeyDecision::TrustAndRemember)
                        .map_err(|error| Failure::Other(format!("{error:?}")))?;
                }
                SessionEvent::Prompt(prompt) => {
                    rounds += 1;
                    if rounds > 3 {
                        let _ = manager.close(id);
                        return Err(Failure::Other(format!(
                            "the device keeps asking login questions: {prompt:?}"
                        )));
                    }
                    let answers = prompt
                        .questions
                        .iter()
                        .map(|question| {
                            if question.echo {
                                String::new()
                            } else {
                                password.to_owned()
                            }
                        })
                        .collect();
                    manager
                        .answer(id, answers)
                        .map_err(|error| Failure::Other(format!("{error:?}")))?;
                }
                SessionEvent::Algorithms(algorithms) => session.algorithms = algorithms,
                SessionEvent::Backup(_) => {}
                SessionEvent::Output(bytes) => session.received.extend_from_slice(&bytes),
                SessionEvent::LogFailed(failure) => {
                    return Err(Failure::Other(format!("log failed: {failure}")))
                }
            }
        }
    }

    /// The next event within `wait` (output is acknowledged at once), or
    /// `None` if there is none by then.
    async fn event_within(&mut self, wait: Duration) -> Result<Option<SessionEvent>, String> {
        match tokio::time::timeout(wait, self.events.next()).await {
            Err(_) => Ok(None),
            Ok(None) => Err("the session's events ended".into()),
            Ok(Some(event)) => {
                if let SessionEvent::Output(bytes) = &event {
                    self.events.acknowledge(bytes.len());
                }
                Ok(Some(event))
            }
        }
    }

    /// The next event (output is acknowledged at once), or an error after
    /// the timeout.
    async fn next_event(&mut self) -> Result<SessionEvent, String> {
        match tokio::time::timeout(self.timeout, self.events.next()).await {
            Err(_) => Err(format!(
                "nothing happened for {} s; last output: {:?}",
                self.timeout.as_secs(),
                tail(&plain(&self.received), 400)
            )),
            Ok(None) => Err("the session's events ended".into()),
            Ok(Some(event)) => {
                if let SessionEvent::Output(bytes) = &event {
                    self.events.acknowledge(bytes.len());
                }
                Ok(event)
            }
        }
    }

    /// Waits for the first prompt, then sends the device's setup lines.
    pub async fn ready(&mut self, device: &Device) -> Result<(), String> {
        self.wait_for(&device.prompt)
            .await
            .map_err(|error| format!("no prompt after logging in: {error}"))?;
        for line in &device.setup {
            self.run(line, &device.prompt).await?;
        }
        Ok(())
    }

    /// Sends `line` and Enter. Output not yet waited for (a prompt the
    /// device redrew, say) is dropped first, so it cannot pass for the
    /// answer.
    pub fn send_line(&mut self, line: &str) -> Result<(), String> {
        self.received.clear();
        self.manager
            .write(self.id, format!("{line}\r").into_bytes())
            .map_err(|error| format!("write: {error:?}"))
    }

    /// Sends `line` and returns the plain text up to the next prompt.
    pub async fn run(&mut self, line: &str, prompt: &Regex) -> Result<String, String> {
        self.send_line(line)?;
        self.wait_for(prompt)
            .await
            .map_err(|error| format!("after {line:?}: {error}"))
    }

    /// Collects output until its plain text ends with `prompt` and nothing
    /// more arrives for [`SETTLE`] (so output that merely ends in `#` for a
    /// moment is not taken for the prompt), and returns that text. The
    /// timeout applies to each wait for more output.
    pub async fn wait_for(&mut self, prompt: &Regex) -> Result<String, String> {
        loop {
            // Escape sequences can be cut where the tail starts; that only
            // spoils the start of the tail, and prompts are at its end.
            let from = self.received.len().saturating_sub(4096);
            if prompt.is_match(&plain(&self.received[from..])) {
                match self.event_within(SETTLE).await? {
                    None => {
                        let text = plain(&self.received);
                        self.received.clear();
                        return Ok(text);
                    }
                    Some(SessionEvent::Output(bytes)) => {
                        self.received.extend_from_slice(&bytes);
                        continue;
                    }
                    Some(other) => return Err(format!("at the prompt: unexpected {other:?}")),
                }
            }
            match self.next_event().await? {
                SessionEvent::Output(bytes) => self.received.extend_from_slice(&bytes),
                SessionEvent::State(SessionState::Closed(reason)) => {
                    return Err(format!(
                        "the session closed ({reason}) while waiting for {:?}; last output: {:?}",
                        prompt.as_str(),
                        tail(&plain(&self.received), 400)
                    ))
                }
                SessionEvent::LogFailed(failure) => return Err(format!("log failed: {failure}")),
                other => return Err(format!("unexpected {other:?}")),
            }
        }
    }

    /// Reads output for `duration` without acknowledging it (a terminal
    /// that has stopped consuming), then acknowledges it all; returns how
    /// many bytes arrived meanwhile.
    pub async fn stall(&mut self, duration: Duration) -> Result<usize, String> {
        let until = tokio::time::Instant::now() + duration;
        let mut held = 0;
        loop {
            match tokio::time::timeout_at(until, self.events.next()).await {
                Err(_) => break,
                Ok(Some(SessionEvent::Output(bytes))) => {
                    held += bytes.len();
                    self.received.extend_from_slice(&bytes);
                }
                Ok(other) => return Err(format!("while stalled: unexpected {other:?}")),
            }
        }
        self.events.acknowledge(held);
        Ok(held)
    }

    /// Sends the exit line and waits for the device to end the session.
    pub async fn exit(mut self, line: &str) -> Result<(), String> {
        self.send_line(line)?;
        loop {
            match self.next_event().await? {
                SessionEvent::Output(bytes) => self.received.extend_from_slice(&bytes),
                SessionEvent::State(SessionState::Closed(CloseReason::RemoteExited { .. })) => {
                    return Ok(())
                }
                SessionEvent::State(SessionState::Closed(reason)) => {
                    return Err(format!("after {line:?} the session closed with {reason:?}"))
                }
                other => return Err(format!("after {line:?}: unexpected {other:?}")),
            }
        }
    }

    /// Closes the session from this side and waits until it is closed.
    pub async fn close(&mut self) -> Result<(), String> {
        self.manager
            .close(self.id)
            .map_err(|error| format!("close: {error:?}"))?;
        loop {
            match self.next_event().await? {
                SessionEvent::State(SessionState::Closed(_)) => return Ok(()),
                SessionEvent::Output(_) => {}
                other => return Err(format!("while closing: unexpected {other:?}")),
            }
        }
    }
}

/// The text of terminal output without control sequences: CSI and OSC
/// sequences, other escapes, carriage returns and control characters
/// other than line feeds and tabs are dropped.
pub fn plain(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b => {
                i += 1;
                match bytes.get(i) {
                    Some(b'[') => {
                        i += 1;
                        while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                            i += 1;
                        }
                        i += 1;
                    }
                    Some(b']') => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    Some(_) => i += 1,
                    None => {}
                }
            }
            b'\n' | b'\t' => {
                out.push(bytes[i]);
                i += 1;
            }
            byte if byte < 0x20 || byte == 0x7f => i += 1,
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The last `chars` characters of `text`.
fn tail(text: &str, chars: usize) -> &str {
    let count = text.chars().count();
    match text.char_indices().nth(count.saturating_sub(chars)) {
        Some((index, _)) => &text[index..],
        None => text,
    }
}
