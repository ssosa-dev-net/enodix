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

pub mod enrichment;
pub mod files;
pub mod scripts;
pub mod tunnel_ports;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use netterm_core::configs::{ConfigStore, SaveOutcome};
use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::diff::DiffLine;
use netterm_core::job::{
    self, HostResult, HostState, JobEvent, JobOptions, JobStep, JobTarget, LoginBackup,
    LoginBackupOutcome, SessionLogin, JOB_TERMINAL_SIZE,
};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::logging::{LogFolder, SessionLog};
use netterm_core::session::{
    CloseReason, HostKeyDecision, NegotiatedAlgorithms, OpenError, OpenOptions, OutputOptions,
    SessionEvent, SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAuth,
    SshTarget, TelnetTarget, TerminalSize, DEFAULT_TELNET_PORT,
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
    /// FRR in a Linux container with an SSH server (`lab/frr/Dockerfile`),
    /// logged in to as root (a shell).
    Frr,
    /// The same FRR container logged in to as its `vtysh` user, whose
    /// login shell is vtysh: FRR as a router's CLI.
    Vtysh,
    /// Cisco IOS or IOS-XE (the topology's CSR 1000v).
    Ios,
    /// Cisco NX-OS.
    Nxos,
    /// Juniper Junos (the topology's cJunos Evolved).
    Junos,
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
    /// The built-in platform definition the device is driven by in jobs.
    definition: Option<&'static str>,
    /// A command that shows the privilege level, and what its output must
    /// match once the job has entered privileged mode.
    privilege_check: Option<(&'static str, &'static str)>,
    /// What the last line of the fetched configuration must match.
    config_end: Option<&'static str>,
    /// A command printing more lines than a job's terminal has rows, which
    /// a device that pages would stop in the middle of.
    paging_command: Option<&'static str>,
}

impl Platform {
    fn defaults(self) -> Defaults {
        /// Cisco-style: `name>`, `name#`, `name(config)#`.
        const CISCO_PROMPT: &str = r"[\w.\-]+(\(\S+\))?[>#]";
        const NONE: Defaults = Defaults {
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
            definition: None,
            privilege_check: None,
            config_end: None,
            paging_command: None,
        };
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
                definition: Some("srlinux"),
                privilege_check: None,
                config_end: Some(r"^set / "),
                paging_command: Some("info flat"),
            },
            Self::Ceos => Defaults {
                username: Some("admin"),
                // `ceos1>` or `ceos1#`.
                prompt: Some(CISCO_PROMPT),
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
                definition: Some("eos"),
                privilege_check: Some(("show privilege", r"(?i)privilege level is 15")),
                config_end: Some(r"^end$"),
                paging_command: Some("show running-config all"),
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
                // Root gets a shell, not vtysh: see `Vtysh`.
                ..NONE
            },
            Self::Vtysh => Defaults {
                username: Some("vtysh"),
                // `frr1#` (vtysh has no unprivileged mode).
                prompt: Some(r"[\w.\-]+(\(\S+\))?#"),
                setup: &["terminal length 0"],
                command: Some("show version"),
                expect: Some(r"FRRouting"),
                exit: Some("exit"),
                definition: Some("frr"),
                config_end: Some(r"^end$"),
                paging_command: Some("show memory"),
                ..NONE
            },
            Self::Ios => Defaults {
                username: Some("admin"),
                prompt: Some(CISCO_PROMPT),
                setup: &["terminal length 0"],
                command: Some("show version"),
                expect: Some(r"(?i)cisco ios"),
                large_command: Some("show running-config all"),
                large_min_bytes: 32 * 1024,
                exit: Some("exit"),
                definition: Some("ios"),
                privilege_check: Some(("show privilege", r"(?i)privilege level is 15")),
                config_end: Some(r"^end$"),
                paging_command: Some("show running-config all"),
                ..NONE
            },
            Self::Nxos => Defaults {
                username: Some("admin"),
                prompt: Some(r"[\w.\-]+(\(\S+\))?#"),
                setup: &["terminal length 0"],
                command: Some("show version"),
                expect: Some(r"(?i)nx-os"),
                large_command: Some("show running-config all"),
                large_min_bytes: 32 * 1024,
                exit: Some("exit"),
                definition: Some("nxos"),
                paging_command: Some("show running-config all"),
                ..NONE
            },
            Self::Junos => Defaults {
                username: Some("admin"),
                // `admin@cjunos1>`.
                prompt: Some(r"[\w.\-]+@[\w.\-]+[>#]"),
                setup: &["set cli screen-length 0"],
                command: Some("show version"),
                expect: Some(r"(?i)junos"),
                exit: Some("exit"),
                definition: Some("junos"),
                config_end: Some(r"^set "),
                paging_command: Some("show system processes extensive"),
                ..NONE
            },
            Self::Other => NONE,
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
    /// The built-in platform definition jobs drive the device with (`""`:
    /// none, so the platform checks are not made).
    definition: Option<String>,
    /// The profile's enable secret, for a device whose privilege command
    /// asks for one.
    enable_secret: Option<String>,
    enable_secret_env: Option<String>,
    privilege_check: Option<String>,
    privilege_expect: Option<String>,
    config_end: Option<String>,
    paging_command: Option<String>,
    /// The checks made on this entry, by name (default: all of them). For
    /// a second login to a device that is there for a few checks only.
    checks: Option<Vec<String>>,
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
    /// The platform definition jobs drive it with (the host file's
    /// `platform`); `None`: the platform checks are not made.
    pub definition: Option<String>,
    /// The enable secret of its profile.
    pub enable_secret: Option<String>,
    /// A command showing the privilege level, and what its output must
    /// match after the job's privilege command.
    pub privilege_check: Option<(String, Regex)>,
    /// What the last line of the fetched configuration must match.
    pub config_end: Option<Regex>,
    /// A command printing more lines than a job's terminal has rows.
    pub paging_command: Option<String>,
    /// The only checks made on this entry; `None`: all of them.
    pub only: Option<Vec<Check>>,
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
            let definition = device
                .definition
                .or_else(|| defaults.definition.map(str::to_owned))
                .filter(|definition| !definition.trim().is_empty());
            let enable_secret = match (device.enable_secret, device.enable_secret_env) {
                (None, None) => None,
                (value, named) => Some(secret(
                    value,
                    named,
                    "",
                    &format!("device {name} has no enable secret"),
                )?),
            };
            let privilege_check = match (
                device
                    .privilege_check
                    .or_else(|| defaults.privilege_check.map(|(c, _)| c.to_owned())),
                device
                    .privilege_expect
                    .or_else(|| defaults.privilege_check.map(|(_, e)| e.to_owned())),
            ) {
                (Some(command), Some(expect)) => Some((
                    command,
                    Regex::new(&expect).map_err(|error| {
                        format!("device {name}: bad `privilege_expect` pattern: {error}")
                    })?,
                )),
                (None, None) => None,
                _ => {
                    return Err(format!(
                        "device {name}: set both privilege_check and privilege_expect, or neither"
                    ))
                }
            };
            let config_end = device
                .config_end
                .or_else(|| defaults.config_end.map(str::to_owned))
                .map(|pattern| {
                    Regex::new(&pattern).map_err(|error| {
                        format!("device {name}: bad `config_end` pattern: {error}")
                    })
                })
                .transpose()?;
            let paging_command = device
                .paging_command
                .or_else(|| defaults.paging_command.map(str::to_owned));
            let only = match device.checks {
                None => None,
                Some(names) => Some(
                    names
                        .iter()
                        .map(|wanted| {
                            ALL_CHECKS
                                .iter()
                                .copied()
                                .find(|check| check.name() == wanted)
                                .ok_or_else(|| {
                                    format!(
                                        "device {name}: `checks` names {wanted:?}; the checks are: {}",
                                        ALL_CHECKS
                                            .iter()
                                            .map(|check| check.name())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )
                                })
                        })
                        .collect::<Result<Vec<Check>, String>>()?,
                ),
            };
            devices.push(Device {
                only,
                paging_command,
                definition,
                enable_secret,
                privilege_check,
                config_end,
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
        let mut snippets = String::new();
        for device in &lab.devices {
            let mut host = format!(
                "address: {}\nport: {}\ncredential_profile: {}\n",
                yaml_text(&device.address),
                device.port,
                device.name
            );
            if let Some(definition) = &device.definition {
                // Only sessions opened with a login backup back up.
                host.push_str(&format!(
                    "platform: {}\nbackup_on_login: true\n",
                    yaml_text(definition)
                ));
            }
            write(&format!("{}.yaml", device.name), host)?;
            if let Some((command, _)) = &device.privilege_check {
                snippets.push_str(&format!(
                    "{}: {}\n",
                    privilege_snippet(device),
                    yaml_text(command)
                ));
            }
            if let Some(command) = &device.paging_command {
                snippets.push_str(&format!(
                    "{}: {}\n",
                    paging_snippet(device),
                    yaml_text(command)
                ));
            }
        }
        if !snippets.is_empty() {
            write("_snippets.yaml", snippets)?;
        }
        let mut workspace = Workspace::open(&root).map_err(|error| error.to_string())?;
        // Config backups keep what they fetch under the workspace id.
        workspace
            .ensure_id()
            .map_err(|error| format!("the suite's workspace id: {error}"))?;
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
                enable_secret: device.enable_secret.clone(),
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

    fn profiles(&self) -> Profiles<'_> {
        Profiles::new(&self.store, &self.workspace)
    }

    fn workspace_id(&self) -> &str {
        self.workspace
            .id()
            .expect("the suite's workspace has an id")
    }

    /// An empty config history of its own for `device` and `what`.
    fn config_store(&self, device: &Device, what: &str) -> Result<ConfigStore, String> {
        let root = self
            .temp
            .path()
            .join("configs")
            .join(what)
            .join(&device.name);
        if root.exists() {
            fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        }
        Ok(ConfigStore::new(root))
    }

    /// A manager that already trusts the device's host keys (and the jump
    /// host's), as jobs refuse unknown ones: one session was opened first.
    async fn trusting_manager(&self, device: &Device) -> Result<SessionManager, String> {
        let manager = self.manager();
        let mut session =
            Conversation::open_ssh(&manager, self.ssh_target(device)?, None, self.timeout())
                .await?;
        session.close().await?;
        Ok(manager)
    }

    /// Runs a job of `step` on `device` alone; its result.
    async fn run_job(
        &self,
        manager: &SessionManager,
        device: &Device,
        step: JobStep,
        configs: Option<ConfigStore>,
    ) -> Result<HostResult, String> {
        let result = self
            .job_result(manager, device, step, configs, &self.profiles())
            .await?;
        match &result.state {
            HostState::Done => Ok(result),
            other => Err(format!(
                "the job ended {other:?}; its output ended: {:?}",
                tail(&result.output, 600)
            )),
        }
    }

    /// Runs a job of `step` on `device` alone, with the logins of
    /// `profiles`; how the host ended, whatever its state.
    async fn job_result(
        &self,
        manager: &SessionManager,
        device: &Device,
        step: JobStep,
        configs: Option<ConfigStore>,
        profiles: &Profiles<'_>,
    ) -> Result<HostResult, String> {
        let plan = job::plan(
            &self.workspace,
            profiles,
            &JobTarget::Hosts {
                ids: vec![device.host_id()],
            },
            &step,
        )
        .map_err(|error| format!("the job could not be planned: {error}"))?;
        if let Some(skipped) = plan.skipped.first() {
            return Err(format!("the job skipped the device: {skipped:?}"));
        }
        let options = JobOptions {
            parallel: 1,
            command_timeout: self.timeout(),
            configs,
            script: None,
            ..JobOptions::default()
        };
        let mut job = job::start(manager, plan, options)
            .map_err(|error| format!("the job did not start: {error}"))?;
        // Connecting, privilege, paging and the step: a few answers each.
        let report = tokio::time::timeout(self.timeout() * 6, async {
            loop {
                match job.next().await {
                    Some(JobEvent::Finished(report)) => return Ok(report),
                    Some(_) => {}
                    None => return Err("the job ended without a report".to_owned()),
                }
            }
        })
        .await
        .map_err(|_| "the job did not finish".to_owned())??;
        report
            .hosts
            .into_iter()
            .next()
            .ok_or_else(|| "the job ran no host".to_owned())
    }

    /// The suite's logins, with `secret` as `device`'s enable secret.
    fn store_with_enable_secret(
        &self,
        device: &Device,
        secret: &str,
    ) -> Result<MemoryStore, String> {
        let store = MemoryStore::new();
        let scope = self.workspace.credential_scope();
        for profile in self.store.list(&scope).map_err(|error| error.0)? {
            let profile = if profile.name == device.name {
                CredentialProfile {
                    enable_secret: Some(secret.into()),
                    ..profile
                }
            } else {
                profile
            };
            store.put(&scope, &profile).map_err(|error| error.0)?;
        }
        Ok(store)
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

/// The name of the snippet that runs `device`'s paging command.
fn paging_snippet(device: &Device) -> String {
    format!("lab-paging-{}", device.name)
}

/// The name of the snippet that shows `device`'s privilege level.
fn privilege_snippet(device: &Device) -> String {
    format!("lab-privilege-{}", device.name)
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
    /// A job with the device's built-in platform definition turns paging
    /// off: a command printing more lines than the job's terminal has rows
    /// shows no pager and ends at the prompt.
    Paging,
    /// A config backup job fetches the whole configuration (and only it);
    /// a second backup of the unchanged device keeps nothing new (its
    /// ignored lines cover what changes by itself).
    ConfigBackup,
    /// A job with the definition's privilege command is in privileged mode.
    Privilege,
    /// Whether the device accepts a second shell channel on the session's
    /// connection, and the backup on login works either way.
    SecondChannel,
    /// Files, as the lab's documentation says the device offers them:
    /// SFTP (list, upload, download and compare, rename, delete) or, on a
    /// device without it, SCP by the fall back (send, fetch and compare).
    Files,
}

pub const ALL_CHECKS: &[Check] = &[
    Check::LoginAndCommand,
    Check::HostKeys,
    Check::WrongPassword,
    Check::Resize,
    Check::LargeOutput,
    Check::SessionLog,
    Check::Telnet,
    Check::Paging,
    Check::ConfigBackup,
    Check::Privilege,
    Check::SecondChannel,
    Check::Files,
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
            Check::Paging => "paging off",
            Check::ConfigBackup => "config backup",
            Check::Privilege => "privileged mode",
            Check::SecondChannel => "second channel and backup on login",
            Check::Files => "files",
        }
    }

    pub async fn run(self, run: &LabRun, device: &Device) -> Outcome {
        if device
            .only
            .as_ref()
            .is_some_and(|only| !only.contains(&self))
        {
            return Outcome::NotApplicable("left out by the entry's `checks`".into());
        }
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
            Check::Files => files::check(run, device).await,
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
            Check::Paging | Check::ConfigBackup | Check::Privilege | Check::SecondChannel => {
                let Some(definition) = &device.definition else {
                    return Outcome::NotApplicable(
                        "no built-in platform definition for this device".into(),
                    );
                };
                let Some(found) = run.workspace.host_platform(&device.host_id()) else {
                    return Outcome::Failed(format!(
                        "the platform {definition:?} names no usable definition"
                    ));
                };
                match self {
                    Check::Paging => match &device.paging_command {
                        None => Outcome::NotApplicable("no paging_command for this device".into()),
                        Some(_) => paging(run, device).await.into(),
                    },
                    Check::ConfigBackup => config_backup(run, device).await.into(),
                    Check::Privilege => match (&found.privilege_command, &device.privilege_check) {
                        (None, _) => Outcome::NotApplicable(format!(
                            "the {} definition has no privilege command",
                            found.name
                        )),
                        (Some(_), None) => {
                            Outcome::NotApplicable("no privilege_check for this device".into())
                        }
                        (Some(_), Some((_, expect))) => {
                            privilege(run, device, expect, found.privilege_prompt.as_deref())
                                .await
                                .into()
                        }
                    },
                    _ => second_channel(run, device).await.into(),
                }
            }
        }
    }
}

/// What a pager shows (IOS/NX-OS/EOS `--More--`, Junos `---(more)---`,
/// `less`'s `(END)` and `lines 1-40`).
const PAGER: &str = r"(?im)--\s*more\s*--|---\(more|\(END\)|^lines \d+-\d+";

async fn paging(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.trusting_manager(device).await?;
    let result = run
        .run_job(
            &manager,
            device,
            JobStep::Snippet {
                name: paging_snippet(device),
                answers: Default::default(),
            },
            None,
        )
        .await?;
    let pager = Regex::new(PAGER).expect("the pager pattern");
    if let Some(found) = pager.find(&result.output) {
        return Err(format!(
            "the output shows a pager ({:?}): paging is not off",
            found.as_str()
        ));
    }
    let lines = result.output.lines().count();
    let rows = JOB_TERMINAL_SIZE.rows as usize;
    if lines <= rows {
        return Err(format!(
            "the paging command printed {lines} lines, no more than the job's {rows} rows: \
             set a paging_command with more output"
        ));
    }
    Ok(format!("{lines} lines"))
}

async fn config_backup(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.trusting_manager(device).await?;
    let configs = run.config_store(device, "job")?;
    let id = device.host_id();
    let first = run
        .run_job(
            &manager,
            device,
            JobStep::ConfigBackup,
            Some(configs.clone()),
        )
        .await?;
    let version = match &first.backup {
        Some(SaveOutcome::Saved { version }) => version.clone(),
        other => return Err(format!("the first backup did not save: {other:?}")),
    };
    let kept = configs
        .read(run.workspace_id(), &id, &version)
        .map_err(|error| format!("the kept configuration: {error}"))?;
    let lines = kept.lines().count();
    let last = kept
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if let Some(end) = &device.config_end {
        if !end.is_match(last) {
            return Err(format!(
                "the configuration ({lines} lines) does not end as expected ({:?}); it ends: {:?}",
                end.as_str(),
                tail(&kept, 400)
            ));
        }
    }

    let second = run
        .run_job(
            &manager,
            device,
            JobStep::ConfigBackup,
            Some(configs.clone()),
        )
        .await?;
    match &second.backup {
        Some(SaveOutcome::Unchanged { version: same }) if *same == version => {}
        Some(SaveOutcome::Saved { version: new }) => {
            let changed: Vec<String> = configs
                .diff(run.workspace_id(), &id, &version, new)
                .map_err(|error| format!("diff: {error}"))?
                .into_iter()
                .filter_map(|line| match line {
                    DiffLine::Same { .. } => None,
                    DiffLine::Removed { text, .. } => Some(format!("-{text}")),
                    DiffLine::Added { text, .. } => Some(format!("+{text}")),
                })
                .collect();
            return Err(format!(
                "a second backup of the unchanged device saved a new version (ignored_lines \
                 should cover these lines): {changed:?}"
            ));
        }
        other => return Err(format!("the second backup: {other:?}")),
    }
    Ok(format!(
        "{lines} lines, last {last:?}; unchanged on the second backup"
    ))
}

/// An enable secret no device of the lab has.
const WRONG_ENABLE_SECRET: &str = "netterm-lab-not-the-enable-secret";

/// `question` is the definition's privilege password pattern. An entry
/// with an enable secret is there to show the question answered: the
/// device must have asked it, and a job with a wrong secret must fail the
/// host with the device's refusal (not hang, not carry on unprivileged).
async fn privilege(
    run: &LabRun,
    device: &Device,
    expect: &Regex,
    question: Option<&str>,
) -> Result<String, String> {
    let manager = run.trusting_manager(device).await?;
    let step = || JobStep::Snippet {
        name: privilege_snippet(device),
        answers: Default::default(),
    };
    let result = run.run_job(&manager, device, step(), None).await?;
    let Some(found) = expect.find(&result.output) else {
        return Err(format!(
            "the job's output does not match {:?}: {:?}",
            expect.as_str(),
            tail(&result.output, 600)
        ));
    };
    let level = found.as_str().to_owned();
    if device.enable_secret.is_none() {
        return Ok(level);
    }
    let question = question.ok_or("the definition has no privilege password pattern")?;
    let asked = Regex::new(&format!(r"(?m)^[ \t]*(?:{question})[ \t]*$"))
        .map_err(|error| format!("the definition's privilege password pattern: {error}"))?;
    if !asked.is_match(&result.output) {
        return Err(format!(
            "the entry has an enable secret, but the privilege command asked for no password \
             (is the login privileged already?): {:?}",
            tail(&result.output, 600)
        ));
    }
    let wrong = run.store_with_enable_secret(device, WRONG_ENABLE_SECRET)?;
    let refused = run
        .job_result(
            &manager,
            device,
            step(),
            None,
            &Profiles::new(&wrong, &run.workspace),
        )
        .await?;
    match &refused.state {
        HostState::Failed { reason } if reason.contains("refused the enable secret") => Ok(
            format!("{level}; the device asked for the enable secret; a wrong one was refused"),
        ),
        other => Err(format!(
            "with a wrong enable secret the job ended {other:?}, not refused; its output ended: {:?}",
            tail(&refused.output, 600)
        )),
    }
}

async fn second_channel(run: &LabRun, device: &Device) -> Result<String, String> {
    let manager = run.manager();
    let profiles = run.profiles();
    let id = device.host_id();
    let configs = run.config_store(device, "login")?;
    let backup = LoginBackup::plan(
        &run.workspace,
        &profiles,
        &id,
        SessionLogin::Profile,
        configs,
    )
    .ok_or("no backup on login applies to the device")?
    .with_command_timeout(run.timeout());
    let options = OpenOptions {
        log: None,
        when_connected: Some(Arc::new(backup)),
    };
    let target = run.ssh_target(device)?;
    let password = match &target.auth {
        SshAuth::Password(password) => password.clone(),
        _ => String::new(),
    };
    let mut session = Conversation::connect_with(
        &manager,
        SessionTarget::Ssh(target),
        options,
        run.timeout(),
        &password,
    )
    .await
    .map_err(|failure| format!("did not connect: {failure}"))?;
    session.ready(device).await?;
    // The backup runs beside the terminal, over a second channel, or a
    // second connection where the device refuses one.
    let saved = match session.backup(run.timeout() * 6).await? {
        LoginBackupOutcome::Saved { .. } => "saved",
        LoginBackupOutcome::Unchanged { .. } => "unchanged",
        LoginBackupOutcome::Failed { reason } => {
            return Err(format!("the backup on login failed: {reason}"))
        }
    };

    // Then a channel of our own on the same connection.
    let channel = match manager.open_channel(session.id, SIZE) {
        Err(error) => return Err(format!("open_channel: {error:?}")),
        Ok((channel, events)) => {
            Conversation::attach(&manager, channel, events, run.timeout())
                .connected(&password)
                .await
        }
    };
    let accepted = match channel {
        Ok(mut channel) => {
            channel.ready(device).await?;
            let output = channel.run(&device.command, &device.prompt).await?;
            if !device.expect.is_match(&output) {
                return Err(format!(
                    "on the second channel, the output of {:?} does not match {:?}: {output:?}",
                    device.command,
                    device.expect.as_str()
                ));
            }
            channel.close().await?;
            "accepted".to_owned()
        }
        Err(Failure::Closed(CloseReason::Failed(OpenError::ChannelRefused { reason }))) => {
            format!("REFUSED ({reason}), so the backup used a second connection")
        }
        Err(other) => return Err(format!("the second channel: {other}")),
    };
    // The terminal's own channel is untouched.
    let output = session.run(&device.command, &device.prompt).await?;
    if !device.expect.is_match(&output) {
        return Err(format!(
            "after the second channel, the session's output of {:?} does not match: {output:?}",
            device.command
        ));
    }
    session.close().await?;
    Ok(format!(
        "second channel {accepted}; backup on login {saved}"
    ))
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
    /// Backups on login reported and not yet taken by [`Self::backup`].
    backups: Vec<LoginBackupOutcome>,
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
        let options = OpenOptions {
            log,
            when_connected: None,
        };
        Self::connect_with(manager, target, options, timeout, password).await
    }

    /// [`Self::connect`], opening the session with `options`.
    pub async fn connect_with(
        manager: &'a SessionManager,
        target: SessionTarget,
        options: OpenOptions,
        timeout: Duration,
        password: &str,
    ) -> Result<Conversation<'a>, Failure> {
        let (id, events) = manager.open_with(target, SIZE, options);
        Self::attach(manager, id, events, timeout)
            .connected(password)
            .await
    }

    /// A conversation with the session `id` (not yet connected).
    pub fn attach(
        manager: &'a SessionManager,
        id: SessionId,
        events: SessionEvents,
        timeout: Duration,
    ) -> Conversation<'a> {
        Conversation {
            manager,
            id,
            events,
            timeout,
            received: Vec::new(),
            asked: Vec::new(),
            algorithms: Vec::new(),
            backups: Vec::new(),
        }
    }

    /// Waits until the session is connected (see [`Self::connect`]).
    pub async fn connected(mut self, password: &str) -> Result<Conversation<'a>, Failure> {
        let manager = self.manager;
        let id = self.id;
        let session = &mut self;
        let mut rounds = 0;
        loop {
            match session.next_event().await.map_err(Failure::Other)? {
                SessionEvent::State(SessionState::Connecting) => {}
                SessionEvent::State(SessionState::Connected) => break,
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
                SessionEvent::Backup(_) => unreachable!("next_event keeps backups"),
                SessionEvent::Output(bytes) => session.received.extend_from_slice(&bytes),
                SessionEvent::LogFailed(failure) => {
                    return Err(Failure::Other(format!("log failed: {failure}")))
                }
            }
        }
        Ok(self)
    }

    /// The next backup on login the session reports, waiting at most
    /// `wait` for it (output meanwhile is kept for the next wait).
    pub async fn backup(&mut self, wait: Duration) -> Result<LoginBackupOutcome, String> {
        if !self.backups.is_empty() {
            return Ok(self.backups.remove(0));
        }
        let until = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(until, self.events.next()).await {
                Err(_) => {
                    return Err(format!(
                        "no backup on login reported within {} s",
                        wait.as_secs()
                    ))
                }
                Ok(None) => return Err("the session's events ended".into()),
                Ok(Some(SessionEvent::Backup(outcome))) => return Ok(outcome),
                Ok(Some(SessionEvent::Output(bytes))) => {
                    self.events.acknowledge(bytes.len());
                    self.received.extend_from_slice(&bytes);
                }
                Ok(Some(SessionEvent::State(SessionState::Closed(reason)))) => {
                    return Err(format!("the session closed ({reason}) before its backup"))
                }
                Ok(Some(other)) => {
                    return Err(format!("waiting for the backup: unexpected {other:?}"))
                }
            }
        }
    }

    /// The next event within `wait` (output is acknowledged at once), or
    /// `None` if there is none by then.
    async fn event_within(&mut self, wait: Duration) -> Result<Option<SessionEvent>, String> {
        let until = tokio::time::Instant::now() + wait;
        loop {
            match tokio::time::timeout_at(until, self.events.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err("the session's events ended".into()),
                Ok(Some(SessionEvent::Backup(outcome))) => self.backups.push(outcome),
                Ok(Some(event)) => {
                    if let SessionEvent::Output(bytes) = &event {
                        self.events.acknowledge(bytes.len());
                    }
                    return Ok(Some(event));
                }
            }
        }
    }

    /// The next event (output is acknowledged at once), or an error after
    /// the timeout.
    async fn next_event(&mut self) -> Result<SessionEvent, String> {
        match self.event_within(self.timeout).await? {
            None => Err(format!(
                "nothing happened for {} s; last output: {:?}",
                self.timeout.as_secs(),
                tail(&plain(&self.received), 400)
            )),
            Some(event) => Ok(event),
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
