//! What kind of folder a folder is before it is opened as a workspace
//! (#131). Opening one makes it a workspace: the app writes its
//! [`WORKSPACE_FILE`] into it and shows its subfolders as host folders. A
//! folder that holds other things and nothing of a workspace (a Documents
//! folder chosen by mistake) is asked about first, so this only reads.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{
    check_root, read_mapping, yaml_stem, OpenError, ADDRESS, FILTERS_FILE, FOLDER_SETTINGS_FILE,
    HIGHLIGHTS_FILE, JOBS_FILE, PLATFORMS_FILE, ROLES_FILE, SNIPPETS_FILE, WORKSPACE_FILE,
};

/// What a folder holds, as far as opening it as a workspace goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderKind {
    /// It holds a host file (a YAML mapping with an `address`, in it or in
    /// a subfolder) or one of a workspace's own files (its
    /// [`WORKSPACE_FILE`], a workspace-wide definitions file at its root, a
    /// [`FOLDER_SETTINGS_FILE`] anywhere): it opens as it is.
    Workspace,
    /// Nothing but hidden entries (`.git`) and folders holding nothing
    /// else: how a new workspace starts when its folder was made by hand.
    Empty,
    /// Other things and nothing of a workspace: opening it writes a file
    /// into a folder that was not one, so the user is asked first.
    Other,
}

/// The workspace-wide files only a workspace's root holds.
const ROOT_FILES: [&str; 8] = [
    WORKSPACE_FILE,
    FOLDER_SETTINGS_FILE,
    ROLES_FILE,
    FILTERS_FILE,
    PLATFORMS_FILE,
    SNIPPETS_FILE,
    HIGHLIGHTS_FILE,
    JOBS_FILE,
];

/// A host file is small: a bigger YAML file is not read to find out.
const LARGEST_HOST_FILE: u64 = 256 * 1024;

/// How many entries are looked at before a folder with none of a
/// workspace in them is taken for one of other things (a whole Documents
/// folder is not walked).
const MOST_ENTRIES: usize = 20_000;

/// Says what kind of folder `root` is ([`FolderKind`]), reading only: the
/// folder is walked as [`super::Workspace::open`] loads it (hidden entries
/// and links to folders left out), nearest entries first, until something
/// of a workspace is found. A root that is not there, is a file or cannot
/// be listed is refused as opening refuses it.
pub fn folder_kind(root: impl AsRef<Path>) -> Result<FolderKind, OpenError> {
    let root = root.as_ref();
    check_root(root)?;
    let mut other = false;
    let mut seen = 0;
    let mut folders: VecDeque<(PathBuf, bool)> = VecDeque::from([(root.to_path_buf(), true)]);
    while let Some((dir, is_root)) = folders.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            other = true;
            continue;
        };
        for entry in entries {
            seen += 1;
            if seen > MOST_ENTRIES {
                return Ok(FolderKind::Other);
            }
            let Ok(entry) = entry else {
                other = true;
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                other = true;
                continue;
            };
            if file_type.is_dir() {
                folders.push_back((entry.path(), false));
                continue;
            }
            if file_type.is_symlink() && entry.path().is_dir() {
                // Not followed when loading either.
                other = true;
                continue;
            }
            let workspace_file = if is_root {
                ROOT_FILES.contains(&name.as_str())
            } else {
                name == FOLDER_SETTINGS_FILE
            };
            if workspace_file || (!name.starts_with('_') && is_host_file(&entry.path(), &name)) {
                return Ok(FolderKind::Workspace);
            }
            other = true;
        }
    }
    Ok(if other {
        FolderKind::Other
    } else {
        FolderKind::Empty
    })
}

/// Whether the file is a host file: YAML whose top level maps `address`.
fn is_host_file(path: &Path, name: &str) -> bool {
    yaml_stem(name).is_some()
        && fs::metadata(path).is_ok_and(|metadata| metadata.len() <= LARGEST_HOST_FILE)
        && read_mapping(path).is_ok_and(|document| document.contains_key(ADDRESS))
}
