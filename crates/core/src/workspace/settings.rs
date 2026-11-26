//! Inherited connection settings.
//!
//! A folder's settings file and a host's own file may both set connection
//! settings ([`Settings`]). What applies to a host or folder
//! ([`EffectiveSettings`]) follows these rules, walking from the workspace
//! root down to it:
//!
//! - a single-value setting comes from the nearest definition: the host,
//!   then its folder, then each parent up to the root, else a default;
//! - web links merge by name, a nearer link replacing one of the same name;
//! - tags add up.
//!
//! Every effective value carries its [`Origin`], so the interface can show
//! where it came from. Later features give each setting its behaviour; the
//! port is already used when connecting.

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use super::{detail_text, normalize, optional_text, EditError};
use crate::paste::MAX_PASTE_LINE_DELAY_MS;

/// File keys of the settings.
pub(super) const PROTOCOL: &str = "protocol";
pub(super) const PORT: &str = "port";
pub(super) const CREDENTIAL_PROFILE: &str = "credential_profile";
pub(super) const JUMP_HOST: &str = "jump_host";
pub(super) const LEGACY_CRYPTO: &str = "legacy_crypto";
pub(super) const CONNECT_TIMEOUT: &str = "connect_timeout";
pub(super) const LOGGING: &str = "logging";
pub(super) const LOG_TIMESTAMPS: &str = "log_timestamps";
pub(super) const BACKUP_ON_LOGIN: &str = "backup_on_login";
pub(super) const ENVIRONMENT: &str = "environment";
pub(super) const PASTE_LINE_DELAY: &str = "paste_line_delay";
pub(super) const TERMINAL_TYPE: &str = "terminal_type";
pub(super) const PLATFORM: &str = "platform";
pub(super) const SERIAL: &str = "serial";
pub(super) const WEB_LINKS: &str = "web_links";

/// Serial parameter keys, inside `serial`.
const SERIAL_PORT: &str = "port";
const BAUD_RATE: &str = "baud_rate";
const DATA_BITS: &str = "data_bits";
const PARITY: &str = "parity";
const STOP_BITS: &str = "stop_bits";
const FLOW_CONTROL: &str = "flow_control";

/// Serial parameters when nothing sets them: 9600 8N1, no flow control.
pub const DEFAULT_BAUD_RATE: u32 = 9600;
pub const DEFAULT_DATA_BITS: u8 = 8;
pub const DEFAULT_STOP_BITS: u8 = 1;

/// The connect timeout, in seconds, when nothing sets it (the session's
/// default), and the longest one allowed.
pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u32 =
    crate::session::DEFAULT_CONNECT_TIMEOUT.as_secs() as u32;
pub const MAX_CONNECT_TIMEOUT_SECONDS: u32 = 3600;

/// The settings one host file or folder settings file sets itself. `None`
/// (or an empty list) means it does not set it, so it inherits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The protocol a plain "connect" uses.
    pub protocol: Option<Protocol>,
    /// Without one anywhere, the protocol's own default port applies.
    pub port: Option<u16>,
    /// The name of a credential profile; never a secret.
    pub credential_profile: Option<String>,
    /// The id of another host of the workspace to connect through.
    pub jump_host: Option<String>,
    pub legacy_crypto: Option<bool>,
    /// How long, in seconds, opening a session may wait on the device (1
    /// to [`MAX_CONNECT_TIMEOUT_SECONDS`]).
    pub connect_timeout: Option<u32>,
    pub logging: Option<bool>,
    /// Whether session logs write the time before each line.
    pub log_timestamps: Option<bool>,
    /// Whether every SSH session to the host saves its configuration in
    /// the config history once connected (see [`crate::job::LoginBackup`]).
    pub backup_on_login: Option<bool>,
    pub environment: Option<Environment>,
    /// Milliseconds to wait between pasted lines (0 to
    /// [`MAX_PASTE_LINE_DELAY_MS`]), for slow devices.
    pub paste_line_delay: Option<u32>,
    /// The terminal type sessions to the host announce (`TERM`).
    pub terminal_type: Option<TerminalType>,
    /// The kind of device (`eos`, `srlinux`, ...), as written; any text.
    pub platform: Option<String>,
    /// Links set here, in file order; names are unique.
    pub web_links: Vec<WebLink>,
    pub serial: SerialSettings,
}

/// Serial line parameters set at one level; each one inherits on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialSettings {
    /// The serial port to open, as the system names it (`COM3`,
    /// `/dev/ttyUSB0`); never blank.
    pub port: Option<String>,
    pub baud_rate: Option<u32>,
    /// 5 to 8.
    pub data_bits: Option<u8>,
    pub parity: Option<Parity>,
    /// 1 or 2.
    pub stop_bits: Option<u8>,
    pub flow_control: Option<FlowControl>,
}

/// A named link to a web page about the host, such as its GUI. The URL is a
/// template, filled as snippets are (see [`crate::template`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebLink {
    pub name: String,
    pub url: String,
}

impl WebLink {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
        }
    }

    /// The URL with its variables filled from `values` (a host's: see
    /// [`super::ResolvedHost::variables`]). Values are inserted as they are.
    pub fn fill(
        &self,
        values: &crate::template::Variables,
    ) -> Result<String, crate::template::MissingVariables> {
        crate::template::Template::parse(&self.url).fill(values)
    }
}

/// Defines a setting's choice of values, named in files as they are here
/// (in lower case).
macro_rules! choice {
    ($(#[$doc:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            const ALL: &[(Self, &'static str)] = &[$((Self::$variant, $text)),+];

            /// The value as written in files.
            pub fn as_str(self) -> &'static str {
                Self::ALL
                    .iter()
                    .find(|(value, _)| *value == self)
                    .map_or("", |(_, text)| text)
            }

            fn parse(text: &str) -> Option<Self> {
                Self::ALL
                    .iter()
                    .find(|(_, known)| *known == text)
                    .map(|(value, _)| *value)
            }

            fn expected() -> String {
                let names: Vec<&str> = Self::ALL.iter().map(|(_, text)| *text).collect();
                format!("one of {}", names.join(", "))
            }
        }
    };
}

choice!(
    /// How to reach a host.
    Protocol {
        Ssh = "ssh",
        Telnet = "telnet",
        Serial = "serial",
    }
);

choice!(
    /// What kind of network a host belongs to.
    Environment {
        Production = "production",
        Staging = "staging",
        Lab = "lab",
    }
);

choice!(
    /// The terminal type a session announces to the device (SSH's pty
    /// request, telnet's terminal type). Some equipment draws its menus
    /// wrongly with the default.
    TerminalType {
        Xterm256Color = "xterm-256color",
        Xterm = "xterm",
        Vt100 = "vt100",
        Ansi = "ansi",
        Linux = "linux",
    }
);

// The `choice!` macro defines the enum, so the default is written out.
#[allow(clippy::derivable_impls)]
impl Default for TerminalType {
    fn default() -> Self {
        Self::Xterm256Color
    }
}

choice!(Parity {
    None = "none",
    Odd = "odd",
    Even = "even",
    Mark = "mark",
    Space = "space",
});

choice!(FlowControl {
    None = "none",
    Hardware = "hardware",
    Software = "software",
});

/// Where an effective setting comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Origin {
    /// Nothing sets it; the built-in default applies.
    Default,
    /// The host's own file.
    Host,
    /// The settings file of the folder at `path` (`""` is the root).
    Folder { path: String },
}

/// A setting's value and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Effective<T> {
    pub value: T,
    pub origin: Origin,
}

/// What applies to a host or folder once inheritance is resolved. Settings
/// without a default are `None` when nothing sets them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveSettings {
    pub protocol: Effective<Protocol>,
    /// `None`: the protocol's own default port.
    pub port: Option<Effective<u16>>,
    pub credential_profile: Option<Effective<String>>,
    pub jump_host: Option<Effective<String>>,
    pub legacy_crypto: Effective<bool>,
    /// In seconds.
    pub connect_timeout: Effective<u32>,
    pub logging: Effective<bool>,
    /// Off by default.
    pub log_timestamps: Effective<bool>,
    /// Off by default.
    pub backup_on_login: Effective<bool>,
    pub environment: Option<Effective<Environment>>,
    /// In milliseconds; 0 (no delay) by default.
    pub paste_line_delay: Effective<u32>,
    /// `xterm-256color` by default.
    pub terminal_type: Effective<TerminalType>,
    pub platform: Option<Effective<String>>,
    /// Merged by name: the order a name first appears, from the root down.
    pub web_links: Vec<Effective<WebLink>>,
    pub serial: EffectiveSerial,
    /// Every tag from the root down, once each, from its nearest origin.
    pub tags: Vec<Effective<String>>,
}

/// Effective serial line parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveSerial {
    /// `None` when nothing sets one: a session has to be told which.
    pub port: Option<Effective<String>>,
    pub baud_rate: Effective<u32>,
    pub data_bits: Effective<u8>,
    pub parity: Effective<Parity>,
    pub stop_bits: Effective<u8>,
    pub flow_control: Effective<FlowControl>,
}

/// One step of the walk from the root: who sets what there.
pub(super) struct Level<'a> {
    pub origin: Origin,
    pub settings: &'a Settings,
    pub tags: &'a [String],
}

/// Resolves the levels, ordered from the workspace root to the host or
/// folder itself.
pub(super) fn resolve(levels: &[Level<'_>]) -> EffectiveSettings {
    fn nearest<T>(
        levels: &[Level<'_>],
        get: impl Fn(&Settings) -> Option<T>,
    ) -> Option<Effective<T>> {
        levels.iter().rev().find_map(|level| {
            get(level.settings).map(|value| Effective {
                value,
                origin: level.origin.clone(),
            })
        })
    }
    fn or_default<T>(found: Option<Effective<T>>, value: T) -> Effective<T> {
        found.unwrap_or(Effective {
            value,
            origin: Origin::Default,
        })
    }

    let mut web_links: Vec<Effective<WebLink>> = Vec::new();
    let mut tags: Vec<Effective<String>> = Vec::new();
    for level in levels {
        for link in &level.settings.web_links {
            let effective = Effective {
                value: link.clone(),
                origin: level.origin.clone(),
            };
            match web_links.iter_mut().find(|l| l.value.name == link.name) {
                Some(slot) => *slot = effective,
                None => web_links.push(effective),
            }
        }
        for tag in level.tags {
            match tags.iter_mut().find(|t| t.value == *tag) {
                Some(slot) => slot.origin = level.origin.clone(),
                None => tags.push(Effective {
                    value: tag.clone(),
                    origin: level.origin.clone(),
                }),
            }
        }
    }

    EffectiveSettings {
        protocol: or_default(nearest(levels, |s| s.protocol), Protocol::Ssh),
        port: nearest(levels, |s| s.port),
        credential_profile: nearest(levels, |s| s.credential_profile.clone()),
        jump_host: nearest(levels, |s| s.jump_host.clone()),
        legacy_crypto: or_default(nearest(levels, |s| s.legacy_crypto), false),
        connect_timeout: or_default(
            nearest(levels, |s| s.connect_timeout),
            DEFAULT_CONNECT_TIMEOUT_SECONDS,
        ),
        logging: or_default(nearest(levels, |s| s.logging), true),
        log_timestamps: or_default(nearest(levels, |s| s.log_timestamps), false),
        backup_on_login: or_default(nearest(levels, |s| s.backup_on_login), false),
        environment: nearest(levels, |s| s.environment),
        paste_line_delay: or_default(nearest(levels, |s| s.paste_line_delay), 0),
        terminal_type: or_default(
            nearest(levels, |s| s.terminal_type),
            TerminalType::default(),
        ),
        platform: nearest(levels, |s| s.platform.clone()),
        web_links,
        serial: EffectiveSerial {
            port: nearest(levels, |s| s.serial.port.clone()),
            baud_rate: or_default(nearest(levels, |s| s.serial.baud_rate), DEFAULT_BAUD_RATE),
            data_bits: or_default(nearest(levels, |s| s.serial.data_bits), DEFAULT_DATA_BITS),
            parity: or_default(nearest(levels, |s| s.serial.parity), Parity::None),
            stop_bits: or_default(nearest(levels, |s| s.serial.stop_bits), DEFAULT_STOP_BITS),
            flow_control: or_default(
                nearest(levels, |s| s.serial.flow_control),
                FlowControl::None,
            ),
        },
        tags,
    }
}

// Reading.

/// The settings a host file or folder settings file sets.
pub(super) fn settings_from_document(document: &Mapping) -> Result<Settings, String> {
    let serial = match document.get(SERIAL) {
        None | Some(Value::Null) => SerialSettings::default(),
        Some(Value::Mapping(serial)) => serial_from_mapping(serial)?,
        Some(_) => return Err(format!("`{SERIAL}` must map serial parameters to values")),
    };
    Ok(Settings {
        protocol: read_choice(document, PROTOCOL, Protocol::parse, Protocol::expected)?,
        port: read_port(document)?,
        credential_profile: optional_text(document, CREDENTIAL_PROFILE)?,
        jump_host: optional_text(document, JUMP_HOST)?,
        legacy_crypto: read_flag(document, LEGACY_CRYPTO)?,
        connect_timeout: read_connect_timeout(document)?,
        logging: read_flag(document, LOGGING)?,
        log_timestamps: read_flag(document, LOG_TIMESTAMPS)?,
        backup_on_login: read_flag(document, BACKUP_ON_LOGIN)?,
        environment: read_choice(
            document,
            ENVIRONMENT,
            Environment::parse,
            Environment::expected,
        )?,
        paste_line_delay: read_paste_line_delay(document)?,
        terminal_type: read_choice(
            document,
            TERMINAL_TYPE,
            TerminalType::parse,
            TerminalType::expected,
        )?,
        // Read as host files always did: a number is its text, blank unset.
        platform: detail_text(document, PLATFORM)?,
        web_links: read_web_links(document)?,
        serial,
    })
}

pub(super) fn read_port(document: &Mapping) -> Result<Option<u16>, String> {
    let error = || format!("`{PORT}` must be a number from 1 to 65535");
    match document.get(PORT) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port != 0)
            .map(Some)
            .ok_or_else(error),
        Some(_) => Err(error()),
    }
}

fn read_connect_timeout(document: &Mapping) -> Result<Option<u32>, String> {
    let error = || connect_timeout_error(&format!("`{CONNECT_TIMEOUT}`"));
    match document.get(CONNECT_TIMEOUT) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|seconds| (1..=u64::from(MAX_CONNECT_TIMEOUT_SECONDS)).contains(seconds))
            .map(|seconds| Some(seconds as u32))
            .ok_or_else(error),
        Some(_) => Err(error()),
    }
}

fn connect_timeout_error(what: &str) -> String {
    format!("{what} must be a number of seconds from 1 to {MAX_CONNECT_TIMEOUT_SECONDS}")
}

fn read_paste_line_delay(document: &Mapping) -> Result<Option<u32>, String> {
    let error = || paste_line_delay_error(&format!("`{PASTE_LINE_DELAY}`"));
    match document.get(PASTE_LINE_DELAY) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .filter(|ms| *ms <= u64::from(MAX_PASTE_LINE_DELAY_MS))
            .map(|ms| Some(ms as u32))
            .ok_or_else(error),
        Some(_) => Err(error()),
    }
}

fn paste_line_delay_error(what: &str) -> String {
    format!("{what} must be a number of milliseconds from 0 to {MAX_PASTE_LINE_DELAY_MS}")
}

fn serial_from_mapping(serial: &Mapping) -> Result<SerialSettings, String> {
    let key = |name: &str| format!("{SERIAL}.{name}");
    let number = |name: &str, low: u64, high: u64| -> Result<Option<u64>, String> {
        match serial.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number
                .as_u64()
                .filter(|n| (low..=high).contains(n))
                .map(Some)
                .ok_or(()),
            Some(_) => Err(()),
        }
        .map_err(|()| {
            if high == u64::from(u32::MAX) {
                format!("`{}` must be a positive whole number", key(name))
            } else if high - low == 1 {
                format!("`{}` must be {low} or {high}", key(name))
            } else {
                format!("`{}` must be a number from {low} to {high}", key(name))
            }
        })
    };
    Ok(SerialSettings {
        port: optional_text(serial, SERIAL_PORT).map_err(|_| {
            format!(
                "`{}` must be the name of a serial port, such as COM3",
                key(SERIAL_PORT)
            )
        })?,
        baud_rate: number(BAUD_RATE, 1, u64::from(u32::MAX))?.map(|n| n as u32),
        data_bits: number(DATA_BITS, 5, 8)?.map(|n| n as u8),
        parity: read_choice(serial, PARITY, Parity::parse, Parity::expected)
            .map_err(|_| format!("`{}` must be {}", key(PARITY), Parity::expected()))?,
        stop_bits: number(STOP_BITS, 1, 2)?.map(|n| n as u8),
        flow_control: read_choice(
            serial,
            FLOW_CONTROL,
            FlowControl::parse,
            FlowControl::expected,
        )
        .map_err(|_| {
            format!(
                "`{}` must be {}",
                key(FLOW_CONTROL),
                FlowControl::expected()
            )
        })?,
    })
}

fn read_choice<T>(
    document: &Mapping,
    key: &str,
    parse: fn(&str) -> Option<T>,
    expected: fn() -> String,
) -> Result<Option<T>, String> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => parse(text.trim())
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be {}", expected())),
        Some(_) => Err(format!("`{key}` must be {}", expected())),
    }
}

fn read_flag(document: &Mapping, key: &str) -> Result<Option<bool>, String> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

fn read_web_links(document: &Mapping) -> Result<Vec<WebLink>, String> {
    let error = || format!("`{WEB_LINKS}` must map link names to URLs");
    match document.get(WEB_LINKS) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Mapping(links)) => links
            .iter()
            .map(|(name, url)| match (name, url) {
                (Value::String(name), Value::String(url))
                    if !name.trim().is_empty() && !url.trim().is_empty() =>
                {
                    Ok(WebLink::new(name.trim(), url.trim()))
                }
                _ => Err(error()),
            })
            .collect(),
        Some(_) => Err(error()),
    }
}

// Writing.

/// `edit`'s settings, cleaned up (text trimmed, blank text unset) and
/// checked.
fn checked(edit: &Settings) -> Result<Settings, EditError> {
    let invalid = |message: &str| Err(EditError::Invalid(message.into()));
    if edit.port == Some(0) {
        return invalid("the port must be a number from 1 to 65535");
    }
    if edit
        .connect_timeout
        .is_some_and(|seconds| !(1..=MAX_CONNECT_TIMEOUT_SECONDS).contains(&seconds))
    {
        return Err(EditError::Invalid(connect_timeout_error(
            "the connect timeout",
        )));
    }
    if edit
        .paste_line_delay
        .is_some_and(|ms| ms > MAX_PASTE_LINE_DELAY_MS)
    {
        return Err(EditError::Invalid(paste_line_delay_error(
            "the paste line delay",
        )));
    }
    let serial = &edit.serial;
    if serial.baud_rate == Some(0) {
        return invalid("the baud rate must be a positive whole number");
    }
    if serial
        .data_bits
        .is_some_and(|bits| !(5..=8).contains(&bits))
    {
        return invalid("data bits must be a number from 5 to 8");
    }
    if serial
        .stop_bits
        .is_some_and(|bits| !(1..=2).contains(&bits))
    {
        return invalid("stop bits must be 1 or 2");
    }
    let mut web_links: Vec<WebLink> = Vec::new();
    for link in &edit.web_links {
        let link = WebLink::new(link.name.trim(), link.url.trim());
        match (link.name.is_empty(), link.url.is_empty()) {
            (true, true) => continue,
            (true, false) => return invalid("a web link has a URL but no name"),
            (false, true) => {
                return Err(EditError::Invalid(format!(
                    "web link `{}` has no URL",
                    link.name
                )))
            }
            (false, false) => {}
        }
        if web_links.iter().any(|seen| seen.name == link.name) {
            return Err(EditError::Invalid(format!(
                "web link `{}` appears twice",
                link.name
            )));
        }
        web_links.push(link);
    }
    Ok(Settings {
        credential_profile: normalize(edit.credential_profile.as_deref()),
        jump_host: normalize(edit.jump_host.as_deref()),
        platform: normalize(edit.platform.as_deref()),
        web_links,
        serial: SerialSettings {
            port: normalize(edit.serial.port.as_deref()),
            ..edit.serial.clone()
        },
        ..edit.clone()
    })
}

/// Applies `edit`'s settings to `document`, which holds `current`; only
/// changed settings are touched. `set` puts a key in its place.
pub(super) fn apply_settings(
    document: &mut Mapping,
    current: &Settings,
    edit: &Settings,
    set: fn(&mut Mapping, &str, Value),
) -> Result<(), EditError> {
    let edit = checked(edit)?;
    let mut update = |key: &str, old: Option<Value>, new: Option<Value>| {
        if old == new {
            return;
        }
        match new {
            Some(value) => set(document, key, value),
            None => {
                document.shift_remove(key);
            }
        }
    };
    let text = |value: Option<&'static str>| value.map(Value::from);
    update(
        PROTOCOL,
        text(current.protocol.map(Protocol::as_str)),
        text(edit.protocol.map(Protocol::as_str)),
    );
    update(
        PORT,
        current.port.map(Value::from),
        edit.port.map(Value::from),
    );
    update(
        CONNECT_TIMEOUT,
        current.connect_timeout.map(Value::from),
        edit.connect_timeout.map(Value::from),
    );
    for (key, old, new) in [
        (
            CREDENTIAL_PROFILE,
            &current.credential_profile,
            &edit.credential_profile,
        ),
        (JUMP_HOST, &current.jump_host, &edit.jump_host),
        (PLATFORM, &current.platform, &edit.platform),
    ] {
        update(
            key,
            old.clone().map(Value::from),
            new.clone().map(Value::from),
        );
    }
    for (key, old, new) in [
        (LEGACY_CRYPTO, current.legacy_crypto, edit.legacy_crypto),
        (LOGGING, current.logging, edit.logging),
        (LOG_TIMESTAMPS, current.log_timestamps, edit.log_timestamps),
        (
            BACKUP_ON_LOGIN,
            current.backup_on_login,
            edit.backup_on_login,
        ),
    ] {
        update(key, old.map(Value::from), new.map(Value::from));
    }
    update(
        ENVIRONMENT,
        text(current.environment.map(Environment::as_str)),
        text(edit.environment.map(Environment::as_str)),
    );
    update(
        TERMINAL_TYPE,
        text(current.terminal_type.map(TerminalType::as_str)),
        text(edit.terminal_type.map(TerminalType::as_str)),
    );
    update(
        PASTE_LINE_DELAY,
        current.paste_line_delay.map(Value::from),
        edit.paste_line_delay.map(Value::from),
    );

    if edit.serial != current.serial {
        let mut serial = document
            .get(SERIAL)
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        let parameters: [(&str, Option<Value>); 6] = [
            (SERIAL_PORT, edit.serial.port.clone().map(Value::from)),
            (BAUD_RATE, edit.serial.baud_rate.map(Value::from)),
            (DATA_BITS, edit.serial.data_bits.map(Value::from)),
            (PARITY, text(edit.serial.parity.map(Parity::as_str))),
            (STOP_BITS, edit.serial.stop_bits.map(Value::from)),
            (
                FLOW_CONTROL,
                text(edit.serial.flow_control.map(FlowControl::as_str)),
            ),
        ];
        for (key, value) in parameters {
            match value {
                Some(value) => {
                    // A number keeps its exact form if unchanged.
                    if serial.get(key) != Some(&value) {
                        serial.insert(key.into(), value);
                    }
                }
                None => {
                    serial.shift_remove(key);
                }
            }
        }
        if serial.is_empty() {
            document.shift_remove(SERIAL);
        } else {
            set(document, SERIAL, Value::Mapping(serial));
        }
    }

    if edit.web_links != current.web_links {
        if edit.web_links.is_empty() {
            document.shift_remove(WEB_LINKS);
        } else {
            let links = edit
                .web_links
                .into_iter()
                .map(|link| (link.name.into(), link.url.into()))
                .collect();
            set(document, WEB_LINKS, Value::Mapping(links));
        }
    }
    Ok(())
}
