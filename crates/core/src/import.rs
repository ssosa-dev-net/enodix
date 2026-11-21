//! Importers: hosts from other sources into a workspace folder.
//!
//! [`csv`] reads spreadsheets, [`nornir`] a Nornir simple inventory. Every
//! importer follows the same rules (`docs/spec-v1.md`, "Import"): a preview
//! before anything is written, existing hosts are never overwritten without
//! the user's confirmation, passwords are never written to the workspace,
//! and every import ends with an [`ImportReport`].

use std::collections::HashMap;

use serde::Serialize;

use crate::workspace::import::{HostWrite, ImportedValues, WriteOutcome};
use crate::workspace::{CustomField, HostId, Workspace};

pub mod csv;
pub mod nornir;

use csv::HostField;

/// What an import did, entry by entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// New hosts.
    pub created: Vec<ReportEntry>,
    /// Existing hosts the user confirmed to take the imported values.
    pub updated: Vec<ReportEntry>,
    /// Entries deliberately left out: existing hosts not confirmed, empty
    /// entries, passwords.
    pub skipped: Vec<ReportEntry>,
    /// Entries that could not be imported: bad or missing values, or the
    /// file system refused.
    pub failed: Vec<ReportEntry>,
    /// Folders the import created, as workspace paths, parents first.
    pub created_folders: Vec<String>,
    /// The hosts were written but the workspace could not be reloaded
    /// afterwards: why.
    pub reload_error: Option<String>,
}

/// One line of an [`ImportReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntry {
    /// Where the entry starts in the imported file (1-based); `None` for
    /// what concerns the whole file, such as a password column.
    pub line: Option<usize>,
    /// The host's name, or what the entry is about (a column's header).
    pub name: String,
    /// The host created or updated, or the existing one that was skipped.
    pub host: Option<HostId>,
    /// Why it was skipped or failed.
    pub reason: Option<String>,
}

/// What would become of one imported entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PreviewOutcome {
    /// A new host.
    Create { id: HostId },
    /// The host exists: skipped unless the user confirms that it takes the
    /// imported values.
    Exists { id: HostId },
    /// Nothing to import (a CSV import's empty row).
    Skip { reason: String },
    /// The entry cannot be imported.
    Fail { reason: String },
}

/// The values an entry gives a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedHost {
    pub name: String,
    pub address: String,
    /// The workspace folder it goes to.
    pub folder: String,
    /// The other built-in fields with a value.
    pub fields: Vec<FieldValue>,
    /// The port setting, if the entry gives one.
    pub port: Option<u16>,
    pub tags: Vec<String>,
    /// Custom fields with a value.
    pub custom: Vec<CustomField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldValue {
    pub field: HostField,
    pub value: String,
}

/// Why a password is not imported.
pub(crate) const PASSWORDS_SKIPPED: &str = "passwords are never written to the workspace";

/// Why a file to import could not be read, in the user's words: a file
/// that is not there says so and what to do, not the system's "os error 2"
/// (#116).
pub(crate) fn read_reason(error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "there is no such file; check the path".into()
    } else {
        error.to_string()
    }
}

/// Whether a field called `name` holds a password or another secret (a
/// token, a key, an SNMP community, credentials), which an import never
/// writes to the workspace. Judged by the name alone, erring on the side of
/// leaving a field out.
pub(crate) fn is_secret_name(name: &str) -> bool {
    let name = name.to_lowercase();
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().any(|word| {
        matches!(
            *word,
            "password"
                | "passwords"
                | "passwd"
                | "pwd"
                | "pass"
                | "secret"
                | "secrets"
                | "passphrase"
                | "token"
                | "tokens"
                | "key"
                | "keys"
                | "psk"
                | "community"
                | "communities"
                | "credential"
                | "credentials"
                | "bearer"
        ) || word.ends_with("password")
            || word.ends_with("passwd")
            || word.ends_with("secret")
            || word.ends_with("token")
            || word.ends_with("apikey")
            || word.ends_with("privatekey")
    })
}

/// The hosts of the folder at `folder` by lower-case name, each under the
/// name its file gives it and its file name: what a new host would clash
/// with.
pub(crate) fn existing_hosts(workspace: &Workspace, folder: &str) -> HashMap<String, HostId> {
    workspace
        .folder(folder)
        .map(|folder| {
            folder
                .hosts
                .iter()
                .flat_map(|host| {
                    let file = host.id.as_str().rsplit('/').next().unwrap_or_default();
                    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
                    [
                        (host.name.to_lowercase(), host.id.clone()),
                        (stem.to_lowercase(), host.id.clone()),
                    ]
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One previewed entry, as [`write_planned`] takes it.
pub(crate) struct Planned<'a> {
    pub line: Option<usize>,
    pub name: &'a str,
    pub outcome: &'a PreviewOutcome,
    pub host: Option<&'a ImportedHost>,
}

/// Writes the previewed entries (new hosts, and existing ones whose id is in
/// `replace`) and adds what became of each to `report`.
pub(crate) fn write_planned(
    workspace: &mut Workspace,
    planned: &[Planned<'_>],
    replace: &[HostId],
    report: &mut ImportReport,
) {
    let entry = |planned: &Planned<'_>, host: Option<HostId>, reason: Option<String>| ReportEntry {
        line: planned.line,
        name: planned.name.to_string(),
        host,
        reason,
    };

    let mut writes = Vec::new();
    for planned in planned {
        match (planned.outcome, planned.host) {
            (PreviewOutcome::Create { .. }, Some(host)) => writes.push(HostWrite::New {
                folder: host.folder.clone(),
                name: host.name.clone(),
                values: values(host),
            }),
            (PreviewOutcome::Exists { id }, Some(host)) if replace.contains(id) => {
                writes.push(HostWrite::Update {
                    id: id.clone(),
                    values: values(host),
                })
            }
            _ => {}
        }
    }
    let written = workspace.write_imported(writes);
    let mut outcomes = written.outcomes.into_iter();

    for planned in planned {
        match (planned.outcome, planned.host) {
            (PreviewOutcome::Create { .. }, Some(_)) => match outcomes.next() {
                Some(WriteOutcome::Created(id)) => {
                    report.created.push(entry(planned, Some(id), None))
                }
                Some(WriteOutcome::Exists(id)) => {
                    let reason = not_replaced(&id);
                    report.skipped.push(entry(planned, Some(id), Some(reason)));
                }
                Some(WriteOutcome::Failed(reason)) => {
                    report.failed.push(entry(planned, None, Some(reason)))
                }
                other => unreachable!("a new host's write gave {other:?}"),
            },
            (PreviewOutcome::Exists { id }, Some(_)) if replace.contains(id) => {
                match outcomes.next() {
                    Some(WriteOutcome::Updated(id)) => {
                        report.updated.push(entry(planned, Some(id), None))
                    }
                    Some(WriteOutcome::Failed(reason)) => {
                        report
                            .failed
                            .push(entry(planned, Some(id.clone()), Some(reason)))
                    }
                    other => unreachable!("an update gave {other:?}"),
                }
            }
            (PreviewOutcome::Create { .. }, None) => unreachable!("a new host without values"),
            (PreviewOutcome::Exists { id }, _) => {
                let reason = not_replaced(id);
                report
                    .skipped
                    .push(entry(planned, Some(id.clone()), Some(reason)));
            }
            (PreviewOutcome::Skip { reason }, _) => {
                report
                    .skipped
                    .push(entry(planned, None, Some(reason.clone())))
            }
            (PreviewOutcome::Fail { reason }, _) => {
                report
                    .failed
                    .push(entry(planned, None, Some(reason.clone())))
            }
        }
    }
    report.created_folders = written.created_folders;
    report.reload_error = written.reload_error;
}

fn not_replaced(id: &HostId) -> String {
    format!("{id} already exists; it is only replaced when you confirm it")
}

fn values(host: &ImportedHost) -> ImportedValues {
    ImportedValues {
        address: Some(host.address.clone()),
        text: host
            .fields
            .iter()
            .map(|field| (field.field.key(), field.value.clone()))
            .collect(),
        port: host.port,
        tags: (!host.tags.is_empty()).then(|| host.tags.clone()),
        custom: host
            .custom
            .iter()
            .map(|field| (field.key.clone(), field.value.clone()))
            .collect(),
    }
}
