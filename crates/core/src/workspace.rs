//! Workspaces: an inventory stored as a folder of YAML files.
//!
//! Directories are the hierarchy. Each host is one YAML file (`*.yaml` or
//! `*.yml`), so that edits to different hosts never conflict in git. Each
//! directory may hold one settings file, [`FOLDER_SETTINGS_FILE`]. Other
//! files whose name starts with `_` are reserved for workspace-wide
//! definitions and are not hosts; hidden entries (a leading `.`, such as
//! `.git`) and files of other types are ignored.
//!
//! The files are the only source of truth. Loading never stops at a bad
//! file: each one becomes a [`Problem`] naming the file, and the rest of the
//! workspace loads.

use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use crate::session::{SshTarget, DEFAULT_SSH_PORT};

/// The settings file a directory may hold (description now; inherited
/// settings and web links later).
pub const FOLDER_SETTINGS_FILE: &str = "_folder.yaml";

/// Host file keys.
const NAME: &str = "name";
const ADDRESS: &str = "address";
const PORT: &str = "port";
const DESCRIPTION: &str = "description";

/// A loaded workspace: its tree and the problems found while loading it.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    tree: Folder,
    problems: Vec<Problem>,
}

/// A directory of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    /// The directory's name.
    pub name: String,
    /// The directory relative to the workspace root, `/`-separated; empty
    /// for the root.
    pub path: String,
    pub description: Option<String>,
    /// Subfolders, ordered by name.
    pub folders: Vec<Folder>,
    /// Hosts in this directory, ordered by name.
    pub hosts: Vec<Host>,
}

/// Identifies a host within its workspace: the host file's path relative to
/// the workspace root, `/`-separated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(String);

impl HostId {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A host, read from its own file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Host {
    pub id: HostId,
    /// `name` in the file; the file name without its extension when absent.
    pub name: String,
    /// The management address (`address` in the file).
    pub address: String,
    /// `port` in the file, if set.
    pub port: Option<u16>,
    /// Everything the file holds, in file order, so that keys this version
    /// does not know survive a rewrite.
    #[serde(skip)]
    document: Mapping,
}

impl Host {
    /// The SSH target for this host with the given credentials. Without a
    /// port in the file, the SSH default applies.
    pub fn ssh_target(&self, username: String, password: String) -> SshTarget {
        SshTarget {
            host: self.address.clone(),
            port: self.port.unwrap_or(DEFAULT_SSH_PORT),
            username,
            password,
        }
    }
}

/// A file that could not be loaded, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The file (or directory) relative to the workspace root, `/`-separated.
    pub file: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

/// The workspace folder itself could not be opened.
#[derive(Debug, thiserror::Error)]
#[error("cannot open workspace {}: {reason}", path.display())]
pub struct OpenError {
    pub path: PathBuf,
    pub reason: String,
}

/// A host file could not be written.
#[derive(Debug, thiserror::Error)]
#[error("cannot write {file}: {source}")]
pub struct WriteError {
    pub file: String,
    #[source]
    pub source: io::Error,
}

impl Workspace {
    /// Loads the workspace folder at `root`. Only a missing or unreadable
    /// root fails; bad files inside it are reported by [`Self::problems`].
    pub fn open(root: impl AsRef<Path>) -> Result<Self, OpenError> {
        let root = root.as_ref().to_path_buf();
        let open_error = |reason: String| OpenError {
            path: root.clone(),
            reason,
        };
        let metadata = fs::metadata(&root).map_err(|error| open_error(error.to_string()))?;
        if !metadata.is_dir() {
            return Err(open_error("not a folder".into()));
        }
        fs::read_dir(&root).map_err(|error| open_error(error.to_string()))?;

        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let mut problems = Vec::new();
        let tree = load_folder(&root, name, String::new(), &mut problems);
        problems.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(Self {
            root,
            tree,
            problems,
        })
    }

    /// The folder this workspace was opened from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root folder of the tree.
    pub fn tree(&self) -> &Folder {
        &self.tree
    }

    /// Files that could not be loaded, ordered by path.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The host with this id, if the workspace has it.
    pub fn host(&self, id: &HostId) -> Option<&Host> {
        fn find<'a>(folder: &'a Folder, id: &HostId) -> Option<&'a Host> {
            folder
                .hosts
                .iter()
                .find(|host| &host.id == id)
                .or_else(|| folder.folders.iter().find_map(|child| find(child, id)))
        }
        find(&self.tree, id)
    }

    /// Writes `host` back to its file. Keys of the file that the host does
    /// not model are kept, in their original order; comments are not.
    ///
    /// The workspace in memory is not updated; open it again to see the
    /// change.
    pub fn write_host(&self, host: &Host) -> Result<(), WriteError> {
        let write_error = |source| WriteError {
            file: host.id.to_string(),
            source,
        };
        if self.host(&host.id).is_none() {
            return Err(write_error(io::Error::new(
                io::ErrorKind::NotFound,
                "no such host in this workspace",
            )));
        }

        let mut document = host.document.clone();
        // A name taken from the file name stays implicit.
        let file_stem = host.id.as_str().rsplit('/').next().and_then(yaml_stem);
        if document.contains_key(NAME) || file_stem != Some(host.name.as_str()) {
            document.insert(NAME.into(), host.name.clone().into());
        }
        document.insert(ADDRESS.into(), host.address.clone().into());
        match host.port {
            Some(port) => {
                document.insert(PORT.into(), port.into());
            }
            None => {
                document.shift_remove(PORT);
            }
        }
        let text = serde_norway::to_string(&document)
            .map_err(|error| write_error(io::Error::other(error)))?;

        // Write beside the file, then replace it, so a failure never leaves
        // a half-written host behind.
        let path = self.root.join(host.id.as_str());
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let temporary = path.with_file_name(format!(".{file_name}.tmp"));
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, &path))
            .map_err(|error| {
                let _ = fs::remove_file(&temporary);
                write_error(error)
            })
    }
}

fn load_folder(dir: &Path, name: String, path: String, problems: &mut Vec<Problem>) -> Folder {
    let mut folder = Folder {
        name,
        path: path.clone(),
        description: None,
        folders: Vec::new(),
        hosts: Vec::new(),
    };
    let problem = |file: &str, message: String| Problem {
        file: if file.is_empty() {
            ".".into()
        } else {
            file.into()
        },
        message,
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            problems.push(problem(&path, format!("cannot read folder: {error}")));
            return folder;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                problems.push(problem(&path, format!("cannot read folder: {error}")));
                continue;
            }
        };
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        if entry_name.starts_with('.') {
            continue;
        }
        let entry_path = if path.is_empty() {
            entry_name.clone()
        } else {
            format!("{path}/{entry_name}")
        };
        let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        let is_dir = fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir());

        if is_dir && is_link {
            // A link can point back up the tree; folders are real directories.
            problems.push(problem(
                &entry_path,
                "links to folders are not followed".into(),
            ));
        } else if is_dir {
            let child = load_folder(&entry.path(), entry_name, entry_path, problems);
            folder.folders.push(child);
        } else if entry_name == FOLDER_SETTINGS_FILE {
            match read_mapping(&entry.path()).and_then(|document| folder_description(&document)) {
                Ok(description) => folder.description = description,
                Err(message) => problems.push(problem(&entry_path, message)),
            }
        } else if entry_name.starts_with('_') {
            // Reserved for workspace-wide definitions.
        } else if let Some(stem) = yaml_stem(&entry_name) {
            match read_mapping(&entry.path()).and_then(|document| {
                host_from_document(HostId::new(entry_path.clone()), stem, document)
            }) {
                Ok(host) => folder.hosts.push(host),
                Err(message) => problems.push(problem(&entry_path, message)),
            }
        }
    }

    folder.folders.sort_by(|a, b| by_name(&a.name, &b.name));
    folder
        .hosts
        .sort_by(|a, b| by_name(&a.name, &b.name).then_with(|| a.id.cmp(&b.id)));
    folder
}

/// Case-insensitive, with exact order as the tie-break.
fn by_name(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The file name without its YAML extension, if it is a YAML file.
fn yaml_stem(file_name: &str) -> Option<&str> {
    let (stem, extension) = file_name.rsplit_once('.')?;
    let is_yaml = extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml");
    (is_yaml && !stem.is_empty()).then_some(stem)
}

/// Reads a YAML file whose top level must be a mapping. An empty file is an
/// empty mapping.
fn read_mapping(path: &Path) -> Result<Mapping, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("cannot read file: {error}"))?;
    if text.trim().is_empty() {
        return Ok(Mapping::new());
    }
    match serde_norway::from_str::<Value>(&text) {
        Ok(Value::Mapping(mapping)) => Ok(mapping),
        Ok(Value::Null) => Ok(Mapping::new()),
        Ok(_) => Err("expected a mapping of keys to values at the top level".into()),
        Err(error) => Err(format!("invalid YAML: {error}")),
    }
}

fn host_from_document(id: HostId, stem: &str, document: Mapping) -> Result<Host, String> {
    let name = optional_text(&document, NAME)?.unwrap_or_else(|| stem.to_string());
    let address =
        optional_text(&document, ADDRESS)?.ok_or_else(|| format!("missing `{ADDRESS}`"))?;
    let port = match document.get(PORT) {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => Some(
            number
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or_else(|| format!("`{PORT}` must be a number from 1 to 65535"))?,
        ),
        Some(_) => return Err(format!("`{PORT}` must be a number from 1 to 65535")),
    };
    Ok(Host {
        id,
        name,
        address,
        port,
        document,
    })
}

fn folder_description(document: &Mapping) -> Result<Option<String>, String> {
    optional_text(document, DESCRIPTION)
}

/// A key whose value, when present, must be non-empty text (a number is
/// taken as its text).
fn optional_text(document: &Mapping, key: &str) -> Result<Option<String>, String> {
    match document.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim().to_string())),
        Some(Value::String(_)) => Err(format!("`{key}` is empty")),
        Some(Value::Number(number)) => Ok(Some(number.to_string())),
        Some(_) => Err(format!("`{key}` must be text")),
    }
}
