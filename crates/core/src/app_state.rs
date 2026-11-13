//! App-level state: what the app remembers between launches (for now, the
//! last opened workspace). It lives in the per-user application data folder,
//! never in a workspace. The caller chooses that folder; the core assumes
//! nothing about the platform.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The state file's name inside the data folder.
const FILE_NAME: &str = "state.yaml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    /// The workspace to reopen on launch.
    pub last_workspace: Option<PathBuf>,
}

impl AppState {
    /// Reads the state kept in `data_folder`. A missing or unreadable file
    /// gives the default state: losing it only costs the user a reopen.
    pub fn load(data_folder: &Path) -> Self {
        fs::read_to_string(data_folder.join(FILE_NAME))
            .ok()
            .and_then(|text| serde_norway::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the state to `data_folder`, creating the folder if needed.
    pub fn save(&self, data_folder: &Path) -> io::Result<()> {
        fs::create_dir_all(data_folder)?;
        let text = serde_norway::to_string(self).map_err(io::Error::other)?;
        let temporary = data_folder.join(format!("{FILE_NAME}.tmp"));
        fs::write(&temporary, text)?;
        fs::rename(&temporary, data_folder.join(FILE_NAME))
    }
}
