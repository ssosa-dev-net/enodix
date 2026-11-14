//! Platform definitions: how the app drives a kind of device.
//!
//! A definition names a kind of device (with other names it answers to, such
//! as those of Nornir, Netmiko and NAPALM) and holds what a job needs to talk
//! to it: the prompt pattern (required), the commands that turn off paging,
//! enter privileged mode and show the configuration, the pattern of the
//! privilege password question, and the pattern of output lines to ignore
//! (such as a configuration's timestamp).
//!
//! The app ships [`BUILT_IN_PLATFORMS`]. A workspace adds its own and changes
//! the built-in ones in its platforms file (see `Workspace::platforms`). A
//! host's platform setting names a definition by its name or one of its
//! other names, ignoring case (`Workspace::platform`).
//!
//! Patterns are regular expressions in the syntax of the `regex` crate.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// How the app drives a kind of device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformDefinition {
    /// What the definition is called; unique in a workspace, ignoring case,
    /// among names and other names.
    pub name: String,
    /// Other names a host's platform may give for it.
    pub other_names: Vec<String>,
    /// A pattern matching the device's prompt.
    pub prompt: String,
    /// The command that turns off paging.
    pub disable_paging: Option<String>,
    /// The command that enters privileged mode.
    pub privilege_command: Option<String>,
    /// A pattern matching the question for the privilege password.
    pub privilege_prompt: Option<String>,
    /// The command that shows the configuration.
    pub show_config: Option<String>,
    /// A pattern matching output lines to ignore.
    pub ignored_lines: Option<String>,
}

impl PlatformDefinition {
    /// Whether `text` (trimmed, ignoring case) is this definition's name.
    pub fn is_named(&self, text: &str) -> bool {
        same(&self.name, text)
    }

    /// Whether `text` is one of this definition's other names.
    pub fn answers_to(&self, text: &str) -> bool {
        self.other_names.iter().any(|name| same(name, text))
    }

    /// Each of the definition's patterns that cannot be used, as
    /// "`key`: why"; `Ok` when all of them can.
    pub fn check(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        if self.prompt.trim().is_empty() {
            errors.push(format!("`{PROMPT}`: the pattern is empty"));
        } else if let Err(error) = check_pattern(&self.prompt) {
            errors.push(format!("`{PROMPT}`: {error}"));
        }
        for (key, pattern) in [
            (PRIVILEGE_PROMPT, &self.privilege_prompt),
            (IGNORED_LINES, &self.ignored_lines),
        ] {
            if let Some(pattern) = pattern {
                if let Err(error) = check_pattern(pattern) {
                    errors.push(format!("`{key}`: {error}"));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Keys of a definition in the platforms file (also used in messages).
pub(crate) const OTHER_NAMES: &str = "other_names";
pub(crate) const PROMPT: &str = "prompt";
pub(crate) const DISABLE_PAGING: &str = "disable_paging";
pub(crate) const PRIVILEGE_COMMAND: &str = "privilege_command";
pub(crate) const PRIVILEGE_PROMPT: &str = "privilege_prompt";
pub(crate) const SHOW_CONFIG: &str = "show_config";
pub(crate) const IGNORED_LINES: &str = "ignored_lines";

/// Where a definition of a workspace comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlatformSource {
    /// One of [`BUILT_IN_PLATFORMS`], as built in.
    BuiltIn,
    /// A built-in definition the workspace changes.
    Changed,
    /// The workspace's own.
    Workspace,
}

/// A definition as a workspace lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedPlatform {
    pub definition: PlatformDefinition,
    pub source: PlatformSource,
    /// Why one of its patterns cannot be used, if one cannot: such a
    /// definition is not used (no host's platform finds it).
    pub error: Option<String>,
}

/// A definition the app ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltInPlatform {
    pub name: &'static str,
    pub other_names: &'static [&'static str],
    pub prompt: &'static str,
    pub disable_paging: Option<&'static str>,
    pub privilege_command: Option<&'static str>,
    pub privilege_prompt: Option<&'static str>,
    pub show_config: Option<&'static str>,
    pub ignored_lines: Option<&'static str>,
}

impl BuiltInPlatform {
    pub fn definition(&self) -> PlatformDefinition {
        let text = |value: Option<&str>| value.map(str::to_string);
        PlatformDefinition {
            name: self.name.into(),
            other_names: self
                .other_names
                .iter()
                .map(|name| name.to_string())
                .collect(),
            prompt: self.prompt.into(),
            disable_paging: text(self.disable_paging),
            privilege_command: text(self.privilege_command),
            privilege_prompt: text(self.privilege_prompt),
            show_config: text(self.show_config),
            ignored_lines: text(self.ignored_lines),
        }
    }
}

/// A Cisco-style privilege password question.
const PASSWORD_PROMPT: &str = r"(?i)password:\s*$";

/// The definitions the app ships. The prompts of SR Linux and EOS are the
/// ones the containerlab suite uses; every value is still to be verified
/// on real devices by the suite's per-platform checks.
pub const BUILT_IN_PLATFORMS: [BuiltInPlatform; 6] = [
    BuiltInPlatform {
        name: "srlinux",
        other_names: &["nokia_srl", "nokia_srlinux", "srl"],
        // `A:srl1# ` (the line before it is `--{ running }--[  ]--`).
        prompt: r"[A-Z]:\S+#",
        // SR Linux does not page its CLI output.
        disable_paging: None,
        privilege_command: None,
        privilege_prompt: None,
        show_config: Some("info flat"),
        ignored_lines: Some(r"^--\{.*\}--"),
    },
    BuiltInPlatform {
        name: "eos",
        other_names: &["arista_eos", "ceos", "arista"],
        // `ceos1>` or `ceos1(config)#`.
        prompt: r"[\w.\-]+(\(\S+\))?[>#]",
        disable_paging: Some("terminal length 0"),
        privilege_command: Some("enable"),
        privilege_prompt: Some(PASSWORD_PROMPT),
        show_config: Some("show running-config"),
        ignored_lines: Some(r"^! (Command|Time|device):"),
    },
    BuiltInPlatform {
        name: "frr",
        other_names: &["frrouting", "vtysh"],
        // vtysh: `frr1#` or `frr1(config)#`.
        prompt: r"[\w.\-]+(\(\S+\))?#",
        disable_paging: Some("terminal length 0"),
        privilege_command: None,
        privilege_prompt: None,
        show_config: Some("show running-config"),
        ignored_lines: Some(r"^Building configuration"),
    },
    BuiltInPlatform {
        name: "ios",
        other_names: &[
            "cisco_ios",
            "cisco_xe",
            "cisco_iosxe",
            "iosxe",
            "ios-xe",
            "cisco_ios_ssh",
        ],
        prompt: r"[\w.\-@/:]+(\(\S+\))?[>#]",
        disable_paging: Some("terminal length 0"),
        privilege_command: Some("enable"),
        privilege_prompt: Some(PASSWORD_PROMPT),
        show_config: Some("show running-config"),
        ignored_lines: Some(
            r"^(Building configuration|Current configuration :|! Last configuration change|! NVRAM config last updated|ntp clock-period)",
        ),
    },
    BuiltInPlatform {
        name: "nxos",
        other_names: &["cisco_nxos", "nxos_ssh", "nx-os", "cisco_nxos_ssh"],
        prompt: r"[\w.\-]+(\(\S+\))?#",
        disable_paging: Some("terminal length 0"),
        privilege_command: None,
        privilege_prompt: None,
        show_config: Some("show running-config"),
        ignored_lines: Some(r"^!(Command|Running configuration last done at|Time):"),
    },
    BuiltInPlatform {
        name: "junos",
        other_names: &["juniper_junos", "juniper"],
        // `admin@vmx1>` or `admin@vmx1#`.
        prompt: r"[\w.\-]+@[\w.\-]+[>#]",
        disable_paging: Some("set cli screen-length 0"),
        privilege_command: None,
        privilege_prompt: None,
        show_config: Some("show configuration | display set"),
        ignored_lines: Some(r"^(## Last commit:|\{master)"),
    },
];

/// The built-in definitions, listed as built in.
pub fn built_in_platforms() -> Vec<ListedPlatform> {
    BUILT_IN_PLATFORMS
        .iter()
        .map(|platform| ListedPlatform {
            definition: platform.definition(),
            source: PlatformSource::BuiltIn,
            error: None,
        })
        .collect()
}

/// The built-in definition called `name` (ignoring case), if there is one.
pub fn built_in_platform(name: &str) -> Option<&'static BuiltInPlatform> {
    BUILT_IN_PLATFORMS
        .iter()
        .find(|platform| same(platform.name, name))
}

/// The definition `text` names among `listed`: by name first, then by
/// other name (the first that has it), ignoring case. It may be one that
/// cannot be used (see [`find`]).
pub fn find_listed<'a>(listed: &'a [ListedPlatform], text: &str) -> Option<&'a ListedPlatform> {
    listed
        .iter()
        .find(|listed| listed.definition.is_named(text))
        .or_else(|| {
            listed
                .iter()
                .find(|listed| listed.definition.answers_to(text))
        })
}

/// The usable definition `text` names among `listed` ([`find_listed`]);
/// `None` when none does or the one it names cannot be used.
pub fn find<'a>(listed: &'a [ListedPlatform], text: &str) -> Option<&'a PlatformDefinition> {
    let found = find_listed(listed, text)?;
    found.error.is_none().then_some(&found.definition)
}

/// The definition a platform name finds (see `Workspace::platform_names`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformName {
    /// The definition's name.
    pub definition: String,
    /// False when the definition cannot be used (a bad pattern).
    pub usable: bool,
}

/// Whether `pattern` can be used.
fn check_pattern(pattern: &str) -> Result<(), String> {
    Regex::new(pattern)
        .map(|_| ())
        .map_err(|error| format!("the pattern {pattern:?} cannot be used: {error}"))
}

/// Names are the same ignoring case and surrounding blanks.
pub(crate) fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}
