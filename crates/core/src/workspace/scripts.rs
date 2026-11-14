//! Script files: the Python scripts kept in the workspace folder, which the
//! user can run as script jobs (see [`crate::job`]).
//!
//! A script is any `*.py` file in the workspace folder or below it, named
//! by its `/`-separated path relative to the root, as hosts are. They are
//! looked for on disk each time (they are not part of the loaded tree).
//! Dot-entries, symlinked folders, `__pycache__` and virtual environments
//! (a folder holding `pyvenv.cfg`) are not looked into.

use std::fs;
use std::path::{Component, Path, PathBuf};

use super::Workspace;

/// The extension of script files.
pub const SCRIPT_EXTENSION: &str = "py";

/// Why a script path names no script of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    /// Not a plain relative path inside the workspace (`..`, absolute,
    /// a dot-entry), or not a `.py` file.
    #[error(
        "{0:?} is not a script path inside the workspace (a .py file, relative to its folder)"
    )]
    BadPath(String),
    /// No such file.
    #[error("there is no script {0} in the workspace")]
    NotFound(String),
}

impl Workspace {
    /// The workspace's scripts, by path relative to the root, sorted
    /// ignoring case.
    pub fn scripts(&self) -> Vec<String> {
        let mut found = Vec::new();
        collect(self.root(), "", &mut found);
        found.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
        found
    }

    /// The file of the script at `path` (relative, `/` or `\` separated),
    /// if it is one [`Workspace::scripts`] lists: a `.py` file inside the
    /// workspace folder, outside dot-entries, `__pycache__` and virtual
    /// environments, reached through no link.
    pub fn script_file(&self, path: &str) -> Result<PathBuf, ScriptError> {
        let normal = path.trim().replace('\\', "/");
        let relative = Path::new(&normal);
        let plain = !normal.is_empty()
            && relative.components().all(|part| match part {
                Component::Normal(name) => !name.to_string_lossy().starts_with('.'),
                _ => false,
            });
        if !plain || !is_script(relative) {
            return Err(ScriptError::BadPath(path.to_string()));
        }
        let file = self.root().join(relative);
        // No part of the way may be a link out of the workspace, or a
        // folder the listing does not look into.
        let mut walked = self.root().to_path_buf();
        let parts: Vec<_> = relative.components().collect();
        for (index, part) in parts.iter().enumerate() {
            walked.push(part);
            let link =
                fs::symlink_metadata(&walked).is_ok_and(|meta| meta.file_type().is_symlink());
            let folder = index + 1 < parts.len();
            if link || (folder && skipped_folder(&walked)) {
                return Err(ScriptError::BadPath(path.to_string()));
            }
        }
        if file.is_file() {
            Ok(file)
        } else {
            Err(ScriptError::NotFound(normal))
        }
    }
}

fn collect(dir: &Path, prefix: &str, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if kind.is_dir() {
            let inside = entry.path();
            if !skipped_folder(&inside) {
                collect(&inside, &path, found);
            }
        } else if kind.is_file() && is_script(Path::new(&name)) {
            found.push(path);
        }
    }
}

/// Whether `path` names a `.py` file.
fn is_script(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(SCRIPT_EXTENSION))
}

/// Whether the folder `dir` holds no scripts of the workspace: Python's
/// cache, or a virtual environment.
fn skipped_folder(dir: &Path) -> bool {
    dir.file_name().is_some_and(|name| name == "__pycache__") || dir.join("pyvenv.cfg").exists()
}
