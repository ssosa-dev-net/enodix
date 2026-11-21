//! When the open workspace's folder is not the workspace shown (#132): it
//! cannot be reached (a network drive disconnected, the folder moved or
//! removed), or it came back as another workspace. What was loaded is
//! still shown, and every edit is refused with the reason, so that nothing
//! is written into a folder that is not there or not this workspace.

use std::fmt;
use std::io;
use std::path::Path;

use serde::Serialize;

use super::{EditError, Workspace, WORKSPACE_FILE};

/// Why the workspace shown is not what its folder holds now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Unavailable {
    /// The folder cannot be read: `reason` says what the system found.
    Unreachable { reason: String },
    /// The folder holds another workspace: its [`WORKSPACE_FILE`] names
    /// `id` (`None`: it has none, or one that cannot be read), not the id
    /// of the one that was open.
    OtherWorkspace { id: Option<String> },
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable { reason } => {
                write!(f, "the workspace folder cannot be reached ({reason})")
            }
            Self::OtherWorkspace { id: Some(id) } => write!(
                f,
                "the workspace folder is now another workspace (its {WORKSPACE_FILE} names {id})"
            ),
            Self::OtherWorkspace { id: None } => write!(
                f,
                "the workspace folder is now another workspace (it has no {WORKSPACE_FILE} that names this one)"
            ),
        }
    }
}

impl Unavailable {
    /// What happens meanwhile and what to do, for the window.
    pub fn explanation(&self) -> &'static str {
        match self {
            Self::Unreachable { .. } => {
                "What was loaded is still shown and nothing can be saved until the folder is \
                 back; it loads again by itself as soon as it is."
            }
            Self::OtherWorkspace { .. } => {
                "Credential profiles, kept job runs and config history belong to a workspace's \
                 id, so the folder is not shown in place of the one that was open, and nothing \
                 can be saved into it. To use it as the workspace it is now, open it again from \
                 the workspaces."
            }
        }
    }

    /// The folder at `root` cannot be read, the system said `error`.
    pub(super) fn unreachable(root: &Path, error: &io::Error) -> Self {
        let reason = match error.kind() {
            io::ErrorKind::NotFound => {
                "it is not there: a network drive that disconnected, or a folder moved or removed"
                    .to_string()
            }
            _ if root.is_file() => "a file stands at its path".to_string(),
            _ => error.to_string(),
        };
        Self::Unreachable { reason }
    }
}

impl Workspace {
    /// Why the workspace shown is not what its folder holds now, if so
    /// (set by [`Self::refresh`], cleared by the next one that loads it).
    pub fn unavailable(&self) -> Option<&Unavailable> {
        self.unavailable.as_ref()
    }

    /// Refuses an edit while the folder cannot be reached or is another
    /// workspace, with that reason; checks the folder is there first, so an
    /// edit before any refresh noticed is refused alike.
    pub(super) fn available(&self) -> Result<(), EditError> {
        if let Some(unavailable) = &self.unavailable {
            return Err(EditError::Unavailable(unavailable.clone()));
        }
        match std::fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(EditError::Unavailable(Unavailable::Unreachable {
                reason: "a file stands at its path".into(),
            })),
            Err(error) => Err(EditError::Unavailable(Unavailable::unreachable(
                &self.root, &error,
            ))),
        }
    }

    /// Takes `unavailable` as the workspace's state; a change of it is a
    /// new revision, so that the window shows it.
    pub(super) fn set_unavailable(&mut self, unavailable: Option<Unavailable>) {
        if self.unavailable != unavailable {
            self.unavailable = unavailable;
            self.revision += 1;
        }
    }
}
