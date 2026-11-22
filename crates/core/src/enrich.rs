//! Host enrichment: vendor, model, serial number and OS version filled in
//! from the devices themselves.
//!
//! A [`crate::job::JobStep::Enrich`] job runs the script the app ships in
//! its Python package (`python -m enodix.enrich`, [`ENRICH_MODULE`]) as a
//! script job. Pre-flight keeps the hosts it can reach: SSH, a platform
//! definition with a NAPALM driver ([`napalm_driver`]), a stored credential
//! profile, no jump host. The script asks NAPALM for each host's facts
//! with the host's own login and writes them to the file
//! [`FACTS_VARIABLE`] names ([`FACTS_FORMAT`]); the job reads that file
//! when the script ends and keeps the [`CollectedFacts`] with the run.
//!
//! Nothing is written until the user applies a [`preview`]: per host, what
//! the device reports that the host does not have yet ([`ChangeKind::New`])
//! and what would replace a typed value ([`ChangeKind::Replace`]). [`apply`]
//! fills every empty field, replaces typed values only on the hosts the
//! user ticked, and reports each host as filled, unchanged, skipped or
//! failed. Vendor, model and serial number are the host's built-in fields;
//! the OS version goes to the custom field [`OS_VERSION_FIELD`].

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::job::results::{RunSkipped, StoredRun};
use crate::job::JobStep;
use crate::platform::PlatformDefinition;
use crate::workspace::import::{HostWrite, ImportedValues, WriteOutcome};
use crate::workspace::{Host, HostId, Workspace};

/// The Python module the app ships that fetches the facts.
pub const ENRICH_MODULE: &str = "enodix.enrich";
/// Names the file the script writes the facts to.
pub const FACTS_VARIABLE: &str = "ENODIX_FACTS_FILE";
/// The version of the facts file's format.
pub const FACTS_FORMAT: u32 = 1;
/// The custom field the OS version goes to.
pub const OS_VERSION_FIELD: &str = "os_version";
/// What a step of this kind is called, for the user.
pub const ENRICH_TITLE: &str = "Fill in from devices";

/// NAPALM's own drivers, in the order they are preferred when a platform
/// definition answers to several (`nxos_ssh` before `nxos`, which needs
/// NX-API).
pub const NAPALM_DRIVERS: &[&str] = &[
    "ios",
    "eos",
    "junos",
    "nxos_ssh",
    "nxos",
    "iosxr",
    "iosxr_netconf",
];

/// The NAPALM driver for hosts of `definition`: the first of
/// [`NAPALM_DRIVERS`] that the definition is called or answers to (ignoring
/// case). `None`: NAPALM cannot fetch its facts.
pub fn napalm_driver(definition: &PlatformDefinition) -> Option<&'static str> {
    NAPALM_DRIVERS
        .iter()
        .copied()
        .find(|driver| definition.is_named(driver) || definition.answers_to(driver))
}

/// One of the fields enrichment fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrichField {
    Vendor,
    Model,
    SerialNumber,
    /// The custom field [`OS_VERSION_FIELD`].
    OsVersion,
}

impl EnrichField {
    pub const ALL: [EnrichField; 4] = [
        EnrichField::Vendor,
        EnrichField::Model,
        EnrichField::SerialNumber,
        EnrichField::OsVersion,
    ];

    /// The key in the host file (for the OS version: under `custom`).
    pub fn key(self) -> &'static str {
        match self {
            EnrichField::Vendor => "vendor",
            EnrichField::Model => "model",
            EnrichField::SerialNumber => "serial_number",
            EnrichField::OsVersion => OS_VERSION_FIELD,
        }
    }

    /// The host's value of this field (`None` if blank).
    fn of(self, host: &Host) -> Option<&str> {
        let value = match self {
            EnrichField::Vendor => host.vendor.as_deref(),
            EnrichField::Model => host.model.as_deref(),
            EnrichField::SerialNumber => host.serial_number.as_deref(),
            EnrichField::OsVersion => host
                .custom
                .iter()
                .find(|field| field.key == OS_VERSION_FIELD)
                .map(|field| field.value.as_str()),
        };
        value.filter(|value| !value.trim().is_empty())
    }
}

/// What a device reported about itself (NAPALM's `get_facts`). Blank
/// values count as not reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

impl DeviceFacts {
    /// Trims each value and drops blank ones.
    fn normalise(&mut self) {
        for value in [
            &mut self.vendor,
            &mut self.model,
            &mut self.serial_number,
            &mut self.os_version,
        ] {
            *value = value
                .take()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty());
        }
    }

    /// The reported value of `field`, trimmed; `None` if blank.
    pub fn get(&self, field: EnrichField) -> Option<&str> {
        let value = match field {
            EnrichField::Vendor => &self.vendor,
            EnrichField::Model => &self.model,
            EnrichField::SerialNumber => &self.serial_number,
            EnrichField::OsVersion => &self.os_version,
        };
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

/// What the script reported for the job's hosts, in plan order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectedFacts {
    pub hosts: Vec<CollectedHost>,
}

/// What the script reported for one host: its facts, or why it has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectedHost {
    pub id: HostId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<DeviceFacts>,
    /// Why the facts could not be fetched (the device could not be
    /// reached, the login was refused, NAPALM is missing, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The file the script writes.
#[derive(Debug, Deserialize)]
struct FactsFile {
    format: u32,
    #[serde(default)]
    hosts: Vec<CollectedHost>,
}

impl CollectedFacts {
    /// The facts the script wrote to `path` for `hosts` (the job's, in
    /// plan order): each host's entry, or an error saying why there is none
    /// (`missing` when the file holds none for it). A file that is missing
    /// or cannot be read gives every host that reason.
    pub fn read(path: &Path, hosts: &[HostId], missing: &str) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Self::failed(hosts, missing)
            }
            Err(error) => {
                return Self::failed(hosts, &format!("the facts could not be read: {error}"))
            }
        };
        Self::parse(&text, hosts, missing)
    }

    /// As [`CollectedFacts::read`], from the file's text.
    pub fn parse(text: &str, hosts: &[HostId], missing: &str) -> Self {
        let file: FactsFile = match serde_json::from_str(text) {
            Ok(file) => file,
            Err(error) => {
                return Self::failed(hosts, &format!("the script wrote damaged facts: {error}"))
            }
        };
        if file.format != FACTS_FORMAT {
            return Self::failed(
                hosts,
                &format!(
                    "the script wrote facts in format {}, and this version reads format {FACTS_FORMAT}",
                    file.format
                ),
            );
        }
        let mut reported: BTreeMap<HostId, CollectedHost> = BTreeMap::new();
        for mut host in file.hosts {
            if let Some(facts) = &mut host.facts {
                facts.normalise();
            }
            reported.entry(host.id.clone()).or_insert(host);
        }
        Self {
            hosts: hosts
                .iter()
                .map(|id| match reported.remove(id) {
                    Some(host) if host.facts.is_some() || host.error.is_some() => host,
                    _ => CollectedHost {
                        id: id.clone(),
                        facts: None,
                        error: Some(missing.to_string()),
                    },
                })
                .collect(),
        }
    }

    fn failed(hosts: &[HostId], reason: &str) -> Self {
        Self {
            hosts: hosts
                .iter()
                .map(|id| CollectedHost {
                    id: id.clone(),
                    facts: None,
                    error: Some(reason.to_string()),
                })
                .collect(),
        }
    }
}

/// The preview of filling in a run's facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrichPreview {
    /// The hosts the script ran for, in plan order, then the hosts
    /// pre-flight skipped.
    pub hosts: Vec<PreviewHost>,
}

/// What filling in would do to one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewHost {
    pub id: HostId,
    pub name: String,
    pub outcome: PreviewOutcome,
    /// The fields whose value would change: new ones and replacements.
    /// Fields the device reports as the host has them are left out.
    pub changes: Vec<FieldChange>,
    /// The facts reported (kept so that [`apply`] can compare them with the
    /// host as it is then).
    #[serde(skip)]
    facts: Option<DeviceFacts>,
}

impl PreviewHost {
    /// Whether it would replace a typed value (and so needs ticking).
    pub fn replaces(&self) -> bool {
        self.changes
            .iter()
            .any(|change| change.kind == ChangeKind::Replace)
    }
}

/// The kind of a [`PreviewHost`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PreviewOutcome {
    /// The device reported values that differ from the host's.
    Changes,
    /// The device reported nothing the host does not have.
    Unchanged,
    /// Pre-flight left it out.
    Skipped { reason: String },
    /// Its facts could not be fetched.
    Failed { reason: String },
}

/// One field a host would get.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldChange {
    pub field: EnrichField,
    pub kind: ChangeKind,
    /// The typed value (`None`: the field is empty).
    pub current: Option<String>,
    /// What the device reports.
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// The field is empty: filled when applied.
    New,
    /// The field has another typed value: replaced only if the host is
    /// ticked.
    Replace,
}

/// Why a run's facts cannot be previewed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnrichError {
    #[error("the job run {0} did not fill in from devices")]
    NotEnrichment(String),
    #[error("the job run {0} has no facts: it did not finish")]
    NoFacts(String),
}

/// What filling in `facts` would change, compared with the hosts as they
/// are in `workspace`; `skipped` (pre-flight's) are listed after them.
/// Writes nothing.
pub fn preview(
    workspace: &Workspace,
    facts: &CollectedFacts,
    skipped: &[RunSkipped],
) -> EnrichPreview {
    let mut hosts: Vec<PreviewHost> = facts
        .hosts
        .iter()
        .map(|collected| preview_host(workspace, &collected.id, collected))
        .collect();
    hosts.extend(skipped.iter().map(|skipped| PreviewHost {
        id: skipped.id.clone(),
        name: skipped.name.clone(),
        outcome: PreviewOutcome::Skipped {
            reason: skipped.reason.clone(),
        },
        changes: Vec::new(),
        facts: None,
    }));
    EnrichPreview { hosts }
}

/// [`preview`] of a kept run of a [`JobStep::Enrich`] job.
pub fn preview_run(workspace: &Workspace, run: &StoredRun) -> Result<EnrichPreview, EnrichError> {
    if run.step != JobStep::Enrich {
        return Err(EnrichError::NotEnrichment(run.id.clone()));
    }
    let facts = run
        .facts
        .as_ref()
        .ok_or_else(|| EnrichError::NoFacts(run.id.clone()))?;
    Ok(preview(workspace, facts, &run.skipped))
}

const GONE: &str = "it is no longer in the workspace";

fn preview_host(workspace: &Workspace, id: &HostId, collected: &CollectedHost) -> PreviewHost {
    let host = workspace.host(id);
    let name = host.map_or_else(|| id.to_string(), |host| host.name.clone());
    let failed = |reason: String| PreviewHost {
        id: id.clone(),
        name: name.clone(),
        outcome: PreviewOutcome::Failed { reason },
        changes: Vec::new(),
        facts: collected.facts.clone(),
    };
    let Some(host) = host else {
        return failed(GONE.into());
    };
    let Some(facts) = &collected.facts else {
        return failed(
            collected
                .error
                .clone()
                .unwrap_or_else(|| "the script reported nothing for it".into()),
        );
    };
    let changes = changes(host, facts);
    PreviewHost {
        id: id.clone(),
        name,
        outcome: if changes.is_empty() {
            PreviewOutcome::Unchanged
        } else {
            PreviewOutcome::Changes
        },
        changes,
        facts: Some(facts.clone()),
    }
}

/// The fields of `host` that `facts` would change.
fn changes(host: &Host, facts: &DeviceFacts) -> Vec<FieldChange> {
    EnrichField::ALL
        .into_iter()
        .filter_map(|field| {
            let value = facts.get(field)?;
            let current = field.of(host);
            if current.map(str::trim) == Some(value) {
                return None;
            }
            Some(FieldChange {
                field,
                kind: if current.is_some() {
                    ChangeKind::Replace
                } else {
                    ChangeKind::New
                },
                current: current.map(str::to_string),
                value: value.to_string(),
            })
        })
        .collect()
}

/// What applying a preview did, host by host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichReport {
    /// Hosts that got at least one value.
    pub filled: Vec<ReportedHost>,
    /// Hosts written nothing: the device reported what they have, or only
    /// values that would replace typed ones and the host was not ticked.
    pub unchanged: Vec<ReportedHost>,
    /// Hosts pre-flight left out.
    pub skipped: Vec<ReportedHost>,
    /// Hosts whose facts could not be fetched or written.
    pub failed: Vec<ReportedHost>,
    /// The hosts were written but the workspace could not be reloaded
    /// afterwards: why.
    pub reload_error: Option<String>,
}

/// One host of an [`EnrichReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedHost {
    pub id: HostId,
    pub name: String,
    /// The fields written (filled or replaced).
    pub fields: Vec<EnrichField>,
    /// The fields whose typed value was kept, though the device reports
    /// another (the host was not ticked).
    pub kept: Vec<EnrichField>,
    /// Why it was skipped, failed or left unchanged.
    pub reason: Option<String>,
}

/// Fills in what `preview` showed: every empty field of every host, and
/// typed values only on the hosts in `replace`. Only the fields the
/// preview listed are written, each compared again with the host as it is
/// now (a value typed since the preview is not overwritten unless the host
/// is ticked; one that now equals the device's is left alone); the
/// workspace reloads once at the end.
pub fn apply(
    workspace: &mut Workspace,
    preview: &EnrichPreview,
    replace: &[HostId],
) -> EnrichReport {
    let mut report = EnrichReport::default();
    let reported = |host: &PreviewHost, fields, kept, reason: Option<String>| ReportedHost {
        id: host.id.clone(),
        name: host.name.clone(),
        fields,
        kept,
        reason,
    };

    let mut writes = Vec::new();
    // (preview host, fields written, fields kept), for the hosts written.
    let mut written = Vec::new();
    for host in &preview.hosts {
        match &host.outcome {
            PreviewOutcome::Skipped { reason } => {
                report
                    .skipped
                    .push(reported(host, Vec::new(), Vec::new(), Some(reason.clone())));
                continue;
            }
            PreviewOutcome::Failed { reason } => {
                report
                    .failed
                    .push(reported(host, Vec::new(), Vec::new(), Some(reason.clone())));
                continue;
            }
            PreviewOutcome::Changes | PreviewOutcome::Unchanged => {}
        }
        let (Some(current), Some(facts)) = (workspace.host(&host.id), &host.facts) else {
            report
                .failed
                .push(reported(host, Vec::new(), Vec::new(), Some(GONE.into())));
            continue;
        };
        let ticked = replace.contains(&host.id);
        let mut values = ImportedValues::default();
        let mut fields = Vec::new();
        let mut kept = Vec::new();
        let shown: Vec<EnrichField> = host.changes.iter().map(|c| c.field).collect();
        for change in changes(current, facts) {
            if !shown.contains(&change.field) {
                continue;
            }
            if change.kind == ChangeKind::Replace && !ticked {
                kept.push(change.field);
                continue;
            }
            fields.push(change.field);
            match change.field {
                EnrichField::OsVersion => {
                    values.custom.push((OS_VERSION_FIELD.into(), change.value))
                }
                field => values.text.push((field.key(), change.value)),
            }
        }
        if fields.is_empty() {
            let reason = if kept.is_empty() {
                "the device reports what it already has"
            } else {
                "it keeps its typed values, as it was not ticked to replace them"
            };
            report
                .unchanged
                .push(reported(host, fields, kept, Some(reason.into())));
            continue;
        }
        writes.push(HostWrite::Update {
            id: host.id.clone(),
            values,
        });
        written.push((host, fields, kept));
    }

    let outcome = workspace.write_imported(writes);
    for ((host, fields, kept), outcome) in written.into_iter().zip(outcome.outcomes) {
        match outcome {
            WriteOutcome::Updated(_) => report.filled.push(reported(host, fields, kept, None)),
            WriteOutcome::Failed(reason) => {
                report
                    .failed
                    .push(reported(host, Vec::new(), Vec::new(), Some(reason)))
            }
            other => unreachable!("an update gave {other:?}"),
        }
    }
    report.reload_error = outcome.reload_error;
    report
}
