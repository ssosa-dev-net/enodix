//! Config history: every device configuration a backup fetched, kept per
//! host until the user deletes it.
//!
//! A [`ConfigStore`] lives under the app's data area, never in a workspace,
//! and keeps each workspace's hosts apart by the workspace id:
//! `<root>/<workspace id>/<tree path>/<host folder>/<version>.txt`, a
//! version being named by its local save time (`YYYY-MM-DD_HH-MM-SS`,
//! `_2`... when several are saved in one second). [`ConfigStore::save`]
//! keeps a new version only when the configuration changed since the latest
//! one, leaving out of the comparison the lines that match the platform
//! definition's ignored-lines pattern (such as a timestamp); the file kept
//! holds every line. Nothing is pruned and nothing is encrypted: a backup is
//! as sensitive as the configuration it holds.
//!
//! A host's folder is named after its file: the file's stem for a file
//! ending in `.yaml` (`r1.yaml` → `r1`, as every host had before #127),
//! the whole file name for any other (`r1.yml` → `r1.yml`), and for a
//! `.yaml` file whose stem itself ends like a host file (`r1.yml.yaml`).
//! So no two host files of one folder share a folder: before #127 every
//! host had its stem, and `r1.yaml` and `r1.yml` kept one history between
//! them. A host whose folder changed name finds the history kept under its
//! stem once [`ConfigStore::claim_history`] has run for its workspace;
//! history two hosts already shared is never split or copied (see there).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::diff::{diff_lines, lines, DiffLine};
use crate::workspace::{Host, HostId, Workspace};

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

/// The name of the note, in a host's folder, of when a backup last found
/// its configuration unchanged (removed when a version is saved). Not a
/// version: versions end in [`VERSION_EXTENSION`].
const UNCHANGED_NOTE: &str = ".unchanged";

/// What a host's last backup did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LastBackup {
    /// It kept the host's first version.
    First,
    /// It kept a version that differs from the one before.
    Changed,
    /// It found the configuration the same as the newest version.
    Unchanged,
}

/// A host with kept configurations ([`ConfigStore::backed_up`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackedUpHost {
    pub host: HostId,
    /// How many versions are kept.
    pub versions: usize,
    /// The newest version's name.
    pub newest: String,
    /// When it was last backed up, local time `YYYY-MM-DD_HH-MM-SS`: the
    /// newest version's time, or a later backup's that changed nothing.
    pub last_backup: String,
    pub last: LastBackup,
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
        let (parent, file) = self.parent_folder(workspace, host)?;
        let name = folder_name(file);
        if !is_plain_name(&name) {
            return Err(ConfigError::BadHostId(host.clone()));
        }
        Ok(parent.join(name))
    }

    /// The folder of `host`'s tree path in `workspace`, and its file name.
    fn parent_folder<'a>(
        &self,
        workspace: &str,
        host: &'a HostId,
    ) -> Result<(PathBuf, &'a str), ConfigError> {
        if !is_plain_name(workspace) {
            return Err(ConfigError::BadWorkspaceId(workspace.to_string()));
        }
        let path = host.as_str();
        let (parents, file) = path.rsplit_once('/').unwrap_or(("", path));
        let mut folder = self.root.join(workspace);
        for part in parents.split('/').filter(|part| !part.is_empty()) {
            if !is_plain_name(part) {
                return Err(ConfigError::BadHostId(host.clone()));
            }
            folder.push(part);
        }
        Ok((folder, file))
    }

    /// Where versions of `host` were kept before #127 (its file's stem),
    /// when that is not its folder now.
    fn older_folder(&self, workspace: &str, host: &HostId) -> Option<PathBuf> {
        let (parent, file) = self.parent_folder(workspace, host).ok()?;
        let stem = stem(file);
        (stem != folder_name(file) && is_plain_name(&stem)).then(|| parent.join(stem))
    }

    /// Gives the hosts of `workspace` whose folder changed name with #127
    /// (any file not ending in `.yaml`) the versions kept for them under
    /// their file's stem, moving them file by file into their own folder.
    /// Run when a workspace is opened, before anything else reads its
    /// history; running it again does nothing more.
    ///
    /// Versions under a stem are a host's only when no other host of its
    /// folder is kept there or would have been: `r1.yml` takes `r1/` when
    /// there is no `r1.yaml` (or `R1.YML` where names differ by case).
    /// When there is, that folder was **shared** by both hosts before #127:
    /// its versions cannot be told apart, so none is moved, copied or
    /// deleted. If one of the hosts still has its folder there (`r1.yaml`),
    /// they stay its history, as they always were; the others start their
    /// own. If none does, they stay where they are, under neither host.
    /// Either way the folder is reported ([`SharedHistory`]), every time.
    ///
    /// A move cut short (the app closed half way) goes on the next time; a
    /// version already in the host's folder under the same name is never
    /// written over, and stays where it was, reported.
    pub fn claim_history(&self, workspace: &Workspace) -> Claimed {
        let mut claimed = Claimed::default();
        let Some(id) = workspace.id() else {
            return claimed;
        };
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        let mut folders = vec![workspace.tree()];
        while let Some(folder) = folders.pop() {
            folders.extend(folder.folders.iter());
            for host in &folder.hosts {
                self.claim_host(id, host, &folder.hosts, &mut claimed);
            }
        }
        claimed.shared.sort_by(|a, b| a.folder.cmp(&b.folder));
        claimed.shared.dedup_by(|a, b| a.folder == b.folder);
        claimed
    }

    fn claim_host(&self, workspace: &str, host: &Host, siblings: &[Host], claimed: &mut Claimed) {
        let Some(older) = self.older_folder(workspace, &host.id) else {
            return;
        };
        let Ok(own) = self.host_folder(workspace, &host.id) else {
            return;
        };
        let versions = match files_in(&older) {
            Ok(files) if !files.is_empty() => files,
            Ok(_) => return,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                claimed.left.push((
                    host.id.clone(),
                    format!(
                        "the configurations kept for it under {} could not be read: {error}",
                        older.display()
                    ),
                ));
                return;
            }
        };
        // Every other host of the folder kept there now, or before #127.
        let name = stem(file_of(&host.id)).to_lowercase();
        let mut keeper = None;
        let mut sharing = Vec::new();
        for other in siblings.iter().filter(|other| other.id != host.id) {
            // Only a name equal but for case can be the same folder.
            let file = file_of(&other.id);
            if folder_name(file).to_lowercase() != name && stem(file).to_lowercase() != name {
                continue;
            }
            let now = self.host_folder(workspace, &other.id).ok();
            let before = self.older_folder(workspace, &other.id);
            if now.as_deref().is_some_and(|now| same_place(now, &older)) {
                keeper = Some(other.id.clone());
                sharing.push(other.id.clone());
            } else if before
                .as_deref()
                .is_some_and(|before| same_place(before, &older))
            {
                sharing.push(other.id.clone());
            }
        }
        if !sharing.is_empty() {
            sharing.push(host.id.clone());
            sharing.sort();
            claimed.shared.push(SharedHistory {
                folder: older,
                hosts: sharing,
                kept_by: keeper,
                versions: versions.iter().filter(|file| is_version_file(file)).count(),
            });
            return;
        }
        if let Err(error) = fs::create_dir_all(&own) {
            claimed.left.push((
                host.id.clone(),
                format!(
                    "its configurations stay in {}: {} could not be created: {error}",
                    older.display(),
                    own.display()
                ),
            ));
            return;
        }
        let mut stayed = Vec::new();
        for file in versions {
            let to = own.join(&file);
            if fs::symlink_metadata(&to).is_ok() {
                if !is_version_file(&file) {
                    // A note or a temporary file: the host's own is newer.
                    let _ = fs::remove_file(older.join(&file));
                    continue;
                }
                stayed.push(format!(
                    "{} (a file of that name is in {} already)",
                    file.to_string_lossy(),
                    own.display()
                ));
                continue;
            }
            if let Err(error) = fs::rename(older.join(&file), &to) {
                stayed.push(format!("{} ({error})", file.to_string_lossy()));
            }
        }
        if stayed.is_empty() {
            // Only if empty: it may also be a tree folder of the same name.
            let _ = fs::remove_dir(&older);
        } else {
            claimed.left.push((
                host.id.clone(),
                format!(
                    "these of its configurations stay in {}: {}",
                    older.display(),
                    stayed.join(", ")
                ),
            ));
        }
    }

    /// Holds saves back while `work` runs (moving a host's versions), so
    /// that none lands in a folder being moved.
    pub(crate) fn while_not_saving<T>(&self, work: impl FnOnce() -> T) -> T {
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        work()
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
                // Only a note for the backups view: failing to write it
                // does not fail the backup.
                let _ = fs::write(folder.join(UNCHANGED_NOTE), now_stamp());
                return Ok(SaveOutcome::Unchanged { version: latest });
            }
        }
        fs::create_dir_all(&folder).map_err(io_error(&folder))?;
        let stamp = now_stamp();
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
        match fs::remove_file(folder.join(UNCHANGED_NOTE)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(io_error(&folder.join(UNCHANGED_NOTE))(error))
            }
            _ => {}
        }
        Ok(SaveOutcome::Saved { version })
    }

    /// Those of `hosts` (in that order) that have kept versions in
    /// `workspace`, each with its newest version and what its last backup
    /// did.
    pub fn backed_up(
        &self,
        workspace: &str,
        hosts: &[HostId],
    ) -> Result<Vec<BackedUpHost>, ConfigError> {
        let mut listed = Vec::new();
        for host in hosts {
            let folder = self.host_folder(workspace, host)?;
            let ids = version_ids(&folder)?;
            let Some(newest) = ids.first().cloned() else {
                continue;
            };
            let newest_time = newest.get(..19).unwrap_or(&newest).to_string();
            let note = fs::read_to_string(folder.join(UNCHANGED_NOTE))
                .ok()
                .map(|text| text.trim().to_string())
                .filter(|stamp| stamp.len() == 19 && *stamp >= newest_time);
            let (last, last_backup) = match note {
                Some(stamp) => (LastBackup::Unchanged, stamp),
                None if ids.len() == 1 => (LastBackup::First, newest_time),
                None => (LastBackup::Changed, newest_time),
            };
            listed.push(BackedUpHost {
                host: host.clone(),
                versions: ids.len(),
                newest,
                last_backup,
                last,
            });
        }
        Ok(listed)
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
        let base = format!("{}_{version}", stem(file_of(host)));
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

/// What [`ConfigStore::claim_history`] could not do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claimed {
    /// Folders two hosts or more kept versions in before #127.
    pub shared: Vec<SharedHistory>,
    /// Hosts some of whose versions stayed under their stem, and why
    /// (words to show as they are).
    pub left: Vec<(HostId, String)>,
}

/// A folder of versions that several hosts kept between them before #127
/// (their files differ only in the extension, or in case), left as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedHistory {
    pub folder: PathBuf,
    /// The hosts that kept versions there, in order.
    pub hosts: Vec<HostId>,
    /// The host whose folder it still is (a `.yaml` file's), which lists
    /// them; none when it is no host's folder now.
    pub kept_by: Option<HostId>,
    /// How many versions are there.
    pub versions: usize,
}

impl std::fmt::Display for SharedHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hosts: Vec<&str> = self.hosts.iter().map(HostId::as_str).collect();
        write!(
            f,
            "{} holds {} configuration version(s) that {} kept between them before each host had \
             a history of its own, which cannot be told apart: ",
            self.folder.display(),
            self.versions,
            hosts.join(" and ")
        )?;
        match &self.kept_by {
            Some(keeper) => write!(
                f,
                "they stay in {keeper}'s history, the others keep their own from now on"
            ),
            None => write!(
                f,
                "they stay there, in neither host's history, and each keeps its own from now on"
            ),
        }
    }
}

/// The name of a host file's folder in the store (module documentation).
fn folder_name(file: &str) -> String {
    match file.strip_suffix(".yaml") {
        Some(stem) if !stem.is_empty() && !ends_like_a_host_file(stem) => stem.to_string(),
        _ => file.to_string(),
    }
}

/// Whether `name` ends in `.yaml` or `.yml`, in any case.
fn ends_like_a_host_file(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
    })
}

/// A file name without its extension, as every host's folder was named
/// before #127.
fn stem(file: &str) -> String {
    Path::new(file)
        .file_stem()
        .map_or_else(|| file.to_string(), |s| s.to_string_lossy().into_owned())
}

/// The file name of `host`.
fn file_of(host: &HostId) -> &str {
    let path = host.as_str();
    path.rsplit_once('/').map_or(path, |(_, file)| file)
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

/// Whether `name` is a version's file (not a note or a temporary file).
fn is_version_file(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    !name.starts_with('.') && name.ends_with(&format!(".{VERSION_EXTENSION}"))
}

/// Whether the paths name one folder: the same path, or two that exist and
/// are one (names differing in case, where the system ignores it).
fn same_place(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (fs::canonicalize(a), fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

/// The local time now, as a version is named.
fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
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
