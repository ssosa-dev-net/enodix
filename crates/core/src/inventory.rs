//! The resolved inventory export: what the headless binary (`enodix-cli
//! inventory`) prints, and what the Python bridge builds its Nornir
//! inventory from.
//!
//! [`export`] takes a workspace and a [`Limit`] (everything, a folder and
//! the folders below it, a saved filter, or host ids) and gives every host
//! in it with the settings that apply to it once inheritance is resolved,
//! its jump hosts in connect order, the folders it is in, its tags
//! (inherited ones included) and its custom fields, plus the folders that
//! hold those hosts and the workspace's problems (a bad file never stops
//! the export). Credential profiles appear by name only: nothing here
//! reads a credential store. Custom fields whose names say they hold a
//! secret (`import::is_secret_name`, as the importers judge them) are left
//! out and named in `withheld`; web links (URL templates, which may carry a
//! login) are not exported.

use std::collections::HashSet;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::import::is_secret_name;
use crate::names;
use crate::session::{DEFAULT_SSH_PORT, DEFAULT_TELNET_PORT};
use crate::workspace::{
    CustomField, Environment, FlowControl, Folder, HostId, Parity, Problem, Protocol, ResolvedHost,
    TerminalType, Workspace,
};

/// The version of the export's format. It goes up when a field changes
/// meaning or goes away; new fields may be added without a change.
pub const FORMAT: u32 = 1;

/// Which hosts to export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Limit {
    /// Every host of the workspace.
    All,
    /// The hosts in the folder at this path and in the folders below it.
    /// `/` or `\` separate folders; matched exactly, else ignoring case;
    /// `""` is the root.
    Folder(String),
    /// The hosts the saved filter of this name (ignoring case) matches.
    Filter(String),
    /// These hosts, in this order, once each: host ids, matched exactly,
    /// else ignoring case.
    Hosts(Vec<String>),
}

/// Why nothing could be exported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    /// [`Limit::Folder`] names no folder of the workspace.
    #[error("there is no folder {0:?} in the workspace")]
    UnknownFolder(String),
    /// [`Limit::Filter`] names no saved filter (a badly defined one is a
    /// problem and so unknown too).
    #[error("there is no saved filter {name:?} in the workspace ({})", known_filters(.known))]
    UnknownFilter {
        name: String,
        /// The names of the filters the workspace has, in file order.
        known: Vec<String>,
    },
    #[error("{} not in the workspace: {}", if .0.len() == 1 { "this host is" } else { "these hosts are" }, .0.join(", "))]
    /// References in [`Limit::Hosts`] that name no host, as given, once
    /// each.
    UnknownHosts(Vec<String>),
    /// References in [`Limit::Hosts`] that name hosts outside the scope
    /// given to [`export_within`] (a script job's hosts), as given.
    #[error("{} not among the hosts of the job this script runs in: {}", if .0.len() == 1 { "this host is" } else { "these hosts are" }, .0.join(", "))]
    OutsideJob(Vec<String>),
}

fn known_filters(known: &[String]) -> String {
    if known.is_empty() {
        "it has none".into()
    } else {
        format!("saved filters: {}", known.join(", "))
    }
}

/// The exported inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InventoryExport {
    /// [`FORMAT`].
    pub format: u32,
    pub workspace: ExportedWorkspace,
    /// Ordered by id, except for [`Limit::Hosts`] (the order given).
    pub hosts: Vec<ExportedHost>,
    /// The folders holding the exported hosts, their parents included and
    /// the root left out, ordered by path.
    pub folders: Vec<ExportedFolder>,
    /// The files of the workspace that could not be loaded (whatever the
    /// limit): what they define is missing from the export.
    pub problems: Vec<Problem>,
}

/// Which workspace the export comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedWorkspace {
    /// The folder it was opened from.
    pub root: String,
    /// The name of that folder.
    pub name: String,
    /// Its identifier, if it has one yet (see [`Workspace::id`]).
    pub id: Option<String>,
}

/// One host as exported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedHost {
    pub id: HostId,
    pub name: String,
    /// The management address.
    pub address: String,
    /// The path of the folder the host file is in (`""` for the root).
    pub folder: String,
    /// The folders it is in, from the top one down to its own; the root
    /// left out.
    pub folders: Vec<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub site: Option<String>,
    pub building: Option<String>,
    pub room: Option<String>,
    pub rack: Option<String>,
    pub rack_unit: Option<String>,
    /// The role id, as written.
    pub role: Option<String>,
    pub description: Option<String>,
    /// Every tag that applies, inherited ones included, once each.
    pub tags: Vec<String>,
    /// The host's own fields, in file order (a JSON object), without the
    /// ones in `withheld`.
    #[serde(serialize_with = "custom_map")]
    pub custom: Vec<CustomField>,
    /// What was left out because its name says it holds a secret
    /// (`custom.<key>`).
    pub withheld: Vec<String>,
    pub settings: ExportedSettings,
    /// The jump hosts to go through, in connect order (the first is
    /// reached directly); empty for a direct connection, when the chain is
    /// broken, and for a host not reached over SSH (only SSH is tunnelled).
    pub jump_hosts: Vec<ExportedJumpHost>,
    /// Why the jump hosts of an SSH host cannot be worked out, if they
    /// cannot.
    pub jump_chain_error: Option<String>,
}

/// The settings that apply to a host, values only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedSettings {
    pub protocol: Protocol,
    /// The port that applies, else the protocol's default (22 for SSH, 23
    /// for telnet); `None` for a serial host nothing gives a port.
    pub port: Option<u16>,
    /// The name of the credential profile; never a secret.
    pub credential_profile: Option<String>,
    /// The jump host as written.
    pub jump_host: Option<String>,
    pub legacy_crypto: bool,
    /// Seconds.
    pub connect_timeout: u32,
    pub logging: bool,
    pub backup_on_login: bool,
    pub environment: Option<Environment>,
    /// Milliseconds.
    pub paste_line_delay: u32,
    pub terminal_type: TerminalType,
    pub platform: Option<String>,
    pub serial: ExportedSerial,
}

/// The serial line parameters that apply to a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedSerial {
    pub port: Option<String>,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow_control: FlowControl,
}

/// A jump host, resolved with its own settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedJumpHost {
    pub id: HostId,
    pub name: String,
    pub address: String,
    /// Its SSH port.
    pub port: u16,
    pub credential_profile: Option<String>,
}

/// A folder holding exported hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportedFolder {
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    /// The tags its own settings file sets.
    pub tags: Vec<String>,
}

/// Exports the hosts `limit` names from `workspace`.
pub fn export(workspace: &Workspace, limit: &Limit) -> Result<InventoryExport, ExportError> {
    export_within(workspace, limit, None)
}

/// Exports the hosts `limit` names from `workspace` that are also in
/// `scope`, if given (the hosts of the script job asking): a folder, a
/// filter or everything gives only those of them in the scope; host ids
/// outside it are refused ([`ExportError::OutsideJob`]).
pub fn export_within(
    workspace: &Workspace,
    limit: &Limit,
    scope: Option<&[HostId]>,
) -> Result<InventoryExport, ExportError> {
    let mut ids = selected(workspace, limit)?;
    if let Some(scope) = scope {
        let scope: HashSet<&HostId> = scope.iter().collect();
        if let Limit::Hosts(references) = limit {
            let outside: Vec<String> = references
                .iter()
                .filter(|reference| {
                    workspace
                        .find_host_id(reference.trim())
                        .is_some_and(|id| !scope.contains(&id))
                })
                .fold(Vec::new(), |mut outside, reference| {
                    if !outside.contains(reference) {
                        outside.push(reference.clone());
                    }
                    outside
                });
            if !outside.is_empty() {
                return Err(ExportError::OutsideJob(outside));
            }
        }
        ids.retain(|id| scope.contains(id));
    }
    let hosts: Vec<ExportedHost> = ids
        .iter()
        .filter_map(|id| workspace.resolve_host(id))
        .map(|host| exported_host(workspace, &host))
        .collect();
    let mut paths: Vec<&str> = hosts
        .iter()
        .flat_map(|host| host.folders.iter().map(String::as_str))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let folders = paths
        .into_iter()
        .filter_map(|path| workspace.folder(path))
        .map(|folder| ExportedFolder {
            path: folder.path.clone(),
            name: folder.name.clone(),
            description: folder.description.clone(),
            tags: folder.tags.clone(),
        })
        .collect();
    let root = workspace.root();
    Ok(InventoryExport {
        format: FORMAT,
        workspace: ExportedWorkspace {
            root: root.display().to_string(),
            name: workspace.tree().name.clone(),
            id: workspace.id().map(str::to_owned),
        },
        hosts,
        folders,
        problems: workspace.problems().to_vec(),
    })
}

/// The ids of the hosts `limit` names, in export order.
fn selected(workspace: &Workspace, limit: &Limit) -> Result<Vec<HostId>, ExportError> {
    match limit {
        Limit::All => Ok(sorted(hosts_below(workspace.tree()))),
        Limit::Folder(path) => {
            let folder = find_folder(workspace.tree(), path)
                .ok_or_else(|| ExportError::UnknownFolder(path.clone()))?;
            Ok(sorted(hosts_below(folder)))
        }
        Limit::Filter(name) => {
            let wanted = names::folded(name.trim());
            let filter = workspace
                .filters()
                .iter()
                .find(|filter| names::folded(&filter.name) == wanted)
                .ok_or_else(|| ExportError::UnknownFilter {
                    name: name.clone(),
                    known: workspace.filters().iter().map(|f| f.name.clone()).collect(),
                })?;
            Ok(workspace.apply_filter(filter).hosts)
        }
        Limit::Hosts(references) => {
            let mut ids: Vec<HostId> = Vec::new();
            let mut seen: HashSet<HostId> = HashSet::new();
            let mut unknown: Vec<String> = Vec::new();
            for reference in references {
                match workspace.find_host_id(reference.trim()) {
                    Some(id) if seen.insert(id.clone()) => ids.push(id),
                    Some(_) => {}
                    None if !unknown.contains(reference) => unknown.push(reference.clone()),
                    None => {}
                }
            }
            if unknown.is_empty() {
                Ok(ids)
            } else {
                Err(ExportError::UnknownHosts(unknown))
            }
        }
    }
}

fn sorted(mut ids: Vec<HostId>) -> Vec<HostId> {
    ids.sort();
    ids
}

/// Every host in `folder` and below.
fn hosts_below(folder: &Folder) -> Vec<HostId> {
    let mut ids: Vec<HostId> = folder.hosts.iter().map(|host| host.id.clone()).collect();
    for child in &folder.folders {
        ids.extend(hosts_below(child));
    }
    ids
}

/// The folder at `path` below `root`: each name (trimmed) matched exactly,
/// else ignoring case.
fn find_folder<'a>(root: &'a Folder, path: &str) -> Option<&'a Folder> {
    path.split(['/', '\\'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .try_fold(root, |folder, name| {
            folder
                .folders
                .iter()
                .find(|child| child.name == name)
                .or_else(|| {
                    let lower = names::folded(name);
                    folder
                        .folders
                        .iter()
                        .find(|child| names::folded(&child.name) == lower)
                })
        })
}

/// The folder paths from the top one down to `folder`, the root left out.
fn folder_chain(folder: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut path = String::new();
    for name in folder.split('/').filter(|name| !name.is_empty()) {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(name);
        chain.push(path.clone());
    }
    chain
}

fn exported_host(workspace: &Workspace, resolved: &ResolvedHost) -> ExportedHost {
    let host = &resolved.host;
    let folder = host
        .id
        .as_str()
        .rsplit_once('/')
        .map_or("", |(folder, _)| folder)
        .to_owned();
    let (jump_hosts, jump_chain_error) = if resolved.settings.protocol.value != Protocol::Ssh {
        (Vec::new(), None)
    } else {
        match workspace.jump_chain(resolved) {
            Ok(chain) => (chain.iter().map(exported_jump_host).collect(), None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        }
    };
    let (withheld, custom): (Vec<&CustomField>, Vec<&CustomField>) = host
        .custom
        .iter()
        .partition(|field| is_secret_name(&field.key));
    ExportedHost {
        id: host.id.clone(),
        name: host.name.clone(),
        address: host.address.clone(),
        folders: folder_chain(&folder),
        folder,
        vendor: host.vendor.clone(),
        model: host.model.clone(),
        serial_number: host.serial_number.clone(),
        site: host.site.clone(),
        building: host.building.clone(),
        room: host.room.clone(),
        rack: host.rack.clone(),
        rack_unit: host.rack_unit.clone(),
        role: host.role.clone(),
        description: host.description.clone(),
        tags: resolved
            .settings
            .tags
            .iter()
            .map(|tag| tag.value.clone())
            .collect(),
        custom: custom.into_iter().cloned().collect(),
        withheld: withheld
            .iter()
            .map(|field| format!("custom.{}", field.key))
            .collect(),
        settings: exported_settings(resolved),
        jump_hosts,
        jump_chain_error,
    }
}

fn exported_settings(resolved: &ResolvedHost) -> ExportedSettings {
    let settings = &resolved.settings;
    let protocol = settings.protocol.value;
    let serial = &settings.serial;
    ExportedSettings {
        protocol,
        port: match protocol {
            Protocol::Ssh => Some(resolved.port_or(DEFAULT_SSH_PORT)),
            Protocol::Telnet => Some(resolved.port_or(DEFAULT_TELNET_PORT)),
            Protocol::Serial => settings.port.as_ref().map(|port| port.value),
        },
        credential_profile: settings
            .credential_profile
            .as_ref()
            .map(|profile| profile.value.clone()),
        jump_host: settings.jump_host.as_ref().map(|jump| jump.value.clone()),
        legacy_crypto: settings.legacy_crypto.value,
        connect_timeout: settings.connect_timeout.value,
        logging: settings.logging.value,
        backup_on_login: settings.backup_on_login.value,
        environment: settings.environment.as_ref().map(|e| e.value),
        paste_line_delay: settings.paste_line_delay.value,
        terminal_type: settings.terminal_type.value,
        platform: settings.platform.as_ref().map(|p| p.value.clone()),
        serial: ExportedSerial {
            port: serial.port.as_ref().map(|port| port.value.clone()),
            baud_rate: serial.baud_rate.value,
            data_bits: serial.data_bits.value,
            parity: serial.parity.value,
            stop_bits: serial.stop_bits.value,
            flow_control: serial.flow_control.value,
        },
    }
}

fn exported_jump_host(jump: &ResolvedHost) -> ExportedJumpHost {
    ExportedJumpHost {
        id: jump.host.id.clone(),
        name: jump.host.name.clone(),
        address: jump.host.address.clone(),
        port: jump.port_or(DEFAULT_SSH_PORT),
        credential_profile: jump
            .settings
            .credential_profile
            .as_ref()
            .map(|profile| profile.value.clone()),
    }
}

/// Custom fields as a JSON object, in their order.
fn custom_map<S: Serializer>(fields: &[CustomField], serializer: S) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(fields.len()))?;
    for field in fields {
        map.serialize_entry(&field.key, &field.value)?;
    }
    map.end()
}
