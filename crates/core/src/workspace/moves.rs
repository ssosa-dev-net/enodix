//! What renaming or moving a host or a folder changes besides the files
//! moved: the references to the hosts concerned. A host's id is its file's
//! path, and other files name hosts by id: the `jump_host` setting (in host
//! files and folder settings files) and the saved jobs' `host` (and, for a
//! folder, `folder`) targets. Those that named a moved host or folder are
//! rewritten to the new path; only files that held such a reference are
//! written, and in each only that value changes (its line keeps its
//! indentation, quoting style aside, and comment; when the file is laid out
//! so that the line cannot be found, the whole file is written again from
//! its content, losing comments).
//!
//! What the app keeps outside the workspace about a host (its session logs
//! and config history) follows with [`crate::history::carry`], given the
//! moves of [`Moved::hosts`].

use std::fs;

use serde::Serialize;
use serde_norway::{Mapping, Value};

use super::jobs::{self, JOBS_FILE};
use super::settings::JUMP_HOST;
use super::{
    join, read_mapping, scalar_text, write_text, Folder, HostId, Problem, Workspace,
    FOLDER_SETTINGS_FILE,
};

/// A host whose id changed: it was renamed or moved, or a folder above it
/// was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostMove {
    pub from: HostId,
    pub to: HostId,
}

/// What renaming or moving a host or a folder did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved<T> {
    /// The new host id or folder path.
    pub to: T,
    /// Every host whose id changed, in tree order; empty when nothing
    /// moved (the same name or folder as before).
    pub hosts: Vec<HostMove>,
    /// The files rewritten because they named a moved host or folder, as
    /// paths relative to the workspace root (where they are now).
    pub rewritten: Vec<String>,
    /// The files that named one but could not be rewritten, with why: they
    /// still name the old path.
    pub not_rewritten: Vec<Problem>,
}

impl<T> Moved<T> {
    pub(super) fn nothing(to: T) -> Self {
        Self {
            to,
            hosts: Vec::new(),
            rewritten: Vec::new(),
            not_rewritten: Vec::new(),
        }
    }
}

/// The moves of every host under `folder` (by its old path `from`) when the
/// folder becomes `to`.
pub(super) fn folder_moves(folder: &Folder, from: &str, to: &str) -> Vec<HostMove> {
    fn collect(folder: &Folder, from: &str, to: &str, moves: &mut Vec<HostMove>) {
        for host in &folder.hosts {
            let rest = &host.id.as_str()[from.len()..];
            moves.push(HostMove {
                from: host.id.clone(),
                to: HostId::new(format!("{to}{rest}")),
            });
        }
        for child in &folder.folders {
            collect(child, from, to, moves);
        }
    }
    let mut moves = Vec::new();
    collect(folder, from, to, &mut moves);
    moves
}

/// A folder path that moved, for the saved jobs that target a folder.
pub(super) struct FolderMove<'a> {
    pub from: &'a str,
    pub to: &'a str,
}

impl FolderMove<'_> {
    /// The new path of `path`, if it is the moved folder or one below it.
    fn moved(&self, path: &str) -> Option<String> {
        let path = path.trim().trim_matches('/');
        if path == self.from {
            Some(self.to.to_string())
        } else {
            path.strip_prefix(self.from)
                .and_then(|rest| rest.strip_prefix('/'))
                .map(|rest| join(self.to, rest))
        }
    }
}

/// The new id of the host `reference` names, if it is one of `moves`.
/// References match host ids ignoring case, Unicode form and surrounding
/// blanks, as the jump hosts do.
fn moved_host(moves: &[HostMove], reference: &str) -> Option<String> {
    let reference = crate::names::folded(&reference.trim().replace('\\', "/"));
    moves
        .iter()
        .find(|moved| crate::names::folded(moved.from.as_str()) == reference)
        .map(|moved| moved.to.as_str().to_string())
}

impl Workspace {
    /// Rewrites the references to the moved hosts (and folder) in the
    /// files of the workspace as it is now loaded (after the move), into
    /// `moved`; reloads if it wrote anything.
    pub(super) fn rewrite_references<T>(
        &mut self,
        moved: &mut Moved<T>,
        folder: Option<FolderMove<'_>>,
    ) {
        if moved.hosts.is_empty() && folder.is_none() {
            return;
        }
        let moves = moved.hosts.clone();
        let jump = |value: &str| moved_host(&moves, value);
        let mut files = Vec::new();
        collect_jump_files(&self.tree, &jump, &mut files);
        for file in files {
            self.rewrite_file(&file, &mut *moved, |document| {
                let changed = document
                    .get(JUMP_HOST)
                    .and_then(scalar_text)
                    .and_then(|v| jump(&v));
                if let Some(to) = changed.clone() {
                    document.insert(JUMP_HOST.into(), to.into());
                }
                changed.is_some()
            });
        }
        if self.root.join(JOBS_FILE).is_file() {
            self.rewrite_file(JOBS_FILE, moved, |document| {
                jobs::retarget(document, jump, |path| {
                    folder.as_ref().and_then(|folder| folder.moved(path))
                })
            });
        }
        if !moved.rewritten.is_empty() {
            if let Err(error) = self.reload() {
                moved.not_rewritten.push(Problem {
                    file: ".".into(),
                    message: format!("the workspace could not be read again: {error}"),
                });
            }
        }
    }

    /// Rewrites `file` (relative to the root) as `change` changes its
    /// content, keeping its text but for the values changed when it can.
    fn rewrite_file<T>(
        &self,
        file: &str,
        moved: &mut Moved<T>,
        change: impl FnOnce(&mut Mapping) -> bool,
    ) {
        let path = self.root.join(file);
        let failed = |moved: &mut Moved<T>, message: String| {
            moved.not_rewritten.push(Problem {
                file: file.to_string(),
                message,
            });
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return failed(moved, format!("cannot read file: {error}")),
        };
        let original = match read_mapping(&path) {
            Ok(document) => document,
            Err(message) => return failed(moved, message),
        };
        let mut expected = original.clone();
        if !change(&mut expected) {
            return;
        }
        let rewritten = rewrite_text(&text, &original, &expected);
        let written = match rewritten {
            Some(text) => write_text(&self.root, file, &text),
            None => super::write_mapping(&self.root, file, &expected),
        };
        match written {
            Ok(()) => moved.rewritten.push(file.to_string()),
            Err(error) => failed(moved, error.to_string()),
        }
    }
}

/// The host files and folder settings files (relative paths) whose own
/// `jump_host` names a host `jump` gives a new id for, in tree order.
fn collect_jump_files(
    folder: &Folder,
    jump: &dyn Fn(&str) -> Option<String>,
    files: &mut Vec<String>,
) {
    let names_moved = |settings: &super::Settings| {
        settings
            .jump_host
            .as_deref()
            .is_some_and(|value| jump(value).is_some())
    };
    if names_moved(&folder.settings) {
        files.push(join(&folder.path, FOLDER_SETTINGS_FILE));
    }
    for host in &folder.hosts {
        if names_moved(&host.settings) {
            files.push(host.id.as_str().to_string());
        }
    }
    for child in &folder.folders {
        collect_jump_files(child, jump, files);
    }
}

/// `text` (whose content is `original`) with the values that differ in
/// `expected` replaced in place, if that gives exactly `expected`. Only
/// scalar values of `key: value` lines are replaced; `None` when the
/// changes are not all on such lines.
fn rewrite_text(text: &str, original: &Mapping, expected: &Mapping) -> Option<String> {
    let mut changes = Vec::new();
    collect_changes(original, expected, &mut changes);
    let mut result = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        result.push_str(&replace_value(line, &changes).unwrap_or_else(|| line.to_string()));
    }
    let parsed: Value = serde_norway::from_str(&result).ok()?;
    (parsed == Value::Mapping(expected.clone())).then_some(result)
}

/// A value to replace: under `key`, `from` becomes `to`.
struct Change {
    key: String,
    from: String,
    to: String,
}

fn collect_changes(original: &Mapping, expected: &Mapping, changes: &mut Vec<Change>) {
    for (key, new) in expected {
        let Some(key_text) = scalar_text(key) else {
            continue;
        };
        match (original.get(key), new) {
            (Some(Value::Mapping(old)), Value::Mapping(new)) => collect_changes(old, new, changes),
            (Some(old), new) if old != new => {
                if let (Some(from), Some(to)) = (
                    if old.is_null() {
                        Some(String::new())
                    } else {
                        scalar_text(old)
                    },
                    scalar_text(new),
                ) {
                    changes.push(Change {
                        key: key_text,
                        from,
                        to,
                    });
                }
            }
            _ => {}
        }
    }
}

/// `line` with its value replaced, if it is `key: value` for one of
/// `changes`.
fn replace_value(line: &str, changes: &[Change]) -> Option<String> {
    let body = line.trim_end_matches(['\n', '\r']);
    let ending = &line[body.len()..];
    let indent = body.len() - body.trim_start().len();
    let (key, rest) = body[indent..].split_once(':')?;
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let spaces = rest.len() - rest.trim_start().len();
    let value_start = indent + key.len() + 1 + spaces;
    let value_end = value_start + value_length(&body[value_start..]);
    let value_text = &body[value_start..value_end];
    let value = if value_text.is_empty() {
        String::new()
    } else {
        match serde_norway::from_str::<Value>(value_text).ok()? {
            Value::Null => String::new(),
            value => scalar_text(&value)?,
        }
    };
    let change = changes
        .iter()
        .find(|change| change.key == key.trim() && change.from == value)?;
    let separator = if spaces == 0 { " " } else { "" };
    Some(format!(
        "{}{separator}{}{}{ending}",
        &body[..value_start],
        scalar(&change.to),
        &body[value_end..],
    ))
}

/// The length of the scalar at the start of `text`: a quoted one up to its
/// closing quote, a plain one up to a comment, without trailing blanks.
fn value_length(text: &str) -> usize {
    let bytes = text.as_bytes();
    match bytes.first() {
        Some(b'\'') => {
            let mut i = 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    return i + 1;
                }
                i += 1;
            }
            text.len()
        }
        Some(b'"') => {
            let mut i = 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 2,
                    b'"' => return i + 1,
                    _ => i += 1,
                }
            }
            text.len()
        }
        _ => {
            let end = text
                .match_indices('#')
                .find(|(i, _)| *i > 0 && text[..*i].ends_with([' ', '\t']))
                .map_or(text.len(), |(i, _)| i);
            text[..end].trim_end().len()
        }
    }
}

/// `value` as a YAML scalar on one line.
fn scalar(value: &str) -> String {
    serde_norway::to_string(&Value::String(value.to_string()))
        .map(|text| text.trim_end().to_string())
        .unwrap_or_else(|_| format!("{value:?}"))
}
