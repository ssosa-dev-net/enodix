//! Nornir import: the hosts of a Nornir simple inventory.
//!
//! A simple inventory is a hosts file, optionally with a groups file and a
//! defaults file ([`NornirInventory::parse`] / [`NornirInventory::read`]).
//! Each host goes to the target folder as a host file named after its key:
//! `hostname` is the management address, `port` the port setting,
//! `platform` the platform field. Values a host does not set itself come
//! from its groups as Nornir resolves them (its groups in order, each
//! followed by its own parent groups, depth first), then from the defaults;
//! `data` is resolved key by key the same way, and every entry becomes a
//! custom field (nested mappings as `parent.child` keys, lists of plain
//! values joined with `, `, anything else as JSON). Every group a host
//! belongs to, its groups' parents included, becomes a tag; without a
//! groups file, the groups a host names are its tags.
//!
//! Never imported: passwords and other secrets (any key named like one, at
//! any depth: reported once per place in the files), usernames (they belong
//! in a credential profile), `connection_options`, and keys a simple
//! inventory does not have; each is listed in the report as skipped. Hosts
//! that exist already are handled as in every import (see
//! [`crate::import`]).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use serde::Serialize;
use serde_norway::{Mapping, Value};

use super::csv::HostField;
use super::{
    existing_hosts, is_secret_name, write_planned, FieldValue, ImportReport, ImportedHost, Planned,
    PreviewOutcome, ReportEntry, PASSWORDS_SKIPPED,
};
use crate::names;
use crate::workspace::import::check_import_name;
use crate::workspace::{bad_address, BadAddress};
use crate::workspace::{join, CustomField, HostId, Workspace, HOST_EXTENSION};

/// One of the files of a simple inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NornirFile {
    Hosts,
    Groups,
    Defaults,
}

impl NornirFile {
    /// What the file's top level must be.
    fn expected(self) -> &'static str {
        match self {
            Self::Hosts | Self::Groups => "a mapping of names to entries",
            Self::Defaults => "a mapping of settings",
        }
    }
}

impl fmt::Display for NornirFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hosts => "hosts",
            Self::Groups => "groups",
            Self::Defaults => "defaults",
        })
    }
}

/// Inventory files that cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum NornirError {
    #[error("cannot read the {file} file: {reason}")]
    Io { file: NornirFile, reason: String },
    #[error("the {file} file is not valid YAML: {reason}")]
    Yaml { file: NornirFile, reason: String },
    #[error("the {file} file is not {}", file.expected())]
    NotAMapping { file: NornirFile },
}

/// An import that cannot run.
#[derive(Debug, thiserror::Error)]
pub enum NornirImportError {
    #[error("the target folder {0} is not in this workspace")]
    NoSuchFolder(String),
}

/// A host or group of an inventory file.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    /// Where its key is in its file, when it could be found.
    line: Option<usize>,
    value: Value,
}

/// A simple inventory as read: its hosts, groups and defaults.
#[derive(Debug, Clone)]
pub struct NornirInventory {
    hosts: Vec<Entry>,
    /// `None` without a groups file.
    groups: Option<Vec<Entry>>,
    defaults: Option<Mapping>,
}

impl NornirInventory {
    /// Reads the files at these paths.
    pub fn read(
        hosts: &Path,
        groups: Option<&Path>,
        defaults: Option<&Path>,
    ) -> Result<Self, NornirError> {
        let text = |path: &Path, file| {
            std::fs::read_to_string(path).map_err(|error| NornirError::Io {
                file,
                reason: super::read_reason(&error),
            })
        };
        let hosts = text(hosts, NornirFile::Hosts)?;
        let groups = groups.map(|p| text(p, NornirFile::Groups)).transpose()?;
        let defaults = defaults
            .map(|p| text(p, NornirFile::Defaults))
            .transpose()?;
        Self::parse(&hosts, groups.as_deref(), defaults.as_deref())
    }

    /// Reads the text of a hosts file, and of a groups and a defaults file
    /// if there are any. Empty files are empty inventories.
    pub fn parse(
        hosts: &str,
        groups: Option<&str>,
        defaults: Option<&str>,
    ) -> Result<Self, NornirError> {
        Ok(Self {
            hosts: entries(hosts, NornirFile::Hosts)?,
            groups: groups
                .map(|text| entries(text, NornirFile::Groups))
                .transpose()?,
            defaults: defaults
                .map(|text| mapping(text, NornirFile::Defaults))
                .transpose()?,
        })
    }
}

/// The top-level mapping of an inventory file (empty for an empty file).
fn mapping(text: &str, file: NornirFile) -> Result<Mapping, NornirError> {
    let yaml = |error: serde_norway::Error| NornirError::Yaml {
        file,
        reason: error.to_string(),
    };
    let mut value: Value = serde_norway::from_str(text).map_err(yaml)?;
    value.apply_merge().map_err(yaml)?;
    match value {
        Value::Null => Ok(Mapping::new()),
        Value::Mapping(mapping) => Ok(mapping),
        _ => Err(NornirError::NotAMapping { file }),
    }
}

/// The named entries of a hosts or groups file, in file order.
fn entries(text: &str, file: NornirFile) -> Result<Vec<Entry>, NornirError> {
    let lines = top_level_lines(text);
    Ok(mapping(text, file)?
        .into_iter()
        .filter_map(|(key, value)| {
            let name = scalar_text(&key)?;
            Some(Entry {
                line: lines.get(&name).copied(),
                name,
                value,
            })
        })
        .collect())
}

/// The line (1-based) of each key written at the start of a line: the
/// top-level keys of a block-style YAML file.
fn top_level_lines(text: &str) -> HashMap<String, usize> {
    let mut lines = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let Some(first) = line.chars().next() else {
            continue;
        };
        if first.is_whitespace() || matches!(first, '#' | '-' | '.' | '%' | '{' | '[') {
            continue;
        }
        let key = match first {
            '\'' | '"' => line[1..].find(first).map(|end| &line[1..=end]),
            _ => line.find(':').map(|end| line[..end].trim_end()),
        };
        if let Some(key) = key {
            let key = if first == '\'' {
                key.replace("''", "'")
            } else {
                key.to_string()
            };
            lines.entry(key).or_insert(index + 1);
        }
    }
    lines
}

/// A plain value as text (`None` for null, lists and mappings).
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Tagged(tagged) => scalar_text(&tagged.value),
        Value::Null | Value::Sequence(_) | Value::Mapping(_) => None,
    }
}

/// A value as it reads in YAML, for messages.
fn yaml_text(value: &Value) -> String {
    scalar_text(value).unwrap_or_else(|| {
        serde_norway::to_string(value)
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    })
}

/// The keys of a simple inventory's hosts, groups and defaults.
const HOSTNAME: &str = "hostname";
const PORT: &str = "port";
const PLATFORM: &str = "platform";
const USERNAME: &str = "username";
const GROUPS: &str = "groups";
const DATA: &str = "data";
const CONNECTION_OPTIONS: &str = "connection_options";

const USERNAME_SKIPPED: &str = "usernames belong in a credential profile; not imported";
const CONNECTION_OPTIONS_SKIPPED: &str = "connection options are not imported";
const UNKNOWN_SKIPPED: &str = "not part of a Nornir simple inventory; not imported";
const UNDEFINED_GROUP: &str = "not defined in the groups file: its hosts get the tag only";
const GROUP_NOT_A_MAPPING: &str = "the group's entry is not a mapping; not used";

/// A simple inventory and the folder its hosts go to.
#[derive(Debug, Clone)]
pub struct NornirImport {
    pub inventory: NornirInventory,
    /// The workspace folder the hosts go to; empty for the root.
    pub target: String,
}

/// What [`NornirImport::run`] would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NornirPreview {
    /// One per host of the hosts file, in file order.
    pub hosts: Vec<NornirPreviewHost>,
    /// What is never imported: passwords, usernames, connection options,
    /// unknown keys, groups the groups file lacks.
    pub skipped: Vec<ReportEntry>,
}

/// What would become of one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NornirPreviewHost {
    /// Where the host's key is in the hosts file.
    pub line: Option<usize>,
    /// The host's key.
    pub name: String,
    pub outcome: PreviewOutcome,
    /// The values the host would get; `None` when it fails.
    pub host: Option<ImportedHost>,
}

impl NornirImport {
    /// What importing would do, host by host. Writes nothing.
    pub fn preview(&self, workspace: &Workspace) -> Result<NornirPreview, NornirImportError> {
        if workspace.folder(&self.target).is_none() {
            return Err(NornirImportError::NoSuchFolder(self.target.clone()));
        }
        Ok(Resolver::new(&self.inventory).preview(workspace, &self.target))
    }

    /// Imports the hosts. Hosts that exist already are skipped unless their
    /// id is in `replace` (the user confirmed them); then they take the
    /// imported values and keep the rest. The workspace is reloaded once at
    /// the end, if anything was written.
    pub fn run(
        &self,
        workspace: &mut Workspace,
        replace: &[HostId],
    ) -> Result<ImportReport, NornirImportError> {
        let preview = self.preview(workspace)?;
        let mut report = ImportReport {
            skipped: preview.skipped.clone(),
            ..ImportReport::default()
        };
        let planned: Vec<Planned<'_>> = preview
            .hosts
            .iter()
            .map(|host| Planned {
                line: host.line,
                name: &host.name,
                outcome: &host.outcome,
                host: host.host.as_ref(),
            })
            .collect();
        write_planned(workspace, &planned, replace, &mut report);
        Ok(report)
    }
}

/// One level a host takes values from: itself, a group, the defaults; with
/// its secrets removed.
struct Level {
    mapping: Mapping,
}

impl Level {
    fn get(&self, key: &str) -> Option<&Value> {
        self.mapping.get(key).filter(|value| !value.is_null())
    }
}

/// Resolves the hosts of an inventory.
struct Resolver<'a> {
    inventory: &'a NornirInventory,
    /// Groups by name, secrets removed; `None` for one whose entry is not a
    /// mapping.
    groups: HashMap<&'a str, Option<Level>>,
    defaults: Option<Level>,
    skipped: Vec<ReportEntry>,
    /// Undefined groups already reported.
    undefined: HashSet<String>,
}

impl<'a> Resolver<'a> {
    fn new(inventory: &'a NornirInventory) -> Self {
        let mut skipped = Vec::new();
        let defaults = inventory.defaults.as_ref().map(|defaults| {
            let place = Place {
                label: "defaults".into(),
                line: None,
            };
            level(defaults.clone(), &place, false, &mut skipped)
        });
        let mut groups = HashMap::new();
        for group in inventory.groups.iter().flatten() {
            let place = Place {
                label: format!("group {}", group.name),
                line: group.line,
            };
            let level = match &group.value {
                Value::Null => Some(Level {
                    mapping: Mapping::new(),
                }),
                Value::Mapping(mapping) => Some(level(mapping.clone(), &place, true, &mut skipped)),
                _ => {
                    skipped.push(place.entry(None, GROUP_NOT_A_MAPPING));
                    None
                }
            };
            groups.entry(group.name.as_str()).or_insert(level);
        }
        Self {
            inventory,
            groups,
            defaults,
            skipped,
            undefined: HashSet::new(),
        }
    }

    fn preview(mut self, workspace: &Workspace, target: &str) -> NornirPreview {
        let existing = existing_hosts(workspace, target);
        // Hosts planned by lower-case name → their name and line.
        let mut planned: HashMap<String, (&str, Option<usize>)> = HashMap::new();
        let mut hosts = Vec::new();
        for entry in &self.inventory.hosts {
            let result = self.host(entry, target);
            let preview = |outcome, host| NornirPreviewHost {
                line: entry.line,
                name: entry.name.clone(),
                outcome,
                host,
            };
            let host = match result {
                Ok(host) => host,
                Err(reason) => {
                    hosts.push(preview(PreviewOutcome::Fail { reason }, None));
                    continue;
                }
            };
            let key = names::folded(&host.name);
            if let Some((first, line)) = planned.get(&key) {
                let first = match line {
                    Some(line) => format!("{first} (line {line})"),
                    None => first.to_string(),
                };
                let reason = format!("{first} already imports a host with this name");
                hosts.push(preview(PreviewOutcome::Fail { reason }, None));
                continue;
            }
            planned.insert(key.clone(), (&entry.name, entry.line));
            let outcome = match existing.get(&key) {
                Some(id) => PreviewOutcome::Exists { id: id.clone() },
                None => PreviewOutcome::Create {
                    id: HostId::new(join(target, &format!("{}.{HOST_EXTENSION}", host.name))),
                },
            };
            hosts.push(preview(outcome, Some(host)));
        }
        NornirPreview {
            hosts,
            skipped: self.skipped,
        }
    }

    /// The values the host `entry` gets, or why it cannot be imported.
    fn host(&mut self, entry: &Entry, target: &str) -> Result<ImportedHost, String> {
        let place = Place {
            label: format!("host {}", entry.name),
            line: entry.line,
        };
        let own = match &entry.value {
            Value::Null => Mapping::new(),
            Value::Mapping(mapping) => mapping.clone(),
            _ => return Err("the host's entry is not a mapping".into()),
        };
        let own = level(own, &place, true, &mut self.skipped);
        let name = check_import_name(&entry.name)?.to_string();

        let groups = self.extended_groups(&own)?;
        let mut chain: Vec<&Level> = vec![&own];
        chain.extend(
            groups
                .iter()
                .filter_map(|group| self.groups.get(group.as_str()))
                .flatten(),
        );
        chain.extend(&self.defaults);
        let inherited = |key: &str| chain.iter().find_map(|level| level.get(key));

        let address = match inherited(HOSTNAME) {
            None => {
                return Err("no hostname: not set on the host, its groups or the defaults".into())
            }
            Some(value) => scalar_text(value)
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .ok_or_else(|| format!("hostname {} is not an address", yaml_text(value)))?,
        };
        let mut port = inherited(PORT).map(port).transpose()?;
        // As New host checks it (#131); a port after the hostname is taken
        // as the port setting, which a Nornir inventory has.
        let address = match bad_address(&address) {
            None => address,
            Some(BadAddress::Port { host, port: after }) => {
                let after: u16 =
                    after
                        .parse()
                        .ok()
                        .filter(|after| *after > 0)
                        .ok_or_else(|| {
                            format!("hostname {address} has {after} after it, which is not a port")
                        })?;
                if let Some(set) = port.filter(|set| *set != after) {
                    return Err(format!(
                        "hostname {address} has port {after} after it, but port is {set}: write \
                         the hostname without the port, or make them the same"
                    ));
                }
                port = Some(after);
                host.to_string()
            }
            Some(bad) => return Err(bad.reason(&address, |_| String::new())),
        };
        let mut fields = Vec::new();
        if let Some(value) = inherited(PLATFORM) {
            let platform = scalar_text(value)
                .ok_or_else(|| format!("platform {} is not text", yaml_text(value)))?;
            if !platform.trim().is_empty() {
                fields.push(FieldValue {
                    field: HostField::Platform,
                    value: platform.trim().to_string(),
                });
            }
        }

        // Data, key by key, nearest level first.
        let mut data: Vec<(String, &Value)> = Vec::new();
        for level in &chain {
            let Some(Value::Mapping(mapping)) = level.get(DATA) else {
                continue;
            };
            for (key, value) in mapping {
                let Some(key) = scalar_text(key) else {
                    continue;
                };
                if !data.iter().any(|(seen, _)| *seen == key) {
                    data.push((key, value));
                }
            }
        }
        let mut custom = Vec::new();
        for (key, value) in data {
            custom_fields(&key, value, &mut custom);
        }

        Ok(ImportedHost {
            name,
            address,
            folder: target.to_string(),
            fields,
            port,
            jump_host: None,
            tags: groups,
            custom,
        })
    }

    /// The groups the host is in: those it names, each followed by its own
    /// parent groups (depth first), each once. Groups the groups file lacks
    /// are reported once.
    fn extended_groups(&mut self, own: &Level) -> Result<Vec<String>, String> {
        let named = group_names(own.get(GROUPS))?;
        let mut groups = Vec::new();
        let mut stack: Vec<String> = named.into_iter().rev().collect();
        while let Some(group) = stack.pop() {
            if groups.contains(&group) {
                continue;
            }
            groups.push(group.clone());
            match self.groups.get(group.as_str()) {
                Some(Some(level)) => {
                    // A parent list that is not one is left out.
                    let parents = group_names(level.get(GROUPS)).unwrap_or_default();
                    stack.extend(parents.into_iter().rev());
                }
                Some(None) => {}
                // Reported once, and only when there is a groups file.
                None if self.inventory.groups.is_some() && self.undefined.insert(group.clone()) => {
                    let place = Place {
                        label: format!("group {group}"),
                        line: None,
                    };
                    self.skipped.push(place.entry(None, UNDEFINED_GROUP));
                }
                None => {}
            }
        }
        Ok(groups)
    }
}

/// Where something was found in the files, for the report.
struct Place {
    /// `defaults`, `group <name>` or `host <name>`.
    label: String,
    line: Option<usize>,
}

impl Place {
    /// A report entry about `key` here (`None`: the place itself).
    fn entry(&self, key: Option<&str>, reason: &str) -> ReportEntry {
        ReportEntry {
            line: self.line,
            name: match key {
                Some(key) => format!("{}: {key}", self.label),
                None => self.label.clone(),
            },
            host: None,
            reason: Some(reason.into()),
        }
    }
}

/// A host's, group's or defaults' entry made into a [`Level`]: secrets
/// removed at any depth, and what is never imported reported, in file
/// order.
fn level(
    mapping: Mapping,
    place: &Place,
    has_groups: bool,
    skipped: &mut Vec<ReportEntry>,
) -> Level {
    let mut kept = Mapping::new();
    for (key, mut value) in mapping {
        let name = scalar_text(&key).unwrap_or_default();
        if is_secret_name(&name) {
            if !value.is_null() {
                skipped.push(place.entry(Some(&name), PASSWORDS_SKIPPED));
            }
            continue;
        }
        let mut secrets = Vec::new();
        remove_secrets(&mut value, &name, &mut secrets);
        for path in secrets {
            skipped.push(place.entry(Some(&path), PASSWORDS_SKIPPED));
        }
        let reason = match name.as_str() {
            HOSTNAME | PORT | PLATFORM | DATA => None,
            GROUPS if has_groups => None,
            USERNAME => Some(USERNAME_SKIPPED),
            CONNECTION_OPTIONS => Some(CONNECTION_OPTIONS_SKIPPED),
            _ => Some(UNKNOWN_SKIPPED),
        };
        match reason {
            Some(reason) => skipped.push(place.entry(Some(&name), reason)),
            None => {
                kept.insert(key, value);
            }
        }
    }
    Level { mapping: kept }
}

/// Removes every key named like a secret from the mappings in `value` (at
/// any depth, lists included), adding each one's dotted path (`path.key`)
/// to `removed`, in file order.
fn remove_secrets(value: &mut Value, path: &str, removed: &mut Vec<String>) {
    match value {
        Value::Mapping(mapping) => {
            let mut kept = Mapping::new();
            for (key, mut value) in std::mem::take(mapping) {
                let name = scalar_text(&key).unwrap_or_default();
                let here = format!("{path}.{name}");
                if is_secret_name(&name) {
                    if !value.is_null() {
                        removed.push(here);
                    }
                    continue;
                }
                remove_secrets(&mut value, &here, removed);
                kept.insert(key, value);
            }
            *mapping = kept;
        }
        Value::Sequence(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                remove_secrets(item, &format!("{path}.{index}"), removed);
            }
        }
        Value::Tagged(tagged) => remove_secrets(&mut tagged.value, path, removed),
        _ => {}
    }
}

/// The group names of a `groups` value (a list, or one name).
fn group_names(value: Option<&Value>) -> Result<Vec<String>, String> {
    let not_names = || "groups is not a list of group names".to_string();
    match value {
        None => Ok(Vec::new()),
        Some(Value::Sequence(items)) => items
            .iter()
            .map(|item| {
                scalar_text(item)
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .ok_or_else(not_names)
            })
            .collect(),
        Some(value) => scalar_text(value)
            .map(|name| vec![name.trim().to_string()])
            .ok_or_else(not_names),
    }
}

/// A port setting: a whole number from 1 to 65535.
fn port(value: &Value) -> Result<u16, String> {
    let number = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    number
        .and_then(|number| u16::try_from(number).ok())
        .filter(|port| *port > 0)
        .ok_or_else(|| {
            format!(
                "port {} is not a port number (1 to 65535)",
                yaml_text(value)
            )
        })
}

/// The custom fields a data entry gives: one per plain value, nested
/// mappings as `key.child`.
fn custom_fields(key: &str, value: &Value, out: &mut Vec<CustomField>) {
    match value {
        Value::Null => {}
        Value::Tagged(tagged) => custom_fields(key, &tagged.value, out),
        Value::Mapping(mapping) => {
            for (child, value) in mapping {
                if let Some(child) = scalar_text(child) {
                    custom_fields(&format!("{key}.{child}"), value, out);
                }
            }
        }
        Value::Sequence(items) => {
            let plain: Option<Vec<String>> = items.iter().map(scalar_text).collect();
            let text = match plain {
                Some(values) => values.join(", "),
                None => serde_json::to_string(value).unwrap_or_else(|_| yaml_text(value)),
            };
            if !text.trim().is_empty() {
                out.push(CustomField::new(key, text));
            }
        }
        _ => {
            if let Some(text) = scalar_text(value).filter(|text| !text.trim().is_empty()) {
                out.push(CustomField::new(key, text));
            }
        }
    }
}
