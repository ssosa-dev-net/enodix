//! The resolved inventory export (what the headless binary prints): every
//! host of the `export` fixture with the settings that apply to it, limited
//! by folder, saved filter or host ids.

mod support;

use enodix_core::inventory::{export, ExportError, ExportedHost, InventoryExport, Limit};
use enodix_core::workspace::{Environment, HostId, Protocol, Workspace};
use support::workspaces::fixture;

fn workspace() -> Workspace {
    Workspace::open(fixture("export")).unwrap()
}

fn all() -> InventoryExport {
    export(&workspace(), &Limit::All).unwrap()
}

fn ids(export: &InventoryExport) -> Vec<&str> {
    export.hosts.iter().map(|host| host.id.as_str()).collect()
}

fn host<'a>(export: &'a InventoryExport, id: &str) -> &'a ExportedHost {
    export
        .hosts
        .iter()
        .find(|host| host.id.as_str() == id)
        .unwrap_or_else(|| panic!("no host {id} in {:?}", ids(export)))
}

#[test]
fn every_host_is_exported_by_id_with_the_workspace_it_comes_from() {
    let export = all();
    assert_eq!(
        ids(&export),
        [
            "Madrid/Lab/lab-sw1.yaml",
            "Madrid/core-r1.yaml",
            "Madrid/old-ts1.yaml",
            "bastion.yaml",
            "old-sw1.yaml",
        ]
    );
    assert_eq!(export.format, enodix_core::inventory::FORMAT);
    assert_eq!(export.workspace.id.as_deref(), Some("export-fixture"));
    assert_eq!(export.workspace.name, "export");
}

#[test]
fn a_host_carries_its_resolved_settings_folders_tags_and_custom_fields() {
    let export = all();
    let core = host(&export, "Madrid/core-r1.yaml");
    assert_eq!(core.name, "CORE-R1");
    assert_eq!(core.address, "10.1.0.1");
    assert_eq!(core.vendor.as_deref(), Some("arista"));
    assert_eq!(core.model.as_deref(), Some("7280R"));
    assert_eq!(core.role.as_deref(), Some("router"));
    assert_eq!(core.folder, "Madrid");
    assert_eq!(core.folders, ["Madrid"]);
    // Inherited tags first (root, then Madrid), then the host's own.
    assert_eq!(core.tags, ["acme", "madrid", "core", "edge"]);
    let custom: Vec<(&str, &str)> = core
        .custom
        .iter()
        .map(|field| (field.key.as_str(), field.value.as_str()))
        .collect();
    assert_eq!(custom, [("owner", "noc"), ("circuit", "CID-1")]);
    assert_eq!(core.withheld, ["custom.snmp_community"]);

    let settings = &core.settings;
    assert_eq!(settings.protocol, Protocol::Ssh);
    assert_eq!(settings.port, Some(2222), "from the Madrid folder");
    assert_eq!(settings.platform.as_deref(), Some("eos"));
    assert_eq!(settings.credential_profile.as_deref(), Some("madrid-ops"));
    assert_eq!(settings.environment, Some(Environment::Production));
    assert_eq!(settings.jump_host.as_deref(), Some("bastion.yaml"));
    assert!(settings.logging);
}

#[test]
fn a_port_nothing_sets_is_the_protocols_default() {
    let export = all();
    assert_eq!(host(&export, "bastion.yaml").settings.port, Some(22));
    let telnet = host(&export, "old-sw1.yaml");
    assert_eq!(telnet.settings.protocol, Protocol::Telnet);
    assert_eq!(telnet.settings.port, Some(23));
    assert_eq!(
        telnet.settings.credential_profile.as_deref(),
        Some("corp-admin"),
        "from the root folder"
    );
    assert!(
        telnet.folders.is_empty(),
        "a host at the root is in no folder"
    );
    assert_eq!(telnet.folder, "");
}

#[test]
fn the_jump_chain_is_resolved_in_connect_order() {
    let export = all();
    let core = host(&export, "Madrid/core-r1.yaml");
    assert_eq!(core.jump_chain_error, None);
    assert_eq!(core.jump_hosts.len(), 1);
    let jump = &core.jump_hosts[0];
    assert_eq!(jump.id, HostId::new("bastion.yaml"));
    assert_eq!(jump.address, "192.0.2.10");
    assert_eq!(jump.port, 22);
    assert_eq!(jump.credential_profile.as_deref(), Some("corp-admin"));

    let bastion = host(&export, "bastion.yaml");
    assert!(bastion.jump_hosts.is_empty());
    assert_eq!(bastion.jump_chain_error, None);
}

#[test]
fn a_host_not_reached_over_ssh_has_no_jump_hosts() {
    let export = all();
    let telnet = host(&export, "Madrid/old-ts1.yaml");
    assert_eq!(telnet.settings.protocol, Protocol::Telnet);
    assert_eq!(telnet.settings.jump_host.as_deref(), Some("bastion.yaml"));
    assert!(telnet.jump_hosts.is_empty());
    assert_eq!(telnet.jump_chain_error, None);
}

#[test]
fn a_broken_jump_chain_is_reported_on_the_host_without_stopping_the_export() {
    let export = all();
    let lab = host(&export, "Madrid/Lab/lab-sw1.yaml");
    assert!(lab.jump_hosts.is_empty());
    let error = lab.jump_chain_error.as_deref().unwrap();
    assert!(error.contains("gone.yaml"), "{error}");
    assert_eq!(lab.folders, ["Madrid", "Madrid/Lab"]);
    assert_eq!(lab.settings.environment, Some(Environment::Lab));
    assert_eq!(lab.tags, ["acme", "madrid", "lab"]);
}

#[test]
fn workspace_problems_are_part_of_the_export() {
    let export = all();
    let files: Vec<&str> = export.problems.iter().map(|p| p.file.as_str()).collect();
    assert_eq!(files, ["broken.yaml"]);
}

#[test]
fn the_folders_holding_the_exported_hosts_are_listed_without_the_root() {
    let export = all();
    let folders: Vec<(&str, &str, Option<&str>, &[String])> = export
        .folders
        .iter()
        .map(|folder| {
            (
                folder.path.as_str(),
                folder.name.as_str(),
                folder.description.as_deref(),
                folder.tags.as_slice(),
            )
        })
        .collect();
    assert_eq!(
        folders,
        [
            (
                "Madrid",
                "Madrid",
                Some("Madrid DC"),
                &["madrid".to_string()][..]
            ),
            ("Madrid/Lab", "Lab", None, &["lab".to_string()][..]),
        ]
    );
}

#[test]
fn limiting_to_a_folder_exports_the_hosts_in_it_and_below() {
    let workspace = workspace();
    let export = export(&workspace, &Limit::Folder("Madrid".into())).unwrap();
    assert_eq!(
        ids(&export),
        [
            "Madrid/Lab/lab-sw1.yaml",
            "Madrid/core-r1.yaml",
            "Madrid/old-ts1.yaml"
        ]
    );

    // Matched ignoring case, with either separator.
    let lab = export_of(&workspace, Limit::Folder(" madrid \\ lab ".into()));
    assert_eq!(ids(&lab), ["Madrid/Lab/lab-sw1.yaml"]);
    let paths: Vec<&str> = lab.folders.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["Madrid", "Madrid/Lab"]);

    // The root is the whole workspace.
    assert_eq!(
        ids(&export_of(&workspace, Limit::Folder("".into()))).len(),
        5
    );
}

fn export_of(workspace: &Workspace, limit: Limit) -> InventoryExport {
    export(workspace, &limit).unwrap()
}

#[test]
fn limiting_to_a_saved_filter_exports_the_hosts_it_matches() {
    let workspace = workspace();
    let edge = export_of(&workspace, Limit::Filter("edge".into()));
    assert_eq!(ids(&edge), ["Madrid/core-r1.yaml"]);
    let arista = export_of(&workspace, Limit::Filter("Arista".into()));
    assert_eq!(ids(&arista), ["Madrid/core-r1.yaml"]);
}

#[test]
fn limiting_to_host_ids_exports_those_hosts_in_the_order_given_once_each() {
    let workspace = workspace();
    let export = export_of(
        &workspace,
        Limit::Hosts(vec![
            "old-sw1.yaml".into(),
            "MADRID/CORE-R1.YAML".into(),
            "old-sw1.yaml".into(),
        ]),
    );
    assert_eq!(ids(&export), ["old-sw1.yaml", "Madrid/core-r1.yaml"]);
    let paths: Vec<&str> = export.folders.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["Madrid"]);
}

#[test]
fn an_unknown_folder_filter_or_host_is_an_error() {
    let workspace = workspace();
    match export(&workspace, &Limit::Folder("Lisbon".into())) {
        Err(ExportError::UnknownFolder(path)) => assert_eq!(path, "Lisbon"),
        other => panic!("{other:?}"),
    }
    match export(&workspace, &Limit::Filter("Firewalls".into())) {
        Err(error @ ExportError::UnknownFilter { .. }) => {
            let message = error.to_string();
            assert!(message.contains("Firewalls"), "{message}");
            assert!(message.contains("Edge, Arista"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    match export(
        &workspace,
        &Limit::Hosts(vec![
            "bastion.yaml".into(),
            "nope.yaml".into(),
            "x".into(),
            "nope.yaml".into(),
        ]),
    ) {
        Err(ExportError::UnknownHosts(unknown)) => assert_eq!(unknown, ["nope.yaml", "x"]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_json_has_plain_values_and_no_secrets() {
    let json = serde_json::to_value(all()).unwrap();
    let core = json["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|host| host["id"] == "Madrid/core-r1.yaml")
        .unwrap();
    assert_eq!(core["address"], "10.1.0.1");
    assert_eq!(core["settings"]["protocol"], "ssh");
    assert_eq!(core["settings"]["port"], 2222);
    assert_eq!(core["settings"]["environment"], "production");
    assert_eq!(core["settings"]["serial"]["baud_rate"], 9600);
    assert_eq!(core["settings"]["serial"]["port"], serde_json::Value::Null);
    assert!(core["settings"].get("web_links").is_none());
    assert_eq!(
        core["custom"],
        serde_json::json!({"owner": "noc", "circuit": "CID-1"})
    );
    // Custom fields keep their file order in the text too.
    let text = serde_json::to_string(&all()).unwrap();
    assert!(text.find("\"owner\"").unwrap() < text.find("\"circuit\"").unwrap());
    assert_eq!(core["jump_hosts"][0]["id"], "bastion.yaml");
    assert_eq!(json["problems"][0]["file"], "broken.yaml");
    let all_text = json.to_string().to_lowercase();
    for word in ["password", "secret", "passphrase", "hunter2", "s3cr3t"] {
        assert!(!all_text.contains(word), "{word} in {all_text}");
    }
}
