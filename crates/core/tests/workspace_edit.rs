//! Editing the inventory: each operation is checked by reading the
//! workspace files on disk afterwards, the way git or a colleague sees them.

mod support;

use std::fs;

use netterm_core::workspace::{CustomField, EditError, FolderDeletion, Host, HostId, Workspace};
use support::workspaces::{files, fixture_copy, read};
use tempfile::TempDir;

/// The `edit` fixture as a writable copy, opened.
fn edit_workspace() -> (TempDir, std::path::PathBuf, Workspace) {
    let (temp, root) = fixture_copy("edit");
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, root, workspace)
}

fn host(workspace: &Workspace, id: &str) -> Host {
    workspace
        .host(&HostId::new(id))
        .unwrap_or_else(|| panic!("no host {id}"))
        .clone()
}

const FIXTURE_FILES: [&str; 10] = [
    "Access/",
    "Access/notes.txt",
    "Access/sw1.yaml",
    "Core/",
    "Core/_folder.yaml",
    "Core/core-r1.yaml",
    "Core/core-r2.yml",
    "Empty/",
    "Empty/.gitkeep",
    "_folder.yaml",
];

const CORE_R1: &str = "\
name: core-r1
address: 10.0.0.1
vendor: Arista
role: core
rack_unit: 12
tags:
- core
- dc1
custom:
  contract: ACME-1234
  owner: noc
port: 22
";

#[test]
fn a_host_file_gives_its_fields_tags_and_custom_fields() {
    let (_temp, _root, workspace) = edit_workspace();

    let core = host(&workspace, "Core/core-r1.yaml");
    assert_eq!(core.vendor.as_deref(), Some("Arista"));
    assert_eq!(core.rack_unit.as_deref(), Some("12"));
    assert_eq!(core.settings.platform, None);
    assert_eq!(core.tags, ["core", "dc1"]);
    assert_eq!(
        core.custom,
        [
            CustomField::new("contract", "ACME-1234"),
            CustomField::new("owner", "noc")
        ]
    );
}

#[test]
fn malformed_tags_or_custom_fields_are_reported_for_that_file() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("a.yaml"), "address: 1.1.1.1\ntags: core\n").unwrap();
    fs::write(
        temp.path().join("b.yaml"),
        "address: 1.1.1.2\ncustom:\n  nested:\n    x: 1\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("c.yaml"),
        "address: 1.1.1.3\ncustom: [1]\n",
    )
    .unwrap();

    let workspace = Workspace::open(temp.path()).unwrap();

    let problems: Vec<String> = workspace.problems().iter().map(|p| p.to_string()).collect();
    assert_eq!(
        problems,
        [
            "a.yaml: `tags` must be a list of text",
            "b.yaml: `custom` must map names to text",
            "c.yaml: `custom` must map names to text",
        ]
    );
}

#[test]
fn creating_a_folder_and_a_host_writes_them_to_the_workspace() {
    let (_temp, root, mut workspace) = edit_workspace();

    let edge = workspace.create_folder("", "Edge").unwrap();
    assert_eq!(edge, "Edge");
    let nested = workspace.create_folder("Edge", "Lisbon").unwrap();
    assert_eq!(nested, "Edge/Lisbon");
    let id = workspace
        .create_host("Edge/Lisbon", "edge-r1", "10.9.0.1")
        .unwrap();

    assert_eq!(id, HostId::new("Edge/Lisbon/edge-r1.yaml"));
    assert_eq!(
        read(&root, "Edge/Lisbon/edge-r1.yaml"),
        "address: 10.9.0.1\n"
    );
    assert!(files(&root).contains(&"Edge/".to_string()));
    // The workspace shows the change without reopening.
    let created = host(&workspace, "Edge/Lisbon/edge-r1.yaml");
    assert_eq!(
        (created.name.as_str(), created.address.as_str()),
        ("edge-r1", "10.9.0.1")
    );
    assert_eq!(workspace.folder("Edge/Lisbon").unwrap().name, "Lisbon");
}

#[test]
fn a_name_already_taken_in_the_folder_is_refused() {
    let (_temp, root, mut workspace) = edit_workspace();

    // Case does not make a name distinct: workspaces move between systems.
    let error = workspace.create_folder("", "core").unwrap_err();
    assert!(matches!(error, EditError::AlreadyExists(_)), "{error}");
    // Nor does the extension: core-r2.yml is already host core-r2.
    let error = workspace
        .create_host("Core", "core-r2", "10.0.0.9")
        .unwrap_err();
    assert!(matches!(error, EditError::AlreadyExists(_)), "{error}");
    let error = workspace
        .rename_host(&HostId::new("Core/core-r1.yaml"), "CORE-R2")
        .unwrap_err();
    assert!(matches!(error, EditError::AlreadyExists(_)), "{error}");
    assert_eq!(files(&root), FIXTURE_FILES);

    // Once moved away, the name is free again.
    workspace.move_folder("Core", "Empty").unwrap();
    workspace.create_folder("", "Core").unwrap();
    assert!(files(&root).contains(&"Empty/Core/core-r2.yml".to_string()));
}

#[test]
fn names_that_are_not_portable_file_names_are_refused() {
    let (_temp, root, mut workspace) = edit_workspace();

    for name in [
        "",
        "  ",
        "a/b",
        "a\\b",
        "..",
        ".hidden",
        "_reserved",
        "what?",
        "con",
        "LPT1",
        "trailing.",
    ] {
        let error = workspace.create_folder("", name).unwrap_err();
        assert!(
            matches!(error, EditError::InvalidName { .. }),
            "{name:?}: {error}"
        );
        let error = workspace.create_host("", name, "10.0.0.1").unwrap_err();
        assert!(
            matches!(error, EditError::InvalidName { .. }),
            "{name:?}: {error}"
        );
    }
    assert_eq!(files(&root), FIXTURE_FILES);
}

#[test]
fn creating_or_moving_into_a_folder_outside_the_workspace_is_refused() {
    let (_temp, root, mut workspace) = edit_workspace();

    let error = workspace.create_folder("..", "Escape").unwrap_err();
    assert!(matches!(error, EditError::NotFound(_)), "{error}");
    let error = workspace
        .move_host(&HostId::new("Access/sw1.yaml"), "../..")
        .unwrap_err();
    assert!(matches!(error, EditError::NotFound(_)), "{error}");
    let error = workspace
        .delete_host(&HostId::new("../edit/Access/sw1.yaml"))
        .unwrap_err();
    assert!(matches!(error, EditError::NotFound(_)), "{error}");

    assert_eq!(files(&root), FIXTURE_FILES);
    assert!(!root.parent().unwrap().join("Escape").exists());
}

#[test]
fn renaming_a_folder_renames_its_directory_and_keeps_its_contents() {
    let (_temp, root, mut workspace) = edit_workspace();

    let path = workspace.rename_folder("Core", "Backbone").unwrap().to;

    assert_eq!(path, "Backbone");
    assert_eq!(read(&root, "Backbone/core-r1.yaml"), CORE_R1);
    assert_eq!(
        read(&root, "Backbone/_folder.yaml"),
        "description: Core layer\n"
    );
    assert!(!root.join("Core").exists());
    assert!(workspace
        .host(&HostId::new("Backbone/core-r2.yml"))
        .is_some());
    assert!(workspace.host(&HostId::new("Core/core-r2.yml")).is_none());

    // Changing only the case is a rename too.
    assert_eq!(
        workspace.rename_folder("Backbone", "backbone").unwrap().to,
        "backbone"
    );
    assert!(files(&root).contains(&"backbone/".to_string()));
}

#[test]
fn renaming_a_host_renames_its_file_and_its_name() {
    let (_temp, root, mut workspace) = edit_workspace();

    // A host with a `name` key: the key follows the new name.
    let id = workspace
        .rename_host(&HostId::new("Core/core-r1.yaml"), "spine-1")
        .unwrap()
        .to;
    assert_eq!(id, HostId::new("Core/spine-1.yaml"));
    assert_eq!(
        read(&root, "Core/spine-1.yaml"),
        CORE_R1.replace("name: core-r1", "name: spine-1")
    );

    // A host named by its file stays that way, and keeps its extension.
    let id = workspace
        .rename_host(&HostId::new("Core/core-r2.yml"), "spine-2")
        .unwrap()
        .to;
    assert_eq!(id, HostId::new("Core/spine-2.yml"));
    assert_eq!(read(&root, "Core/spine-2.yml"), "address: 10.0.0.2\n");
    assert_eq!(host(&workspace, "Core/spine-2.yml").name, "spine-2");

    assert_eq!(
        files(&root)
            .into_iter()
            .filter(|f| f.starts_with("Core/") && f != "Core/")
            .collect::<Vec<_>>(),
        ["Core/_folder.yaml", "Core/spine-1.yaml", "Core/spine-2.yml"]
    );
}

#[test]
fn moving_a_host_moves_its_file_unchanged() {
    let (_temp, root, mut workspace) = edit_workspace();

    let id = workspace
        .move_host(&HostId::new("Core/core-r1.yaml"), "Access")
        .unwrap()
        .to;

    assert_eq!(id, HostId::new("Access/core-r1.yaml"));
    assert_eq!(read(&root, "Access/core-r1.yaml"), CORE_R1);
    assert!(!root.join("Core/core-r1.yaml").exists());
    assert_eq!(workspace.folder("Access").unwrap().hosts.len(), 2);

    // To the root, and to where it already is.
    let id = workspace.move_host(&id, "").unwrap().to;
    assert_eq!(id, HostId::new("core-r1.yaml"));
    assert_eq!(workspace.move_host(&id, "").unwrap().to, id);
    assert_eq!(read(&root, "core-r1.yaml"), CORE_R1);
}

#[test]
fn moving_a_folder_moves_its_whole_subtree() {
    let (_temp, root, mut workspace) = edit_workspace();

    let path = workspace.move_folder("Access", "Core").unwrap().to;

    assert_eq!(path, "Core/Access");
    assert_eq!(read(&root, "Core/Access/sw1.yaml"), "address: 10.0.2.1\n");
    assert_eq!(read(&root, "Core/Access/notes.txt"), "not a host\n");
    assert!(!root.join("Access").exists());
    assert!(workspace
        .host(&HostId::new("Core/Access/sw1.yaml"))
        .is_some());

    // Back to the root.
    assert_eq!(
        workspace.move_folder("Core/Access", "").unwrap().to,
        "Access"
    );
    assert!(root.join("Access/sw1.yaml").exists());
}

#[test]
fn a_folder_cannot_be_moved_into_itself_or_below_itself() {
    let (_temp, root, mut workspace) = edit_workspace();
    workspace.create_folder("Core", "Inner").unwrap();

    for target in ["Core", "Core/Inner"] {
        let error = workspace.move_folder("Core", target).unwrap_err();
        assert!(matches!(error, EditError::Invalid(_)), "{target}: {error}");
    }
    let error = workspace.move_folder("", "Core").unwrap_err();
    assert!(matches!(error, EditError::Invalid(_)), "{error}");
    assert!(root.join("Core/Inner").is_dir());
    assert!(root.join("Core/core-r1.yaml").is_file());
}

#[test]
fn deleting_a_host_removes_its_file() {
    let (_temp, root, mut workspace) = edit_workspace();

    workspace
        .delete_host(&HostId::new("Core/core-r2.yml"))
        .unwrap();

    assert!(!root.join("Core/core-r2.yml").exists());
    assert!(workspace.host(&HostId::new("Core/core-r2.yml")).is_none());
    let error = workspace
        .delete_host(&HostId::new("Core/core-r2.yml"))
        .unwrap_err();
    assert!(matches!(error, EditError::NotFound(_)), "{error}");
}

#[test]
fn deleting_a_folder_with_hosts_needs_confirmation() {
    let (_temp, root, mut workspace) = edit_workspace();
    workspace.create_folder("Access", "Floor2").unwrap();
    workspace
        .create_host("Access/Floor2", "sw2", "10.0.2.2")
        .unwrap();

    // Hosts in subfolders count too.
    let error = workspace
        .delete_folder("Access", FolderDeletion::IfNoHosts)
        .unwrap_err();
    assert!(
        matches!(error, EditError::FolderHasHosts { hosts: 2, .. }),
        "{error}"
    );
    assert_eq!(error.to_string(), "Access contains 2 hosts");
    assert!(root.join("Access/sw1.yaml").exists());

    workspace
        .delete_folder("Access", FolderDeletion::IncludingHosts)
        .unwrap();
    assert!(!root.join("Access").exists());
    assert!(workspace.folder("Access").is_none());

    // A folder without hosts goes without asking.
    workspace
        .delete_folder("Empty", FolderDeletion::IfNoHosts)
        .unwrap();
    assert!(!root.join("Empty").exists());

    // The workspace itself cannot be deleted from inside.
    let error = workspace
        .delete_folder("", FolderDeletion::IncludingHosts)
        .unwrap_err();
    assert!(matches!(error, EditError::Invalid(_)), "{error}");
    assert!(root.join("Core/core-r1.yaml").exists());
}

#[test]
fn edited_fields_are_saved_in_a_fixed_key_order_keeping_everything_else() {
    let (_temp, root, mut workspace) = edit_workspace();
    let mut core = host(&workspace, "Core/core-r1.yaml");

    core.settings.platform = Some("EOS".into());
    core.model = Some(" 7280R3 ".into());
    core.serial_number = Some("0012345".into());
    core.site = Some("MAD1".into());
    core.rack = Some("A1".into());
    core.description = Some("Spine switch\nreplaced in 2026".into());
    workspace.write_host(&core).unwrap();

    // New keys go right after the nearest built-in key before them that
    // the file has: `description` after `role`, which this file holds out
    // of order, before `rack_unit`. Untouched values keep their place and
    // form (`rack_unit` stays a number).
    assert_eq!(
        read(&root, "Core/core-r1.yaml"),
        "\
name: core-r1
address: 10.0.0.1
vendor: Arista
platform: EOS
model: 7280R3
serial_number: '0012345'
site: MAD1
rack: A1
role: core
description: |-
  Spine switch
  replaced in 2026
rack_unit: 12
tags:
- core
- dc1
custom:
  contract: ACME-1234
  owner: noc
port: 22
"
    );
    let saved = host(&workspace, "Core/core-r1.yaml");
    assert_eq!(saved.model.as_deref(), Some("7280R3"));
    assert_eq!(saved.serial_number.as_deref(), Some("0012345"));
}

#[test]
fn every_built_in_field_is_written_and_read_back() {
    let (_temp, root, mut workspace) = edit_workspace();
    let mut sw = host(&workspace, "Access/sw1.yaml");

    sw.address = "sw1.example.net".into();
    sw.vendor = Some("Cisco".into());
    sw.settings.platform = Some("IOS-XE".into());
    sw.model = Some("C9300-48P".into());
    sw.serial_number = Some("FOC1234X0AB".into());
    sw.site = Some("Madrid".into());
    sw.building = Some("B2".into());
    sw.room = Some("2.14".into());
    sw.rack = Some("R3".into());
    sw.rack_unit = Some("40".into());
    sw.description = Some("Access, floor 2".into());
    sw.tags = vec!["access".into(), "poe".into()];
    sw.custom = vec![CustomField::new("asset", "A-991")];
    workspace.write_host(&sw).unwrap();

    assert_eq!(
        read(&root, "Access/sw1.yaml"),
        "\
address: sw1.example.net
vendor: Cisco
platform: IOS-XE
model: C9300-48P
serial_number: FOC1234X0AB
site: Madrid
building: B2
room: '2.14'
rack: R3
rack_unit: 40
description: Access, floor 2
tags:
- access
- poe
custom:
  asset: A-991
"
    );
    let reopened = Workspace::open(&root).unwrap();
    let read_back = host(&reopened, "Access/sw1.yaml");
    assert_eq!(read_back, host(&workspace, "Access/sw1.yaml"));
    assert_eq!(read_back.room.as_deref(), Some("2.14"));
    assert_eq!(read_back.rack_unit.as_deref(), Some("40"));

    // A rack unit with leading zeros stays as typed.
    let mut sw = read_back;
    sw.rack_unit = Some("07".into());
    workspace.write_host(&sw).unwrap();
    assert!(read(&root, "Access/sw1.yaml").contains("rack_unit: '07'\n"));
    assert_eq!(
        host(&workspace, "Access/sw1.yaml").rack_unit.as_deref(),
        Some("07")
    );
}

#[test]
fn clearing_fields_removes_their_keys() {
    let (_temp, root, mut workspace) = edit_workspace();
    let mut core = host(&workspace, "Core/core-r1.yaml");

    core.vendor = Some("   ".into());
    core.rack_unit = None;
    core.settings.port = None;
    core.tags.clear();
    core.custom.clear();
    workspace.write_host(&core).unwrap();

    assert_eq!(
        read(&root, "Core/core-r1.yaml"),
        "name: core-r1\naddress: 10.0.0.1\nrole: core\n"
    );
}

#[test]
fn tags_can_be_added_and_removed() {
    let (_temp, root, mut workspace) = edit_workspace();
    let mut core = host(&workspace, "Core/core-r1.yaml");

    // Blank and repeated tags are dropped.
    core.tags = vec!["core".into(), "spine".into(), " ".into(), "spine".into()];
    workspace.write_host(&core).unwrap();

    assert_eq!(
        read(&root, "Core/core-r1.yaml"),
        CORE_R1.replace("- dc1\n", "- spine\n")
    );
    assert_eq!(
        host(&workspace, "Core/core-r1.yaml").tags,
        ["core", "spine"]
    );
}

#[test]
fn custom_fields_can_be_added_edited_and_removed() {
    let (_temp, root, mut workspace) = edit_workspace();
    let mut core = host(&workspace, "Core/core-r1.yaml");

    core.custom = vec![
        CustomField::new("owner", "netops"),
        CustomField::new("circuit", "CID-77"),
        // An empty row, as a half-filled form leaves it, is dropped.
        CustomField::new("", ""),
    ];
    workspace.write_host(&core).unwrap();

    assert_eq!(
        read(&root, "Core/core-r1.yaml"),
        CORE_R1.replace(
            "  contract: ACME-1234\n  owner: noc\n",
            "  owner: netops\n  circuit: CID-77\n"
        )
    );
    assert_eq!(
        host(&workspace, "Core/core-r1.yaml").custom,
        [
            CustomField::new("owner", "netops"),
            CustomField::new("circuit", "CID-77")
        ]
    );
}

#[test]
fn invalid_edits_are_refused_and_the_file_is_untouched() {
    let (_temp, root, mut workspace) = edit_workspace();
    let original = host(&workspace, "Core/core-r1.yaml");

    let mut no_address = original.clone();
    no_address.address = " ".into();
    let mut same_key_twice = original.clone();
    same_key_twice.custom.push(CustomField::new("owner", "x"));
    let mut value_without_key = original.clone();
    value_without_key.custom.push(CustomField::new(" ", "x"));
    let mut port_in_address = original.clone();
    port_in_address.address = "10.0.0.1:2222".into();

    for (edit, message) in [
        (no_address, "the management address is empty"),
        (
            port_in_address,
            "write the address without the port (10.0.0.1), then set Port to 2222 in the host's details",
        ),
        (same_key_twice, "custom field `owner` appears twice"),
        (value_without_key, "a custom field has a value but no name"),
    ] {
        let error = workspace.write_host(&edit).unwrap_err();
        assert!(matches!(error, EditError::Invalid(_)), "{error}");
        assert_eq!(error.to_string(), message);
    }
    assert_eq!(read(&root, "Core/core-r1.yaml"), CORE_R1);
}

#[test]
fn a_new_host_with_a_port_after_its_address_is_refused_with_where_the_port_goes() {
    let (_temp, root, mut workspace) = edit_workspace();

    // How other tools write it; the port would be taken as part of the
    // name and the connection would fail as a name that does not resolve.
    for (address, message) in [
        (
            "10.0.0.9:2222",
            "write the address without the port (10.0.0.9), then set Port to 2222 in the host's details",
        ),
        (
            "edge1.example.net:22",
            "write the address without the port (edge1.example.net), then set Port to 22 in the host's details",
        ),
    ] {
        let error = workspace.create_host("", "edge1", address).unwrap_err();
        assert!(matches!(error, EditError::Invalid(_)), "{error}");
        assert_eq!(error.to_string(), message);
    }
    assert_eq!(files(&root), FIXTURE_FILES);

    // IPv6 addresses have colons of their own and are kept as written.
    for (name, address) in [
        ("v6-a", "2001:db8::1"),
        ("v6-b", "fe80::1"),
        ("v6-c", "::1"),
    ] {
        let id = workspace.create_host("", name, address).unwrap();
        assert_eq!(host(&workspace, id.as_str()).address, address);
    }
}

#[test]
fn an_empty_name_is_said_plainly() {
    let (_temp, _root, mut workspace) = edit_workspace();

    let error = workspace.create_host("", " ", "10.0.0.9").unwrap_err();

    assert_eq!(error.to_string(), "the name is empty");
}

#[test]
fn saving_an_unchanged_host_leaves_its_file_alone() {
    let (_temp, root) = fixture_copy("edit");
    let commented = "# access switch\naddress:   10.0.2.1   # mgmt\n";
    fs::write(root.join("Access/sw1.yaml"), commented).unwrap();
    let mut workspace = Workspace::open(&root).unwrap();

    let sw = host(&workspace, "Access/sw1.yaml");
    workspace.write_host(&sw).unwrap();

    assert_eq!(read(&root, "Access/sw1.yaml"), commented);
}

#[test]
fn saving_the_same_edit_twice_writes_the_same_bytes() {
    let (_temp, root, mut workspace) = edit_workspace();
    let mut core = host(&workspace, "Core/core-r1.yaml");
    core.site = Some("MAD1".into());
    core.custom.push(CustomField::new("note", "yes"));

    workspace.write_host(&core).unwrap();
    let first = read(&root, "Core/core-r1.yaml");
    let mut again = host(&workspace, "Core/core-r1.yaml");
    again.site = Some("MAD1".into());
    workspace.write_host(&again).unwrap();

    assert_eq!(read(&root, "Core/core-r1.yaml"), first);
    // `yes` stays text, not a boolean, when read back.
    assert!(first.contains("  note: yes\n"), "{first}");
    assert_eq!(
        host(&workspace, "Core/core-r1.yaml").custom.last(),
        Some(&CustomField::new("note", "yes"))
    );
}

#[test]
fn a_folder_description_is_written_to_its_settings_file() {
    let (_temp, root, mut workspace) = edit_workspace();

    // Other settings in the file are kept.
    workspace
        .set_folder_description("", Some("Acme Corp, all sites"))
        .unwrap();
    assert_eq!(
        read(&root, "_folder.yaml"),
        "description: Acme Corp, all sites\nowner: noc\n"
    );
    assert_eq!(
        workspace.tree().description.as_deref(),
        Some("Acme Corp, all sites")
    );

    // A folder without a settings file gets one.
    workspace
        .set_folder_description("Access", Some("Access layer"))
        .unwrap();
    assert_eq!(
        read(&root, "Access/_folder.yaml"),
        "description: Access layer\n"
    );

    // Clearing the only setting removes the file.
    workspace.set_folder_description("Core", None).unwrap();
    assert!(!root.join("Core/_folder.yaml").exists());
    assert_eq!(workspace.folder("Core").unwrap().description, None);

    let error = workspace
        .set_folder_description("Nope", Some("x"))
        .unwrap_err();
    assert!(matches!(error, EditError::NotFound(_)), "{error}");
}
