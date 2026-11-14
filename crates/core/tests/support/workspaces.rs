//! Fixture workspaces: folders under `tests/fixtures/workspaces/`, opened in
//! place or copied to a temporary folder for tests that write.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// The fixture workspace folder called `name`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/workspaces")
        .join(name)
}

/// A writable copy of a fixture workspace. Keep the `TempDir` alive for as
/// long as the copy is used.
pub fn fixture_copy(name: &str) -> (TempDir, PathBuf) {
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let temp = TempDir::new().unwrap();
    let root = temp.path().join(name);
    copy(&fixture(name), &root);
    (temp, root)
}

/// Every file under `root`, as sorted `/`-separated relative paths, folders
/// ending in `/` (so empty folders show too).
pub fn files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                out.push(format!("{name}/"));
                walk(&entry.path(), &format!("{name}/"), out);
            } else {
                out.push(name);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out.sort();
    out
}

/// The text of the file at `path` under `root`.
pub fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}
