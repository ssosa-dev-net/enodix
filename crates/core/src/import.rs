//! Importers: hosts from other sources into a workspace folder.
//!
//! [`csv`] reads spreadsheets. Every importer follows the same rules
//! (`docs/spec-v1.md`, "Import"): a preview before anything is written,
//! existing hosts are never overwritten without the user's confirmation,
//! passwords are never written to the workspace, and every import ends with
//! an [`ImportReport`].

use serde::Serialize;

use crate::workspace::HostId;

pub mod csv;

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

/// Whether a field called `name` holds a password or another secret, which
/// an import never writes to the workspace.
pub(crate) fn is_secret_name(name: &str) -> bool {
    let name = name.to_lowercase();
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().any(|word| {
        matches!(
            *word,
            "password" | "passwords" | "passwd" | "pwd" | "pass" | "secret" | "passphrase"
        ) || word.ends_with("password")
            || word.ends_with("passwd")
            || word.ends_with("secret")
    })
}
