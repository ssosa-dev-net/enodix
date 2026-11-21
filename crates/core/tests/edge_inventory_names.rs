//! Edge tests (#117): host and folder names with unusual characters. What
//! the app creates must be a name that works on every system (a workspace
//! moves between Windows and Linux through git); what it finds on disk it
//! must show and edit without mixing two hosts up.
//!
//! Each test says which system it speaks for: names Windows reserves,
//! trailing dots and case-only differences behave differently on Windows
//! (NTFS: case-insensitive, reserved device names) and on Linux (ext4:
//! case-sensitive, any name but `/` and NUL).

mod support;

use std::fs;

use netterm_core::inventory::{self, Limit};
use netterm_core::workspace::{EditError, FolderDeletion, Workspace};
use support::workspaces::files;
use tempfile::TempDir;

fn empty_workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("_workspace.yaml"), "id: edge-names\n").unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    (temp, workspace)
}

/// Windows and Linux.
#[test]
fn names_with_spaces_and_accents_are_created_found_edited_and_exported() {
    let (temp, mut workspace) = empty_workspace();
    let folder = workspace.create_folder("", "Sede Málaga").unwrap();
    let id = workspace
        .create_host(&folder, "  núcleo r1 (planta 2)  ", "10.0.0.1")
        .unwrap();
    assert_eq!(id.as_str(), "Sede Málaga/núcleo r1 (planta 2).yaml");

    let reopened = Workspace::open(temp.path()).unwrap();
    assert!(reopened.problems().is_empty(), "{:?}", reopened.problems());
    let host = reopened.host(&id).unwrap();
    assert_eq!(host.name, "núcleo r1 (planta 2)");
    let hits = reopened.search("NÚCLEO málaga", 10);
    assert_eq!(hits.total, 0, "the folder is not a field of the host");
    assert_eq!(reopened.search("NÚCLEO planta", 10).hits[0].id, id);

    let mut edited = host.clone();
    edited.description = Some("sala técnica".into());
    workspace.write_host(&edited).unwrap();
    let export = inventory::export(&workspace, &Limit::Hosts(vec![id.to_string()])).unwrap();
    assert_eq!(export.hosts[0].description.as_deref(), Some("sala técnica"));

    let moved = workspace.rename_host(&id, "núcleo r1 (antiguo)").unwrap();
    workspace.delete_host(&moved.to).unwrap();
    assert_eq!(files(temp.path()), ["Sede Málaga/", "_workspace.yaml"]);
}

/// Windows and Linux: the app refuses, on every system, the names Windows
/// cannot hold, so a workspace made on Linux still checks out on Windows.
#[test]
fn every_name_windows_reserves_is_refused_on_every_system() {
    let (temp, mut workspace) = empty_workspace();
    for name in [
        "CON",
        "con",
        "Nul",
        "aux",
        "PRN",
        "COM1",
        "com9",
        "LPT1",
        "lpt9",
        "nul.backup",
        "CON .old",
        // Superscript digits are reserved too, and the console's own names.
        "COM0",
        "lpt0",
        "COM¹",
        "com²",
        "COM³",
        "LPT¹",
        "lpt³",
        "CONIN$",
        "conout$",
        "trailing.",
        "trailing..",
        "dots...",
    ] {
        let error = workspace.create_host("", name, "10.0.0.1").unwrap_err();
        assert!(
            matches!(error, EditError::InvalidName { .. }),
            "{name:?}: {error}"
        );
        let error = workspace.create_folder("", name).unwrap_err();
        assert!(
            matches!(error, EditError::InvalidName { .. }),
            "{name:?}: {error}"
        );
    }
    // Names that only look like them are fine.
    for name in ["console", "COM10", "nullable", "lpt", "con-r1", "COM00"] {
        workspace.create_host("", name, "10.0.0.1").unwrap();
    }
    assert_eq!(files(temp.path()).len(), 7);
}

/// Windows and Linux: a name longer than a file system takes is refused
/// with the file system's reason, and nothing is written.
#[test]
fn a_name_too_long_for_the_file_system_is_refused_and_writes_nothing() {
    let (temp, mut workspace) = empty_workspace();
    let long = "r".repeat(300);
    let error = workspace.create_host("", &long, "10.0.0.1").unwrap_err();
    assert!(matches!(error, EditError::Io { .. }), "{error}");
    let error = workspace.create_folder("", &long).unwrap_err();
    assert!(matches!(error, EditError::Io { .. }), "{error}");
    assert_eq!(files(temp.path()), ["_workspace.yaml"]);
}

/// Windows (past the old 260-character path limit) and Linux.
#[test]
fn a_host_whose_path_is_longer_than_windows_old_limit_loads_and_is_edited() {
    let (temp, mut workspace) = empty_workspace();
    let mut folder = String::new();
    let mut top = String::new();
    for level in 0..6 {
        folder = workspace
            .create_folder(&folder, &format!("level-{level}-{}", "x".repeat(50)))
            .unwrap();
        if level == 0 {
            top = folder.clone();
        }
    }
    let id = workspace
        .create_host(&folder, &format!("core-{}", "r".repeat(60)), "10.0.0.1")
        .unwrap();
    assert!(temp.path().join(id.as_str()).to_string_lossy().len() > 400);

    let reopened = Workspace::open(temp.path()).unwrap();
    assert!(reopened.problems().is_empty(), "{:?}", reopened.problems());
    let mut host = reopened.host(&id).unwrap().clone();
    host.description = Some("deep".into());
    workspace.write_host(&host).unwrap();
    let moved = workspace.move_host(&id, "").unwrap();
    assert!(workspace.host(&moved.to).is_some());
    workspace
        .delete_folder(&top, FolderDeletion::IfNoHosts)
        .unwrap();
    assert_eq!(files(temp.path()).len(), 2);
}

/// Windows and Linux: changing only the case of a name is a rename like
/// any other, and leaves one host.
#[test]
fn a_rename_that_changes_only_the_case_leaves_one_host_of_the_new_name() {
    let (temp, mut workspace) = empty_workspace();
    let id = workspace.create_host("", "core-r1", "10.0.0.1").unwrap();
    let moved = workspace.rename_host(&id, "CORE-R1").unwrap();
    assert_eq!(moved.to.as_str(), "CORE-R1.yaml");
    assert_eq!(files(temp.path()), ["CORE-R1.yaml", "_workspace.yaml"]);
    let folder = workspace.create_folder("", "madrid").unwrap();
    let moved = workspace.rename_folder(&folder, "Madrid").unwrap();
    assert_eq!(moved.to, "Madrid");
    assert_eq!(
        files(temp.path()),
        ["CORE-R1.yaml", "Madrid/", "_workspace.yaml"]
    );
}

/// Windows and Linux (NTFS and ext4 both keep the two forms apart).
#[test]
#[ignore = "a name in its composed and its decomposed Unicode form is two hosts that look the \
            same: the app does not normalise names (ticket proposed in #117's report); run with --ignored"]
fn a_name_in_its_other_unicode_form_is_the_same_name() {
    let (_temp, mut workspace) = empty_workspace();
    let composed = "caf\u{e9}";
    let decomposed = "cafe\u{301}";
    workspace.create_host("", composed, "10.0.0.1").unwrap();
    let error = workspace
        .create_host("", decomposed, "10.0.0.2")
        .unwrap_err();
    assert!(matches!(error, EditError::AlreadyExists(_)), "{error}");
}

/// Linux only: a case-sensitive file system can hold two host files whose
/// names differ only in case (made outside the app, or pulled from a
/// branch made there). Both load, each keeps its own id, a reference that
/// names one exactly finds that one, and the app makes no third.
#[cfg(unix)]
#[test]
fn on_linux_two_files_differing_only_in_case_are_two_hosts_never_mixed_up() {
    use netterm_core::workspace::HostId;
    let (temp, _) = empty_workspace();
    let root = temp.path();
    fs::write(root.join("R1.yaml"), "address: 10.0.0.1\n").unwrap();
    fs::write(root.join("r1.yaml"), "address: 10.0.0.2\n").unwrap();
    fs::write(
        root.join("via-upper.yaml"),
        "address: 10.9.0.1\njump_host: R1.yaml\n",
    )
    .unwrap();
    fs::write(
        root.join("via-lower.yaml"),
        "address: 10.9.0.2\njump_host: r1.yaml\n",
    )
    .unwrap();
    let mut workspace = Workspace::open(root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );

    let jump = |workspace: &Workspace, id: &str| {
        let host = workspace.resolve_host(&HostId::new(id)).unwrap();
        workspace.jump_chain(&host).unwrap()[0].host.address.clone()
    };
    assert_eq!(jump(&workspace, "via-upper.yaml"), "10.0.0.1");
    assert_eq!(jump(&workspace, "via-lower.yaml"), "10.0.0.2");
    let export = inventory::export(&workspace, &Limit::Hosts(vec!["r1.yaml".into()])).unwrap();
    assert_eq!(export.hosts[0].address, "10.0.0.2");

    // Editing one leaves the other alone.
    let mut lower = workspace.host(&HostId::new("r1.yaml")).unwrap().clone();
    lower.description = Some("lower".into());
    workspace.write_host(&lower).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("R1.yaml")).unwrap(),
        "address: 10.0.0.1\n"
    );
    let error = workspace.create_host("", "r1", "10.0.0.3").unwrap_err();
    assert!(matches!(error, EditError::AlreadyExists(_)), "{error}");
}

/// Linux only: files Windows could not hold (a reserved name, a trailing
/// dot before the extension) load and can be edited where they exist; the
/// app only refuses to make new ones.
#[cfg(unix)]
#[test]
fn on_linux_files_with_names_windows_reserves_load_and_can_be_edited() {
    use netterm_core::workspace::HostId;
    let (temp, _) = empty_workspace();
    let root = temp.path();
    fs::write(root.join("CON.yaml"), "address: 10.0.0.1\n").unwrap();
    fs::write(root.join("r2..yaml"), "address: 10.0.0.2\n").unwrap();
    fs::write(root.join("what?.yaml"), "address: 10.0.0.3\n").unwrap();
    let mut workspace = Workspace::open(root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    for id in ["CON.yaml", "r2..yaml", "what?.yaml"] {
        let mut host = workspace.host(&HostId::new(id)).unwrap().clone();
        host.description = Some("edited".into());
        workspace.write_host(&host).unwrap();
    }
    let error = workspace
        .rename_host(&HostId::new("r2..yaml"), "NUL")
        .unwrap_err();
    assert!(matches!(error, EditError::InvalidName { .. }), "{error}");
}
