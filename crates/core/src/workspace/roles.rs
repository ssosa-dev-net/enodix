//! Device roles: where a host sits in the network.
//!
//! The app ships a starter list ([`Role`]s with [`RoleSource::BuiltIn`]). A
//! workspace adds its own in [`ROLES_FILE`] at its root, a mapping of role
//! ids to their definition:
//!
//! ```yaml
//! spine:
//!   name: Spine        # optional; the id when absent
//!   icon: switch       # one of the RoleIcon names
//!   color: "#2563eb"   # #rrggbb or #rgb
//! ```
//!
//! A workspace role with the id of a built-in one replaces it. A host names
//! its role by id (`role` in its file, matched ignoring case); a role the
//! workspace does not define shows as a neutral one
//! ([`RoleSource::Undefined`]).

use serde::Serialize;
use serde_norway::{Mapping, Value};

use super::scalar_text;

/// The file at the workspace root that defines the workspace's own roles.
pub const ROLES_FILE: &str = "_roles.yaml";

/// Keys of a role definition.
const NAME: &str = "name";
const ICON: &str = "icon";
const COLOR: &str = "color";

/// The colour of a role the workspace does not define.
pub const NEUTRAL_ROLE_COLOR: &str = "#8a8f98";

/// A role a host can have, with how the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Role {
    /// What host files name it by.
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    pub icon: RoleIcon,
    /// `#rrggbb`, in lower case.
    pub color: String,
    pub source: RoleSource,
}

impl Role {
    /// How a role the workspace does not define shows: under its id, with
    /// the neutral icon and colour.
    pub fn undefined(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: id.to_string(),
            icon: RoleIcon::Unknown,
            color: NEUTRAL_ROLE_COLOR.into(),
            source: RoleSource::Undefined,
        }
    }
}

/// Where a role comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleSource {
    /// The app's starter list.
    BuiltIn,
    /// The workspace's roles file.
    Workspace,
    /// A host names a role that is not defined; it shows neutrally.
    Undefined,
}

macro_rules! icons {
    ($($variant:ident = $text:literal),+ $(,)?) => {
        /// The icons a role can have, named in files as written here. The
        /// interface draws each one.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum RoleIcon {
            $($variant),+
        }

        impl RoleIcon {
            const ALL: &[Self] = &[$(Self::$variant),+];

            /// The icon as named in files.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            fn parse(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|icon| icon.as_str() == text)
            }
        }
    };
}

icons!(
    Core = "core",
    Distribution = "distribution",
    Access = "access",
    Router = "router",
    Firewall = "firewall",
    LoadBalancer = "load_balancer",
    WirelessController = "wireless_controller",
    AccessPoint = "access_point",
    ConsoleServer = "console_server",
    Server = "server",
    Switch = "switch",
    Cloud = "cloud",
    Storage = "storage",
    Generic = "generic",
    // The neutral icon of a role the workspace does not define.
    Unknown = "unknown",
);

/// The starter list: id, name, icon, colour.
const BUILT_IN: [(&str, &str, RoleIcon, &str); 10] = [
    ("core", "Core", RoleIcon::Core, "#7c3aed"),
    (
        "distribution",
        "Distribution",
        RoleIcon::Distribution,
        "#2563eb",
    ),
    ("access", "Access", RoleIcon::Access, "#0284c7"),
    (
        "edge_router",
        "Edge/WAN router",
        RoleIcon::Router,
        "#ea580c",
    ),
    ("firewall", "Firewall", RoleIcon::Firewall, "#dc2626"),
    (
        "load_balancer",
        "Load balancer",
        RoleIcon::LoadBalancer,
        "#ca8a04",
    ),
    (
        "wireless_controller",
        "Wireless controller",
        RoleIcon::WirelessController,
        "#db2777",
    ),
    (
        "access_point",
        "Access point",
        RoleIcon::AccessPoint,
        "#16a34a",
    ),
    (
        "console_server",
        "Out-of-band/console server",
        RoleIcon::ConsoleServer,
        "#0d9488",
    ),
    ("server", "Server", RoleIcon::Server, "#a16207"),
];

/// The built-in roles, in order.
pub(super) fn built_in() -> Vec<Role> {
    BUILT_IN
        .iter()
        .map(|&(id, name, icon, color)| Role {
            id: id.into(),
            name: name.into(),
            icon,
            color: color.into(),
            source: RoleSource::BuiltIn,
        })
        .collect()
}

/// The built-in roles with the workspace's roles file (`document`) applied,
/// and a message for each role it defines badly (that role is left out).
pub(super) fn with_workspace_roles(document: &Mapping) -> (Vec<Role>, Vec<String>) {
    let mut roles = built_in();
    let mut errors = Vec::new();
    for (key, definition) in document {
        let Some(id) = scalar_text(key).map(|id| id.trim().to_string()) else {
            errors.push("role ids must be text".into());
            continue;
        };
        if id.is_empty() {
            errors.push("a role has an empty id".into());
            continue;
        }
        let role = match workspace_role(&id, definition) {
            Ok(role) => role,
            Err(message) => {
                errors.push(format!("role `{id}`: {message}"));
                continue;
            }
        };
        match roles
            .iter_mut()
            .find(|known| known.id.eq_ignore_ascii_case(&id))
        {
            Some(known) if known.source == RoleSource::BuiltIn => *known = role,
            Some(_) => errors.push(format!("role `{id}` is defined twice")),
            None => roles.push(role),
        }
    }
    (roles, errors)
}

fn workspace_role(id: &str, definition: &Value) -> Result<Role, String> {
    let Value::Mapping(definition) = definition else {
        return Err(format!("expected `{NAME}`, `{ICON}` and `{COLOR}`"));
    };
    let name = match definition.get(NAME) {
        None | Some(Value::Null) => id.to_string(),
        Some(value) => scalar_text(value)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("`{NAME}` must be text"))?,
    };
    let icon = match definition.get(ICON) {
        None | Some(Value::Null) => return Err(format!("missing `{ICON}`")),
        Some(value) => value
            .as_str()
            .and_then(|text| RoleIcon::parse(text.trim()))
            .ok_or_else(|| {
                let names: Vec<&str> = RoleIcon::ALL.iter().map(|icon| icon.as_str()).collect();
                format!("`{ICON}` must be one of {}", names.join(", "))
            })?,
    };
    let color = match definition.get(COLOR) {
        None | Some(Value::Null) => return Err(format!("missing `{COLOR}`")),
        Some(value) => value
            .as_str()
            .and_then(|text| parse_color(text.trim()))
            .ok_or_else(|| format!("`{COLOR}` must be a colour like \"#2563eb\""))?,
    };
    Ok(Role {
        id: id.to_string(),
        name,
        icon,
        color,
        source: RoleSource::Workspace,
    })
}

/// `#rrggbb` or `#rgb` as `#rrggbb` in lower case.
fn parse_color(text: &str) -> Option<String> {
    let digits = text.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digits = digits.to_ascii_lowercase();
    match digits.len() {
        6 => Some(format!("#{digits}")),
        3 => Some(digits.chars().fold(String::from("#"), |mut color, c| {
            color.push(c);
            color.push(c);
            color
        })),
        _ => None,
    }
}

/// The role called `id` among `roles`, ignoring case.
pub(super) fn find<'a>(roles: &'a [Role], id: &str) -> Option<&'a Role> {
    let id = id.trim();
    roles.iter().find(|role| role.id.eq_ignore_ascii_case(id))
}
