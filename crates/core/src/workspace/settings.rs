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

use super::{normalize, optional_text, EditError};

/// File keys of the settings.
pub(super) const PROTOCOL: &str = "protocol";
pub(super) const PORT: &str = "port";
pub(super) const CREDENTIAL_PROFILE: &str = "credential_profile";
pub(super) const JUMP_HOST: &str = "jump_host";
pub(super) const LEGACY_CRYPTO: &str = "legacy_crypto";
pub(super) const LOGGING: &str = "logging";
pub(super) const ENVIRONMENT: &str = "environment";
pub(super) const SERIAL: &str = "serial";
pub(super) const WEB_LINKS: &str = "web_links";

/// Serial parameter keys, inside `serial`.
const BAUD_RATE: &str = "baud_rate";
const DATA_BITS: &str = "data_bits";
const PARITY: &str = "parity";
const STOP_BITS: &str = "stop_bits";
const FLOW_CONTROL: &str = "flow_control";

/// Serial parameters when nothing sets them: 9600 8N1, no flow control.
pub const DEFAULT_BAUD_RATE: u32 = 9600;
pub const DEFAULT_DATA_BITS: u8 = 8;
pub const DEFAULT_STOP_BITS: u8 = 1;

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
    pub logging: Option<bool>,
    pub environment: Option<Environment>,
    /// Links set here, in file order; names are unique.
    pub web_links: Vec<WebLink>,
    pub serial: SerialSettings,
}

/// Serial line parameters set at one level; each one inherits on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialSettings {
    pub baud_rate: Option<u32>,
    /// 5 to 8.
    pub data_bits: Option<u8>,
    pub parity: Option<Parity>,
    /// 1 or 2.
    pub stop_bits: Option<u8>,
    pub flow_control: Option<FlowControl>,
}

/// A named link to a web page about the host, such as its GUI. The URL is a
/// template that a later feature fills from the host's fields.
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
    pub logging: Effective<bool>,
    pub environment: Option<Effective<Environment>>,
    /// Merged by name: the order a name first appears, from the root down.
    pub web_links: Vec<Effective<WebLink>>,
    pub serial: EffectiveSerial,
    /// Every tag from the root down, once each, from its nearest origin.
    pub tags: Vec<Effective<String>>,
}

/// Effective serial line parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveSerial {
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
        logging: or_default(nearest(levels, |s| s.logging), true),
        environment: nearest(levels, |s| s.environment),
        web_links,
        serial: EffectiveSerial {
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
        logging: read_flag(document, LOGGING)?,
        environment: read_choice(
            document,
            ENVIRONMENT,
            Environment::parse,
            Environment::expected,
        )?,
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
        web_links,
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
    for (key, old, new) in [
        (
            CREDENTIAL_PROFILE,
            &current.credential_profile,
            &edit.credential_profile,
        ),
        (JUMP_HOST, &current.jump_host, &edit.jump_host),
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
    ] {
        update(key, old.map(Value::from), new.map(Value::from));
    }
    update(
        ENVIRONMENT,
        text(current.environment.map(Environment::as_str)),
        text(edit.environment.map(Environment::as_str)),
    );

    if edit.serial != current.serial {
        let mut serial = document
            .get(SERIAL)
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        let parameters: [(&str, Option<Value>); 5] = [
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
