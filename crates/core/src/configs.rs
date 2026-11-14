//! Config history: every device configuration a backup fetched, kept per
//! host until the user deletes it.
//!
//! A [`ConfigStore`] lives under the app's data area, never in a workspace,
//! and keeps each workspace's hosts apart by the workspace id:
//! `<root>/<workspace id>/<tree path>/<host file stem>/<version>.txt`, a
//! version being named by its local save time (`YYYY-MM-DD_HH-MM-SS`,
//! `_2`... when several are saved in one second). [`ConfigStore::save`]
//! keeps a new version only when the configuration changed since the latest
//! one, leaving out of the comparison the lines that match the platform
//! definition's ignored-lines pattern (such as a timestamp); the file kept
//! holds every line. Nothing is pruned and nothing is encrypted: a backup is
//! as sensitive as the configuration it holds.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::diff::{diff_lines, lines, DiffLine};
use crate::workspace::HostId;

/// The extension of a version's file.
pub const VERSION_EXTENSION: &str = "txt";

/// What [`ConfigStore::save`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SaveOutcome {
    /// The configuration changed (or is the host's first): kept as
    /// `version`.
    Saved { version: String },
    /// The configuration is the same as the latest version, `version`,
    /// apart from ignored lines: nothing was written.
    Unchanged { version: String },
}

impl SaveOutcome {
    /// The version that holds the configuration now.
    pub fn version(&self) -> &str {
        match self {
            Self::Saved { version } | Self::Unchanged { version } => version,
        }
    }
}

/// A kept version of a host's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigVersion {
    /// Its name: the local time it was saved, `YYYY-MM-DD_HH-MM-SS[_n]`.
    pub id: String,
    /// How many lines it has.
    pub lines: usize,
    /// Its size in bytes.
    pub bytes: u64,
}

/// Why kept configurations cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A workspace id that cannot name a folder.
    #[error("configurations cannot be kept for the workspace id {0:?}")]
    BadWorkspaceId(String),
    /// A host id that cannot name a folder (such as one going up).
    #[error("configurations cannot be kept for the host {0}")]
    BadHostId(HostId),
    #[error("{host} has no configuration version {version}")]
    NoSuchVersion { host: HostId, version: String },
    /// The ignored-lines pattern cannot be used.
    #[error("the ignored-lines pattern cannot be used: {0}")]
    Pattern(String),
    #[error("{} is not a folder", .0.display())]
    NotAFolder(PathBuf),
    #[error("{}: {error}", .path.display())]
    Io { path: PathBuf, error: io::Error },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ConfigError + '_ {
    move |error| ConfigError::Io {
        path: path.to_path_buf(),
        error,
    }
}

/// Where configurations are kept. Clones share one lock, so that two saves
/// of a host never both decide against the same latest version.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    root: PathBuf,
    saving: Arc<Mutex<()>>,
}

impl ConfigStore {
    /// A store in `root` (created when the first version is saved).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            saving: Arc::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder holding the versions of `host` in `workspace` (it may not
    /// exist yet).
    pub fn host_folder(&self, workspace: &str, host: &HostId) -> Result<PathBuf, ConfigError> {
        if !is_plain_name(workspace) {
            return Err(ConfigError::BadWorkspaceId(workspace.to_string()));
        }
        let path = host.as_str();
        let (parents, file) = path.rsplit_once('/').unwrap_or(("", path));
        let stem = Path::new(file)
            .file_stem()
            .map_or_else(|| file.to_string(), |s| s.to_string_lossy().into_owned());
        let mut folder = self.root.join(workspace);
        for part in parents.split('/').filter(|part| !part.is_empty()) {
            if !is_plain_name(part) {
                return Err(ConfigError::BadHostId(host.clone()));
            }
            folder.push(part);
        }
        if !is_plain_name(&stem) {
            return Err(ConfigError::BadHostId(host.clone()));
        }
        folder.push(stem);
        Ok(folder)
    }

    /// Keeps `text` as a new version of `host`'s configuration, unless it
    /// is the same as the latest version once the lines matching `ignored`
    /// (a pattern, applied to each line) are left out of both.
    pub fn save(
        &self,
        workspace: &str,
        host: &HostId,
        text: &str,
        ignored: Option<&str>,
    ) -> Result<SaveOutcome, ConfigError> {
        let ignored = match ignored.filter(|pattern| !pattern.trim().is_empty()) {
            Some(pattern) => {
                Some(Regex::new(pattern).map_err(|error| ConfigError::Pattern(error.to_string()))?)
            }
            None => None,
        };
        let folder = self.host_folder(workspace, host)?;
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(latest) = version_ids(&folder)?.into_iter().next() {
            let path = version_path(&folder, &latest);
            let kept = fs::read(&path).map_err(io_error(&path))?;
            let kept = String::from_utf8_lossy(&kept);
            if significant(&kept, ignored.as_ref()) == significant(text, ignored.as_ref()) {
                return Ok(SaveOutcome::Unchanged { version: latest });
            }
        }
        fs::create_dir_all(&folder).map_err(io_error(&folder))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let version = (1..)
            .map(|n| match n {
                1 => stamp.clone(),
                n => format!("{stamp}_{n}"),
            })
            .find(|name| !version_path(&folder, name).exists())
            .expect("an unused name is found");
        let path = version_path(&folder, &version);
        // Written whole, then named, so that a version is never read half
        // written.
        let temporary = folder.join(format!(".{version}.{VERSION_EXTENSION}.tmp"));
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, &path))
            .map_err(io_error(&path))?;
        Ok(SaveOutcome::Saved { version })
    }

    /// The versions of `host`'s configuration, newest first.
    pub fn versions(
        &self,
        workspace: &str,
        host: &HostId,
    ) -> Result<Vec<ConfigVersion>, ConfigError> {
        let folder = self.host_folder(workspace, host)?;
        version_ids(&folder)?
            .into_iter()
            .map(|id| {
                let path = version_path(&folder, &id);
                let bytes = fs::read(&path).map_err(io_error(&path))?;
                Ok(ConfigVersion {
                    lines: lines(&String::from_utf8_lossy(&bytes)).len(),
                    bytes: bytes.len() as u64,
                    id,
                })
            })
            .collect()
    }

    /// The text of the version `version` of `host`'s configuration.
    pub fn read(
        &self,
        workspace: &str,
        host: &HostId,
        version: &str,
    ) -> Result<String, ConfigError> {
        let path = self.existing(workspace, host, version)?;
        let bytes = fs::read(&path).map_err(io_error(&path))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// A line diff of two versions of `host`'s configuration (`old` first),
    /// every line included (ignored ones too).
    pub fn diff(
        &self,
        workspace: &str,
        host: &HostId,
        old: &str,
        new: &str,
    ) -> Result<Vec<DiffLine>, ConfigError> {
        Ok(diff_lines(
            &self.read(workspace, host, old)?,
            &self.read(workspace, host, new)?,
        ))
    }

    /// Copies the version `version` of `host`'s configuration into the
    /// folder `to`, as `<host file stem>_<version>.txt` (`_2`... if taken);
    /// the copy's path.
    pub fn export(
        &self,
        workspace: &str,
        host: &HostId,
        version: &str,
        to: &Path,
    ) -> Result<PathBuf, ConfigError> {
        let path = self.existing(workspace, host, version)?;
        if !to.is_dir() {
            return Err(ConfigError::NotAFolder(to.to_path_buf()));
        }
        let stem = self
            .host_folder(workspace, host)?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = format!("{stem}_{version}");
        for attempt in 1.. {
            let name = match attempt {
                1 => format!("{base}.{VERSION_EXTENSION}"),
                n => format!("{base}_{n}.{VERSION_EXTENSION}"),
            };
            let copy = to.join(name);
            let created = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&copy);
            match created {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(&copy)(error)),
                Ok(file) => {
                    drop(file);
                    fs::copy(&path, &copy).map_err(io_error(&copy))?;
                    return Ok(copy);
                }
            }
        }
        unreachable!("an unused name is found")
    }

    /// The file of an existing version.
    fn existing(
        &self,
        workspace: &str,
        host: &HostId,
        version: &str,
    ) -> Result<PathBuf, ConfigError> {
        let path = version_path(&self.host_folder(workspace, host)?, version);
        if !is_plain_name(version) || !path.is_file() {
            return Err(ConfigError::NoSuchVersion {
                host: host.clone(),
                version: version.to_string(),
            });
        }
        Ok(path)
    }
}

fn version_path(folder: &Path, version: &str) -> PathBuf {
    folder.join(format!("{version}.{VERSION_EXTENSION}"))
}

/// The versions kept in `folder`, newest first.
fn version_ids(folder: &Path) -> Result<Vec<String>, ConfigError> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(folder)(error)),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error(folder))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(&format!(".{VERSION_EXTENSION}")) else {
            continue;
        };
        if id.starts_with('.') || !entry.path().is_file() {
            continue;
        }
        ids.push(id.to_string());
    }
    ids.sort_by(|a, b| natural(b).cmp(&natural(a)));
    Ok(ids)
}

/// The lines of `text` that count when comparing: those `ignored` does not
/// match.
fn significant<'a>(text: &'a str, ignored: Option<&Regex>) -> Vec<&'a str> {
    lines(text)
        .into_iter()
        .filter(|line| ignored.is_none_or(|pattern| !pattern.is_match(line)))
        .collect()
}

/// A version's sort key, so that `_10` comes after `_9`.
fn natural(id: &str) -> (&str, u32) {
    match id.rsplit_once('_') {
        Some((stamp, n)) if stamp.len() >= 19 => (stamp, n.parse().unwrap_or(0)),
        _ => (id, 1),
    }
}

/// Whether `name` is one plain file name (no separators, not `.`/`..`).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
}
