//! Telling a workspace folder from a folder of other things before opening
//! it (#131): opening writes `_workspace.yaml`, so a folder that holds
//! other things and nothing of a workspace (a Documents folder chosen by
//! mistake) is asked about first. Deciding writes nothing.

use std::fs;
use std::path::Path;

use enodix_core::workspace::{folder_kind, FolderKind, Workspace, WORKSPACE_FILE};
use tempfile::TempDir;

/// Every entry under `root`, recursively, with its contents, so a test can
/// check nothing was written.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, prefix: &str, found: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                found.push((format!("{name}/"), Vec::new()));
                walk(&entry.path(), &format!("{name}/"), found);
            } else {
                found.push((name, fs::read(entry.path()).unwrap()));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, "", &mut found);
    found
}

fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn kind(root: &Path) -> FolderKind {
    let before = snapshot(root);
    let kind = folder_kind(root).unwrap();
    assert_eq!(snapshot(root), before, "deciding wrote into the folder");
    kind
}

#[test]
fn a_folder_with_host_files_is_a_workspace() {
    let root = TempDir::new().unwrap();
    write(root.path(), "r1.yaml", "address: 10.0.0.1\n");
    write(root.path(), "notes.txt", "my notes\n");

    assert_eq!(kind(root.path()), FolderKind::Workspace);
}

#[test]
fn host_files_only_in_subfolders_make_a_workspace() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "Europe/Madrid/core-r1.yml",
        "address: 10.1.0.1\n",
    );
    write(root.path(), "Europe/readme.md", "sites\n");

    assert_eq!(kind(root.path()), FolderKind::Workspace);
}

#[test]
fn a_folder_with_a_workspace_file_and_nothing_else_is_a_workspace() {
    let root = TempDir::new().unwrap();
    write(root.path(), WORKSPACE_FILE, "id: abc\n");

    assert_eq!(kind(root.path()), FolderKind::Workspace);
}

#[test]
fn each_workspace_wide_file_and_a_folder_settings_file_count_as_a_workspace() {
    for file in [
        "_roles.yaml",
        "_filters.yaml",
        "_platforms.yaml",
        "_snippets.yaml",
        "_highlights.yaml",
        "_jobs.yaml",
        "_folder.yaml",
        "Lab/_folder.yaml",
    ] {
        let root = TempDir::new().unwrap();
        write(root.path(), file, "\n");
        write(root.path(), "report.docx", "not a host");
        assert_eq!(kind(root.path()), FolderKind::Workspace, "{file}");
    }
}

#[test]
fn an_empty_folder_is_empty() {
    let root = TempDir::new().unwrap();

    assert_eq!(kind(root.path()), FolderKind::Empty);
}

#[test]
fn a_folder_with_only_hidden_entries_is_empty() {
    let root = TempDir::new().unwrap();
    write(root.path(), ".git/HEAD", "ref: refs/heads/main\n");
    write(root.path(), ".gitignore", "*.log\n");

    assert_eq!(kind(root.path()), FolderKind::Empty);
}

#[test]
fn a_folder_with_only_empty_subfolders_is_empty() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("Europe").join("Madrid")).unwrap();
    fs::create_dir(root.path().join("Lab")).unwrap();
    write(root.path(), "Lab/.keep", "");

    assert_eq!(kind(root.path()), FolderKind::Empty);
}

#[test]
fn a_folder_of_documents_is_other() {
    let root = TempDir::new().unwrap();
    write(root.path(), "letter.docx", "Dear ...");
    write(root.path(), "Photos/beach.jpg", "jpeg");

    assert_eq!(kind(root.path()), FolderKind::Other);
}

#[test]
fn only_subfolders_holding_other_things_is_other() {
    let root = TempDir::new().unwrap();
    write(root.path(), "project-a/src/main.rs", "fn main() {}\n");
    write(root.path(), "project-b/README.md", "b\n");

    assert_eq!(kind(root.path()), FolderKind::Other);
}

#[test]
fn yaml_files_that_are_not_hosts_are_other() {
    let root = TempDir::new().unwrap();
    write(
        root.path(),
        "docker-compose.yml",
        "services:\n  web:\n    image: nginx\n",
    );
    write(root.path(), ".github/workflows/ci.yaml", "on: push\n");
    write(root.path(), "_config.yml", "title: my site\n");
    write(root.path(), "list.yaml", "- a\n- b\n");
    write(root.path(), "broken.yaml", "a: [\n");
    write(root.path(), "blank.yaml", "");

    assert_eq!(kind(root.path()), FolderKind::Other);
}

#[test]
fn a_folder_that_is_not_there_or_a_file_is_refused_as_opening_is() {
    let parent = TempDir::new().unwrap();
    let missing = parent.path().join("missing");
    let error = folder_kind(&missing).unwrap_err();
    assert!(error.reason.contains("there is no such folder"), "{error}");
    assert!(!missing.exists());

    write(parent.path(), "hosts.csv", "name,address\n");
    let error = folder_kind(parent.path().join("hosts.csv")).unwrap_err();
    assert!(error.reason.contains("not a folder"), "{error}");
}

#[test]
fn opening_a_folder_writes_nothing_into_it_only_preparing_it_does() {
    // What the headless binary relies on: it opens, never prepares.
    let root = TempDir::new().unwrap();
    write(root.path(), "letter.docx", "Dear ...");
    let before = snapshot(root.path());

    let workspace = Workspace::open(root.path()).unwrap();

    assert!(workspace.tree().hosts.is_empty());
    assert_eq!(snapshot(root.path()), before);
    assert!(!root.path().join(WORKSPACE_FILE).exists());
}
