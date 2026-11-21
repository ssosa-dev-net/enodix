//! A host's history, kept outside the workspace by its path: its session
//! logs ([`LogFolder`]) and its config history ([`ConfigStore`]). When a
//! host's id changes ([`HostMove`], from renaming or moving it or a folder
//! above it), [`carry`] moves both to the new path.
//!
//! History is moved file by file, so that a log being written by an open
//! session moves too: the session goes on writing into it in the new place,
//! and its next connections log there ([`SessionManager::move_logs`]); the
//! records of sessions to it name its new id ([`SessionRecords::move_hosts`]).
//! History is never merged into another host's: when the new place already
//! holds files (an earlier host of that name), the host's history stays
//! where it was. What stays, wholly or partly, is reported
//! ([`LeftHistory`]), never dropped silently.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::configs::ConfigStore;
use crate::logging::LogFolder;
use crate::session::{SessionId, SessionManager, SessionRecords};
use crate::workspace::{HostId, HostMove, Workspace};

/// Which history of a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryKind {
    SessionLogs,
    ConfigHistory,
}

/// History that did not follow its host to the new path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftHistory {
    /// The host's new id.
    pub host: HostId,
    /// Its id before.
    pub from: HostId,
    pub kind: HistoryKind,
    /// Where the history (or the part of it not moved) is.
    pub folder: PathBuf,
    /// Why it stayed there.
    pub reason: String,
}

/// The sessions of the app: they log to the hosts' folders and their
/// records name the hosts.
#[derive(Clone, Copy)]
pub struct Sessions<'a> {
    pub manager: &'a SessionManager,
    pub records: &'a SessionRecords,
}

/// What [`carry`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Carried {
    /// What could not be moved, in the order of the moves.
    pub left: Vec<LeftHistory>,
    /// The sessions whose records name a moved host's new id now.
    pub sessions: Vec<SessionId>,
}

/// Moves the session logs and config history of each of `moves` from the
/// host's old path to its new one, in `workspace` (as it is after the
/// move: its folder name and id place the history). The `sessions`
/// logging to an old folder log to the new one, and their records and
/// origins (what they reconnect to) name the new ids.
pub fn carry(
    workspace: &Workspace,
    moves: &[HostMove],
    logs: &LogFolder,
    configs: &ConfigStore,
    sessions: Option<Sessions<'_>>,
) -> Carried {
    let mut left = Vec::new();
    let mut stayed = |moved: &HostMove, kind, folder: PathBuf, reason: String| {
        left.push(LeftHistory {
            host: moved.to.clone(),
            from: moved.from.clone(),
            kind,
            folder,
            reason,
        });
    };
    let logs_root = logs.root().join(crate::logging::WORKSPACES);
    for moved in moves {
        let from = logs.folder_of(workspace, &moved.from);
        let to = logs.folder_of(workspace, &moved.to);
        let result = move_files(&from, &to, &logs_root, || {
            if let Some(sessions) = sessions {
                sessions.manager.move_logs(&from, &to);
            }
        });
        if let Err(reason) = result {
            stayed(moved, HistoryKind::SessionLogs, from, reason);
        }
    }
    if let Some(id) = workspace.id() {
        let configs_root = configs.root().join(id);
        for moved in moves {
            // A path that cannot hold configurations never had any.
            let Ok(from) = configs.host_folder(id, &moved.from) else {
                continue;
            };
            let result = match configs.host_folder(id, &moved.to) {
                Ok(to) => configs.while_not_saving(|| move_files(&from, &to, &configs_root, || {})),
                Err(error) if files_in(&from).is_ok_and(|files| !files.is_empty()) => {
                    Err(error.to_string())
                }
                Err(_) => Ok(()),
            };
            if let Err(reason) = result {
                stayed(moved, HistoryKind::ConfigHistory, from, reason);
            }
        }
    }
    let sessions = sessions.map_or_else(Vec::new, |sessions| {
        // They reconnect to the new ids too (#130).
        sessions.manager.move_hosts(workspace.root(), moves);
        sessions.records.move_hosts(workspace.root(), moves)
    });
    Carried { left, sessions }
}

/// Moves the files (not the folders) of the folder `from` into `to`, then
/// removes `from` and its parents below `root` that are left empty.
/// Refused, moving nothing, when `to` already holds files; `before` runs
/// once it is known that the files will be moved.
fn move_files(from: &Path, to: &Path, root: &Path, before: impl FnOnce()) -> Result<(), String> {
    let files = match files_in(from) {
        Ok(files) => files,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("it could not be read: {error}")),
    };
    if files.is_empty() {
        return Ok(());
    }
    if same_folder(from, to) {
        // Only the case of a name changed, on a system that ignores it: the
        // folder is found under either name. Give it the new one if it can.
        before();
        let _ = fs::rename(from, to);
        return Ok(());
    }
    match files_in(to) {
        Ok(held) if !held.is_empty() => {
            return Err(format!(
                "{} already holds the history of an earlier host of that name; it was not mixed with this host's",
                to.display()
            ))
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(format!("{} could not be read: {error}", to.display()))
        }
        _ => {}
    }
    fs::create_dir_all(to)
        .map_err(|error| format!("{} could not be created: {error}", to.display()))?;
    before();
    let mut failed = Vec::new();
    let mut move_all = |files: Vec<std::ffi::OsString>| {
        for file in files {
            if let Err(error) = fs::rename(from.join(&file), to.join(&file)) {
                failed.push(format!("{} ({error})", file.to_string_lossy()));
            }
        }
    };
    move_all(files);
    // A session that started logging while `before` ran (it had read the
    // old folder) created its file there: it follows too.
    let late: Vec<_> = files_in(from)
        .unwrap_or_default()
        .into_iter()
        .filter(|file| !to.join(file).exists())
        .collect();
    move_all(late);
    if !failed.is_empty() {
        return Err(format!(
            "these files could not be moved to {}: {}",
            to.display(),
            failed.join(", ")
        ));
    }
    let mut empty = Some(from);
    while let Some(folder) = empty.filter(|folder| folder.starts_with(root) && *folder != root) {
        if fs::remove_dir(folder).is_err() {
            break;
        }
        empty = folder.parent();
    }
    Ok(())
}

/// The names of the files (not folders) in `folder`.
fn files_in(folder: &Path) -> io::Result<Vec<std::ffi::OsString>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.file_name());
        }
    }
    files.sort();
    Ok(files)
}

/// Whether the two paths are one folder that exists.
fn same_folder(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
