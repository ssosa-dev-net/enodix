//! App-level state: what the app remembers between launches (the
//! workspaces the user knows, most recently opened first, and the one to
//! reopen). It lives in the per-user application data folder, never in a
//! workspace. The caller chooses that folder; the core assumes nothing
//! about the platform.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The state file's name inside the data folder.
const FILE_NAME: &str = "state.yaml";

/// Where a state file that could not be read is kept, so that saving does
/// not lose the list it held.
const DAMAGED_FILE_NAME: &str = "state.yaml.damaged";

/// Why a workspace was not taken off the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ForgetError {
    #[error("the open workspace cannot be removed from the list; switch to another first")]
    Open,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// The workspace to reopen on launch.
    pub last_workspace: Option<PathBuf>,
    /// The workspace folders the user has opened or created, most recently
    /// opened first. Forgetting one only takes it off this list.
    pub workspaces: Vec<PathBuf>,
}

/// A workspace on the list, as the switcher shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnownWorkspace {
    pub path: PathBuf,
    /// Its folder's name.
    pub name: String,
    /// Whether it is the workspace open now.
    pub open: bool,
    /// Whether its folder is there (it may have been moved or deleted, or
    /// be on a drive that is not connected).
    pub available: bool,
}

impl AppState {
    /// Reads the state kept in `data_folder`. A missing or unreadable file
    /// gives the default state; a damaged one is first moved aside (to
    /// `state.yaml.damaged`), so that the next save does not destroy the
    /// list of workspaces it may still show a person.
    pub fn load(data_folder: &Path) -> Self {
        let path = data_folder.join(FILE_NAME);
        let mut state: Self = match fs::read_to_string(&path) {
            Ok(text) => serde_norway::from_str(&text).unwrap_or_else(|_| {
                let _ = fs::rename(&path, data_folder.join(DAMAGED_FILE_NAME));
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        // Files written before the list existed hold only the last one.
        if let Some(last) = state.last_workspace.clone() {
            if !state
                .workspaces
                .iter()
                .any(|known| same_folder(known, &last))
            {
                state.workspaces.insert(0, last);
            }
        }
        state
    }

    /// Writes the state to `data_folder`, creating the folder if needed.
    pub fn save(&self, data_folder: &Path) -> io::Result<()> {
        fs::create_dir_all(data_folder)?;
        let text = serde_norway::to_string(self).map_err(io::Error::other)?;
        let temporary = data_folder.join(format!("{FILE_NAME}.tmp"));
        fs::write(&temporary, text)?;
        fs::rename(&temporary, data_folder.join(FILE_NAME))
    }

    /// Notes that the workspace at `path` was opened: it goes to the top of
    /// the list (once, however the path is written) and is reopened on the
    /// next launch.
    pub fn remember(&mut self, path: &Path) {
        let path = resolved_path(path);
        self.workspaces.retain(|known| !same_folder(known, &path));
        self.workspaces.insert(0, path.clone());
        self.last_workspace = Some(path);
    }

    /// Takes the workspace at `path` off the list (its folder is left as it
    /// is); `false` if it was not on it. If it was the one to reopen, none
    /// is reopened. `open` is the workspace open now, which stays.
    pub fn forget(&mut self, path: &Path, open: Option<&Path>) -> Result<bool, ForgetError> {
        if open.is_some_and(|open| same_folder(open, path)) {
            return Err(ForgetError::Open);
        }
        let before = self.workspaces.len();
        self.workspaces.retain(|known| !same_folder(known, path));
        if self
            .last_workspace
            .as_deref()
            .is_some_and(|last| same_folder(last, path))
        {
            self.last_workspace = None;
        }
        Ok(self.workspaces.len() != before)
    }

    /// The list, most recently opened first; `open` is the workspace open
    /// now, if any.
    pub fn known_workspaces(&self, open: Option<&Path>) -> Vec<KnownWorkspace> {
        self.workspaces
            .iter()
            .map(|path| KnownWorkspace {
                path: path.clone(),
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                open: open.is_some_and(|open| same_folder(open, path)),
                available: path.is_dir(),
            })
            .collect()
    }
}

/// Whether `a` and `b` name the same folder: the same [`resolved_path`].
pub fn same_folder(a: &Path, b: &Path) -> bool {
    resolved_path(a) == resolved_path(b)
}

/// The one way to write the folder at `path`: resolved (case, `.`, `..`,
/// links) if it is there, else in full. Windows' `\\?\C:\...` form of a
/// resolved path is given as plain `C:\...`.
pub fn resolved_path(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(resolved) => {
            let text = resolved.display().to_string();
            match text.strip_prefix(r"\\?\") {
                Some(plain) if plain.as_bytes().get(1) == Some(&b':') => PathBuf::from(plain),
                _ => resolved,
            }
        }
        Err(_) => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}
