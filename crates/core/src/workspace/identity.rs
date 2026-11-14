//! A workspace's identity: a stable identifier kept in [`WORKSPACE_FILE`]
//! at its root, so that it stays the same workspace wherever its folder is
//! moved or cloned to. Credential stores keep a workspace's profiles under
//! it ([`Workspace::credential_scope`]), so two workspaces never share a
//! profile even when both have one of the same name.
//!
//! ```yaml
//! id: 3f2b8c4e-0d7a-4f5e-9b1c-2a6d8e0f4b37
//! ```
//!
//! The identifier is no secret: the file is meant to be committed with the
//! rest of the workspace. A copy of a workspace folder has the same
//! identifier, and so shares its profiles on each computer, until its file
//! is deleted (a new identifier is then given the next time it is opened).

use std::fs;
use std::path::Path;

use serde_norway::{Mapping, Value};

use super::{io_error, read_mapping, EditError, OpenError, Problem, Workspace};

/// The file at the workspace root that holds its identifier.
pub const WORKSPACE_FILE: &str = "_workspace.yaml";

/// The key of the identifier in [`WORKSPACE_FILE`].
const ID: &str = "id";

/// The longest identifier accepted, in characters.
const MAX_ID: usize = 64;

/// What a new identifier file holds, before its `id` line.
const HEADER: &str = "\
# This workspace's identifier. Credential profiles are kept on each
# computer under it, so keep this file with the workspace. Delete it in a
# copy of a workspace to make the copy a workspace of its own.
";

/// The identifier in `root`'s [`WORKSPACE_FILE`], if it has a valid one;
/// a file that cannot be read or holds a bad identifier is a [`Problem`].
pub(super) fn load_id(root: &Path, problems: &mut Vec<Problem>) -> Option<String> {
    let path = root.join(WORKSPACE_FILE);
    if !path.is_file() {
        return None;
    }
    match read_mapping(&path).and_then(|document| id_in(&document)) {
        Ok(id) => id,
        Err(message) => {
            problems.push(Problem {
                file: WORKSPACE_FILE.into(),
                message,
            });
            None
        }
    }
}

/// The identifier a [`WORKSPACE_FILE`] mapping holds, if any.
fn id_in(document: &Mapping) -> Result<Option<String>, String> {
    match document.get(ID) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if valid(id) => Ok(Some(id.clone())),
        Some(_) => Err(format!(
            "`{ID}` must be up to {MAX_ID} letters, digits, `-`, `_` or `.`"
        )),
    }
}

fn valid(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Workspace {
    /// Creates a new, empty workspace in the folder `root` (created if it is
    /// not there, with its parents) and opens it. The folder must hold
    /// nothing but hidden entries (such as `.git`): an existing workspace or
    /// other files are opened rather than created over.
    pub fn create(root: impl AsRef<Path>) -> Result<Self, OpenError> {
        let root = root.as_ref();
        let refuse = |reason: String| OpenError {
            path: root.to_path_buf(),
            reason,
        };
        if root.exists() {
            if !root.is_dir() {
                return Err(refuse("it is a file, not a folder".into()));
            }
            let entries = fs::read_dir(root).map_err(|error| refuse(error.to_string()))?;
            for entry in entries {
                let entry = entry.map_err(|error| refuse(error.to_string()))?;
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    return Err(refuse(
                        "the folder is not empty; add it as an existing folder instead".into(),
                    ));
                }
            }
        } else {
            fs::create_dir_all(root).map_err(|error| refuse(error.to_string()))?;
        }
        write_new_id_file(root).map_err(|error| refuse(error.to_string()))?;
        Self::open(root)
    }

    /// The workspace's identifier, from its [`WORKSPACE_FILE`]; `None` until
    /// it has one (see [`Self::ensure_id`]).
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Gives the workspace an identifier if it has none yet, writing it to
    /// its [`WORKSPACE_FILE`] (other keys there are kept). A file that
    /// cannot be read or holds a bad identifier is never written over: the
    /// user fixes or deletes it.
    pub fn ensure_id(&mut self) -> Result<&str, EditError> {
        if self.id.is_none() {
            let path = self.root.join(WORKSPACE_FILE);
            if path.exists() {
                let document = read_mapping(&path).and_then(|document| {
                    id_in(&document)?;
                    Ok(document)
                });
                if let Err(message) = document {
                    return Err(EditError::Invalid(format!(
                        "{WORKSPACE_FILE} cannot be updated: {message}"
                    )));
                }
                let id = new_id();
                self.edit_file(WORKSPACE_FILE.into(), |document| {
                    document.insert(ID.into(), Value::String(id));
                    Ok(())
                })?;
            } else {
                write_new_id_file(&self.root).map_err(io_error(WORKSPACE_FILE))?;
                self.reload()?;
            }
        }
        self.id.as_deref().ok_or_else(|| {
            EditError::Invalid(format!(
                "{WORKSPACE_FILE} was written but cannot be read back"
            ))
        })
    }

    /// What a credential store kept this workspace's profiles under before
    /// workspaces had identifiers: the folder's full path, the same however
    /// it was opened (relative, different case on Windows, through a
    /// symbolic link).
    pub fn path_credential_scope(&self) -> String {
        crate::app_state::resolved_path(&self.root)
            .display()
            .to_string()
    }
}

/// Writes a [`WORKSPACE_FILE`] with a fresh identifier into `root`.
fn write_new_id_file(root: &Path) -> std::io::Result<()> {
    let path = root.join(WORKSPACE_FILE);
    let temporary = root.join(format!(".{WORKSPACE_FILE}.tmp"));
    fs::write(&temporary, format!("{HEADER}{ID}: {}\n", new_id()))
        .and_then(|()| fs::rename(&temporary, &path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
}
