//! CSV import: hosts from a spreadsheet.
//!
//! [`CsvTable::parse`] reads the file: the first row is the header; values
//! are separated by commas or semicolons (whichever the header uses more),
//! may be quoted (`"a, b"`, `""` for a quote, line breaks inside quotes);
//! a byte-order mark is dropped (UTF-8, or UTF-16 text), and a file that is
//! not UTF-8 is read as Windows-1252, as Excel saves "CSV (comma
//! delimited)" on a Western Windows.
//!
//! The user then maps each column ([`ColumnTarget`]; [`suggest_mapping`]
//! guesses from the headers) and picks a target folder: a [`CsvImport`].
//! [`CsvImport::preview`] shows what would happen to each row and writes
//! nothing; [`CsvImport::run`] writes the hosts and returns the
//! [`ImportReport`]. A host that exists already (same folder, same name
//! ignoring case and the file extension) is never written unless the user
//! confirmed it; confirmed, it takes the imported non-empty values (its tags
//! replaced by the row's, if the row has any) and keeps everything else. Columns that hold passwords (by their header or custom
//! key) are never written, whatever their mapping.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    existing_hosts, is_secret_name, write_planned, ImportReport, Planned, ReportEntry,
    PASSWORDS_SKIPPED,
};
pub use super::{FieldValue, ImportedHost, PreviewOutcome};
use crate::names;
use crate::workspace::bad_address;
use crate::workspace::import::check_import_name;
use crate::workspace::{join, HOST_EXTENSION};
use crate::workspace::{CustomField, HostId, Workspace};

/// What separates the values of a CSV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Separator {
    Comma,
    Semicolon,
}

impl Separator {
    fn char(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Semicolon => ';',
        }
    }
}

/// A CSV file as read: the header and the rows below it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvTable {
    pub separator: Separator,
    /// The first row, each value trimmed.
    pub headers: Vec<String>,
    /// The rows after the header; blank lines are left out.
    pub rows: Vec<CsvRow>,
}

/// One row of a [`CsvTable`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvRow {
    /// The line the row starts on (1-based; a quoted value may span lines).
    pub line: usize,
    /// The values as written (quotes removed), not trimmed. There may be
    /// fewer or more than there are headers.
    pub values: Vec<String>,
}

/// A file that cannot be read as CSV.
#[derive(Debug, thiserror::Error)]
pub enum CsvError {
    #[error("cannot read the file: {0}")]
    Io(String),
    #[error("the file is empty")]
    Empty,
    #[error("the file is not text")]
    NotText,
    #[error("the quote opened on line {line} is never closed")]
    UnclosedQuote { line: usize },
}

impl CsvTable {
    /// Reads the CSV file at `path`.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, CsvError> {
        let bytes =
            std::fs::read(path).map_err(|error| CsvError::Io(super::read_reason(&error)))?;
        Self::parse(&bytes)
    }

    /// Reads CSV text (see the module documentation for what is accepted).
    pub fn parse(bytes: &[u8]) -> Result<Self, CsvError> {
        let text = decode(bytes)?;
        let separator = detect_separator(&text);
        let mut records = records(&text, separator.char())?.into_iter();
        let (_, headers) = records.next().ok_or(CsvError::Empty)?;
        Ok(Self {
            separator,
            headers: headers.iter().map(|h| h.trim().to_string()).collect(),
            rows: records
                .map(|(line, values)| CsvRow { line, values })
                .collect(),
        })
    }
}

/// The text of a file: UTF-16 or UTF-8 by their byte-order mark, else UTF-8,
/// else Windows-1252.
fn decode(bytes: &[u8]) -> Result<String, CsvError> {
    let utf16 = |bytes: &[u8], from: fn([u8; 2]) -> u16| {
        if !bytes.len().is_multiple_of(2) {
            return Err(CsvError::NotText);
        }
        let units = bytes.chunks(2).map(|pair| from([pair[0], pair[1]]));
        char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| CsvError::NotText)
    };
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        utf16(rest, u16::from_le_bytes)?
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        utf16(rest, u16::from_be_bytes)?
    } else {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
        }
    };
    if text.contains('\0') {
        return Err(CsvError::NotText);
    }
    Ok(text)
}

/// The character a Windows-1252 byte stands for.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9F => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// Semicolons if the first record has more of them than commas (outside
/// quotes), else commas.
fn detect_separator(text: &str) -> Separator {
    let (mut commas, mut semicolons, mut quoted) = (0, 0, false);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => commas += 1,
            ';' if !quoted => semicolons += 1,
            '\n' | '\r' if !quoted => break,
            _ => {}
        }
    }
    if semicolons > commas {
        Separator::Semicolon
    } else {
        Separator::Comma
    }
}

/// The records of CSV text with the line each starts on; blank lines are
/// left out. Line breaks inside quoted values become `\n`.
fn records(text: &str, separator: char) -> Result<Vec<(usize, Vec<String>)>, CsvError> {
    let mut records = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut line = 1;
    let mut record_line = 1;
    // Nothing of the current field has been read yet (a quote opens it).
    let mut field_start = true;
    let mut quote_line = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quote_line.is_some() {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quote_line = None,
                '\r' | '\n' => {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field_start => {
                quote_line = Some(line);
                field_start = false;
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                fields.push(std::mem::take(&mut field));
                let record = std::mem::take(&mut fields);
                if !(record.len() == 1 && record[0].is_empty() && field_start) {
                    records.push((record_line, record));
                }
                line += 1;
                record_line = line;
                field_start = true;
            }
            _ if c == separator => {
                fields.push(std::mem::take(&mut field));
                field_start = true;
            }
            _ => {
                field.push(c);
                field_start = false;
            }
        }
    }
    if let Some(line) = quote_line {
        return Err(CsvError::UnclosedQuote { line });
    }
    if !fields.is_empty() || !field.is_empty() || !field_start {
        fields.push(field);
        records.push((record_line, fields));
    }
    Ok(records)
}

/// The built-in host fields a column can fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostField {
    /// The host's name, which names its file; the address when not mapped
    /// (or empty).
    Name,
    /// The management address: required.
    Address,
    Vendor,
    Platform,
    Model,
    SerialNumber,
    Site,
    Building,
    Room,
    Rack,
    RackUnit,
    /// A role id.
    Role,
    Description,
}

impl HostField {
    /// Every field, in the order of a host file.
    pub const ALL: [HostField; 13] = [
        Self::Name,
        Self::Address,
        Self::Vendor,
        Self::Platform,
        Self::Model,
        Self::SerialNumber,
        Self::Site,
        Self::Building,
        Self::Room,
        Self::Rack,
        Self::RackUnit,
        Self::Role,
        Self::Description,
    ];

    /// The field's key in a host file.
    pub fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Address => "address",
            Self::Vendor => "vendor",
            Self::Platform => "platform",
            Self::Model => "model",
            Self::SerialNumber => "serial_number",
            Self::Site => "site",
            Self::Building => "building",
            Self::Room => "room",
            Self::Rack => "rack",
            Self::RackUnit => "rack_unit",
            Self::Role => "role",
            Self::Description => "description",
        }
    }

    /// Headers (lower case, words separated by one space) that suggest the
    /// field.
    fn headers(self) -> &'static [&'static str] {
        match self {
            Self::Name => &[
                "name",
                "hostname",
                "host name",
                "host",
                "device",
                "device name",
            ],
            Self::Address => &[
                "address",
                "ip",
                "ip address",
                "ipv4",
                "ipv4 address",
                "management ip",
                "management address",
                "management ip address",
                "mgmt ip",
                "mgmt address",
                "mgmt ip address",
            ],
            Self::Vendor => &["vendor", "manufacturer", "make"],
            Self::Platform => &["platform", "os", "network os"],
            Self::Model => &["model", "hardware model"],
            Self::SerialNumber => &["serial", "serial number", "serial no", "sn", "s n"],
            Self::Site => &["site"],
            Self::Building => &["building"],
            Self::Room => &["room"],
            Self::Rack => &["rack"],
            Self::RackUnit => &["rack unit", "ru", "rack u", "rack position", "unit"],
            Self::Role => &["role", "device role"],
            Self::Description => &["description", "desc", "comment", "comments"],
        }
    }
}

impl fmt::Display for HostField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// What a column fills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ColumnTarget {
    /// A built-in field; each at most once.
    Field {
        field: HostField,
    },
    /// A custom field with this key; each key at most once (ignoring case).
    Custom {
        key: String,
    },
    /// Tags, separated by `,`, `;` or `|`; any number of columns.
    Tags,
    /// The folder below the target folder, `/` or `\` between names; at
    /// most one column. Missing folders are created.
    FolderPath,
    Ignore,
}

/// A mapping for `headers`: the built-in field a header names (first column
/// only), tags, folder path, nothing for passwords and empty headers, else a
/// custom field named after the header.
pub fn suggest_mapping(headers: &[String]) -> Vec<ColumnTarget> {
    let mut taken = Vec::new();
    let mut folder_taken = false;
    headers
        .iter()
        .map(|header| {
            let words: Vec<String> = header
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect();
            let normal = words.join(" ");
            if normal.is_empty() || is_secret_name(header) {
                return ColumnTarget::Ignore;
            }
            if matches!(normal.as_str(), "tags" | "tag" | "labels") {
                return ColumnTarget::Tags;
            }
            if matches!(
                normal.as_str(),
                "folder" | "folder path" | "path" | "directory"
            ) {
                if !folder_taken {
                    folder_taken = true;
                    return ColumnTarget::FolderPath;
                }
            } else if let Some(field) = HostField::ALL
                .into_iter()
                .find(|field| !taken.contains(field) && field.headers().contains(&normal.as_str()))
            {
                taken.push(field);
                return ColumnTarget::Field { field };
            }
            ColumnTarget::Custom {
                key: header.trim().to_string(),
            }
        })
        .collect()
}

/// A mapping that cannot be imported with.
#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("the file has {columns} columns but {mapping} are mapped")]
    ColumnCount { columns: usize, mapping: usize },
    /// No column fills the address. `first_row_is_a_device`: a column is
    /// named by an IP address, so the file most likely has no row of
    /// column names (a first export's mistake, #116).
    #[error("{}", if *.first_row_is_a_device {
        "no column fills the management address, and the first row looks like a device: \
         the first row must name the columns, so add one such as name,address at the top of the file"
    } else {
        "no column fills the management address: choose Management address for the column that holds it"
    })]
    NoAddress { first_row_is_a_device: bool },
    #[error("more than one column is mapped to {0}")]
    FieldTwice(HostField),
    #[error("more than one column is mapped to the folder path")]
    FolderPathTwice,
    #[error("more than one column is mapped to the custom field `{0}`")]
    CustomTwice(String),
    #[error("a column is mapped to a custom field without a name")]
    CustomKeyEmpty,
    #[error("the target folder {0} is not in this workspace")]
    NoSuchFolder(String),
}

/// A CSV file with its mapping and the folder its hosts go to.
#[derive(Debug, Clone)]
pub struct CsvImport {
    pub table: CsvTable,
    /// One target per column of the table.
    pub mapping: Vec<ColumnTarget>,
    /// The workspace folder the hosts (and the folder path column's folders)
    /// go to; empty for the root.
    pub target: String,
}

/// What [`CsvImport::run`] would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    /// One per row of the file.
    pub rows: Vec<PreviewRow>,
    /// The folders it would create, parents first.
    pub new_folders: Vec<String>,
    /// Columns whose values would not be written (passwords).
    pub skipped_columns: Vec<ReportEntry>,
}

/// What would become of one row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub line: usize,
    /// The host's name (empty if the row gives none).
    pub name: String,
    pub outcome: PreviewOutcome,
    /// The values the host would get; `None` when the row fails or is empty.
    pub host: Option<ImportedHost>,
}

const EMPTY_ROW: &str = "the row has nothing to import";

impl CsvImport {
    /// What importing would do, row by row. Writes nothing.
    pub fn preview(&self, workspace: &Workspace) -> Result<ImportPreview, MappingError> {
        Planner::new(self, workspace)?.plan()
    }

    /// Imports the rows. Hosts that exist already are skipped unless their
    /// id is in `replace` (the user confirmed them); then they take the
    /// row's non-empty values and keep the rest. The workspace is reloaded
    /// once at the end, if anything was written.
    pub fn run(
        &self,
        workspace: &mut Workspace,
        replace: &[HostId],
    ) -> Result<ImportReport, MappingError> {
        let preview = self.preview(workspace)?;
        let mut report = ImportReport {
            skipped: preview.skipped_columns.clone(),
            ..ImportReport::default()
        };
        let planned: Vec<Planned<'_>> = preview
            .rows
            .iter()
            .map(|row| Planned {
                line: Some(row.line),
                name: &row.name,
                outcome: &row.outcome,
                host: row.host.as_ref(),
            })
            .collect();
        write_planned(workspace, &planned, replace, &mut report);
        Ok(report)
    }
}

/// Works out what becomes of each row.
struct Planner<'a> {
    import: &'a CsvImport,
    workspace: &'a Workspace,
    /// Columns never written: passwords.
    secret: Vec<bool>,
    /// New folders by lower-case path → the path as first written.
    new_folders: HashMap<String, String>,
    new_folder_order: Vec<String>,
    /// Hosts already planned by lower-case `folder/name` → their line.
    planned: HashMap<String, usize>,
    /// Existing hosts by folder path, then lower-case file stem.
    existing: HashMap<String, HashMap<String, HostId>>,
}

impl<'a> Planner<'a> {
    fn new(import: &'a CsvImport, workspace: &'a Workspace) -> Result<Self, MappingError> {
        let headers = &import.table.headers;
        let mapping = &import.mapping;
        if mapping.len() != headers.len() {
            return Err(MappingError::ColumnCount {
                columns: headers.len(),
                mapping: mapping.len(),
            });
        }
        let mut fields = Vec::new();
        let mut custom: Vec<String> = Vec::new();
        let mut folder_path = false;
        for target in mapping {
            match target {
                ColumnTarget::Field { field } if fields.contains(field) => {
                    return Err(MappingError::FieldTwice(*field))
                }
                ColumnTarget::Field { field } => fields.push(*field),
                ColumnTarget::Custom { key } => {
                    let key = key.trim().to_lowercase();
                    if key.is_empty() {
                        return Err(MappingError::CustomKeyEmpty);
                    }
                    if custom.contains(&key) {
                        return Err(MappingError::CustomTwice(key));
                    }
                    custom.push(key);
                }
                ColumnTarget::FolderPath if folder_path => {
                    return Err(MappingError::FolderPathTwice)
                }
                ColumnTarget::FolderPath => folder_path = true,
                ColumnTarget::Tags | ColumnTarget::Ignore => {}
            }
        }
        if !fields.contains(&HostField::Address) {
            return Err(MappingError::NoAddress {
                first_row_is_a_device: headers
                    .iter()
                    .any(|header| header.parse::<std::net::IpAddr>().is_ok()),
            });
        }
        if workspace.folder(&import.target).is_none() {
            return Err(MappingError::NoSuchFolder(import.target.clone()));
        }
        // Ignored or not, the report says passwords were left out.
        let secret = headers
            .iter()
            .zip(mapping)
            .map(|(header, target)| {
                is_secret_name(header)
                    || matches!(target, ColumnTarget::Custom { key } if is_secret_name(key))
            })
            .collect();
        Ok(Self {
            import,
            workspace,
            secret,
            new_folders: HashMap::new(),
            new_folder_order: Vec::new(),
            planned: HashMap::new(),
            existing: HashMap::new(),
        })
    }

    fn plan(mut self) -> Result<ImportPreview, MappingError> {
        let import = self.import;
        let table = &import.table;
        let skipped_columns = table
            .headers
            .iter()
            .enumerate()
            .filter(|(column, _)| self.secret[*column])
            .filter(|(column, _)| {
                table.rows.iter().any(|row| {
                    row.values
                        .get(*column)
                        .is_some_and(|v| !v.trim().is_empty())
                })
            })
            .map(|(_, header)| ReportEntry {
                line: None,
                name: header.clone(),
                host: None,
                reason: Some(PASSWORDS_SKIPPED.into()),
            })
            .collect();
        let rows = table.rows.iter().map(|row| self.plan_row(row)).collect();
        Ok(ImportPreview {
            rows,
            new_folders: self.new_folder_order,
            skipped_columns,
        })
    }

    fn plan_row(&mut self, row: &CsvRow) -> PreviewRow {
        let line = row.line;
        let columns = self.import.table.headers.len();
        let cell = |column: usize| row.values.get(column).map_or("", |v| v.trim());
        let preview = |name: String, outcome, host| PreviewRow {
            line,
            name,
            outcome,
            host,
        };
        let fail =
            |name: String, reason: String| preview(name, PreviewOutcome::Fail { reason }, None);

        let mut name = String::new();
        let mut address = String::new();
        let mut fields = Vec::new();
        let mut tags = Vec::new();
        let mut custom = Vec::new();
        let mut folder_path = String::new();
        let mut empty = true;
        for (column, target) in self.import.mapping.iter().enumerate() {
            let value = cell(column);
            if self.secret[column] || value.is_empty() {
                continue;
            }
            match target {
                ColumnTarget::Ignore => continue,
                ColumnTarget::Field {
                    field: HostField::Name,
                } => name = value.to_string(),
                ColumnTarget::Field {
                    field: HostField::Address,
                } => address = value.to_string(),
                ColumnTarget::Field { field } => fields.push(FieldValue {
                    field: *field,
                    value: value.to_string(),
                }),
                ColumnTarget::Custom { key } => custom.push(CustomField::new(key.trim(), value)),
                ColumnTarget::Tags => tags.extend(
                    value
                        .split([',', ';', '|'])
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string),
                ),
                ColumnTarget::FolderPath => folder_path = value.to_string(),
            }
            empty = false;
        }
        if name.is_empty() {
            name = address.clone();
        }

        let extra = row.values.iter().skip(columns);
        if extra.clone().any(|value| !value.trim().is_empty()) {
            let count = columns + extra.count();
            return fail(
                name,
                format!("the row has {count} values; the header has {columns}"),
            );
        }
        if empty {
            return preview(
                name,
                PreviewOutcome::Skip {
                    reason: EMPTY_ROW.into(),
                },
                None,
            );
        }
        if address.is_empty() {
            return fail(name, "the row has no management address".into());
        }
        // As New host checks it (#131). There is no port column to take a
        // port into.
        if let Some(bad) = bad_address(&address) {
            let reason = bad.reason(&address, |port| {
                format!(
                    "; a CSV import has no port column, so set Port to {port} in the host's \
                     details or on its folder"
                )
            });
            return fail(name, reason);
        }
        let host_name = match check_import_name(&name) {
            Ok(checked) => checked.to_string(),
            Err(reason) => return fail(name, reason),
        };
        let folder = match self.folder(&folder_path) {
            Ok(folder) => folder,
            Err(reason) => return fail(name, format!("folder path {folder_path}: {reason}")),
        };
        let key = format!("{}/{}", names::folded(&folder), names::folded(&host_name));
        if let Some(first) = self.planned.get(&key) {
            return fail(
                name,
                format!("row {first} already imports {host_name} into this folder"),
            );
        }
        self.planned.insert(key, line);

        let host = ImportedHost {
            name: host_name.clone(),
            address,
            folder: folder.clone(),
            fields,
            port: None,
            tags,
            custom,
        };
        let outcome = match self.existing_host(&folder, &host_name) {
            Some(id) => PreviewOutcome::Exists { id },
            None => {
                self.note_new_folders(&folder);
                PreviewOutcome::Create {
                    id: HostId::new(join(&folder, &format!("{host_name}.{HOST_EXTENSION}"))),
                }
            }
        };
        preview(host_name, outcome, Some(host))
    }

    /// The workspace path of the folder `path` names below the target: each
    /// name matched to an existing folder ignoring case, or to a new folder
    /// another row names.
    fn folder(&self, path: &str) -> Result<String, String> {
        let mut current = self.import.target.clone();
        let mut existing = self.workspace.folder(&current);
        for name in path
            .split(['/', '\\'])
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            let name = check_import_name(name)?;
            let child = existing.and_then(|folder| {
                folder
                    .folders
                    .iter()
                    .find(|child| names::same_name(&child.name, name))
            });
            current = match child {
                Some(child) => child.path.clone(),
                None => {
                    let path = join(&current, name);
                    self.new_folders
                        .get(&names::folded(&path))
                        .cloned()
                        .unwrap_or(path)
                }
            };
            existing = child;
        }
        Ok(current)
    }

    /// Records the folders of `folder` that do not exist yet.
    fn note_new_folders(&mut self, folder: &str) {
        let mut current = String::new();
        for name in folder.split('/').filter(|n| !n.is_empty()) {
            current = join(&current, name);
            if self.workspace.folder(&current).is_none()
                && !self.new_folders.contains_key(&names::folded(&current))
            {
                self.new_folders
                    .insert(names::folded(&current), current.clone());
                self.new_folder_order.push(current.clone());
            }
        }
    }

    /// The host of the folder at `folder` that a new host called `name`
    /// would clash with: by file name, or by the name in its file.
    fn existing_host(&mut self, folder: &str, name: &str) -> Option<HostId> {
        let workspace = self.workspace;
        let hosts = self
            .existing
            .entry(folder.to_string())
            .or_insert_with(|| existing_hosts(workspace, folder));
        hosts.get(&names::folded(name)).cloned()
    }
}
