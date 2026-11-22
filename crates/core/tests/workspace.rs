//! Workspace tests: fixture workspace folders loaded from disk, asserting on
//! the tree a user would see and on the files the app writes back.

mod support;

use std::fmt::Write as _;
use std::fs;

use enodix_core::session::{SessionState, SessionTarget, TerminalSize};
use enodix_core::workspace::{Folder, HostId, Workspace};
use support::workspaces::{fixture, fixture_copy};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

/// The tree as an indented outline: folders end in `/`, hosts show their
/// management address and port (`-` when the file sets none).
fn outline(folder: &Folder) -> String {
    fn walk(folder: &Folder, depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth);
        for child in &folder.folders {
            writeln!(out, "{indent}{}/", child.name).unwrap();
            walk(child, depth + 1, out);
        }
        for host in &folder.hosts {
            let port = host
                .settings
                .port
                .map_or("-".to_string(), |port| port.to_string());
            writeln!(out, "{indent}{} {}:{port}", host.name, host.address).unwrap();
        }
    }
    let mut out = String::new();
    walk(folder, 0, &mut out);
    out
}

#[test]
fn opening_a_folder_shows_its_directories_and_hosts_as_a_tree() {
    let workspace = Workspace::open(fixture("nested")).unwrap();

    assert_eq!(workspace.tree().name, "nested");
    assert_eq!(
        outline(workspace.tree()),
        "\
Empty/
Europe/
  Lisbon/
    border-r1 10.2.0.1:-
  Madrid/
    DC1/
      Rack-A1/
        core-sw1 10.1.0.1:22
        fw1 10.1.0.254:2222
lab-r1 192.0.2.1:22
"
    );
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    assert_eq!(
        workspace.tree().description.as_deref(),
        Some("Acme Corp network")
    );
    let europe = &workspace.tree().folders[1];
    assert_eq!(europe.path, "Europe");
    assert_eq!(europe.description.as_deref(), Some("European sites"));
}

#[test]
fn a_malformed_file_is_reported_by_name_and_the_rest_still_loads() {
    let workspace = Workspace::open(fixture("broken")).unwrap();

    assert_eq!(
        outline(workspace.tree()),
        "\
Site/
  site-r1 10.0.1.1:-
good-r1 10.0.0.1:-
"
    );
    let problems: Vec<String> = workspace.problems().iter().map(|p| p.to_string()).collect();
    let files: Vec<&str> = workspace
        .problems()
        .iter()
        .map(|p| p.file.as_str())
        .collect();
    assert_eq!(
        files,
        [
            "Site/_folder.yaml",
            "a-list.yml",
            "bad-port.yaml",
            "bad-yaml.yaml",
            "no-address.yaml"
        ],
        "{problems:#?}"
    );
    let message = |file: &str| problems.iter().find(|p| p.starts_with(file)).unwrap();
    assert!(message("bad-yaml.yaml: invalid YAML").contains("line 2"));
    assert_eq!(
        message("no-address.yaml"),
        "no-address.yaml: missing `address`"
    );
    assert_eq!(
        message("bad-port.yaml"),
        "bad-port.yaml: `port` must be a number from 1 to 65535"
    );
    assert!(message("a-list.yml").contains("expected a mapping"));
    assert!(message("Site/_folder.yaml").contains("invalid YAML"));
}

#[test]
fn a_missing_workspace_folder_cannot_be_opened() {
    let error = Workspace::open(fixture("does-not-exist")).unwrap_err();

    assert!(error.to_string().contains("does-not-exist"), "{error}");
}

#[test]
fn rewriting_a_host_file_keeps_the_keys_it_does_not_know() {
    let (_temp, root) = fixture_copy("rewrite");
    let mut workspace = Workspace::open(&root).unwrap();
    let id = HostId::new("Edge/edge-r1.yaml");
    let mut host = workspace.host(&id).unwrap().clone();

    host.address = "10.9.0.2".into();
    host.settings.port = None;
    workspace.write_host(&host).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("Edge/edge-r1.yaml")).unwrap(),
        "\
name: edge-r1
vendor: Juniper
address: 10.9.0.2
rack_unit: 12
custom:
  contract: ACME-1234
  owner: noc
"
    );
    let reloaded = Workspace::open(&root).unwrap();
    assert_eq!(
        reloaded.host(&id).unwrap().address,
        "10.9.0.2",
        "the change is seen on reload"
    );
    // No temporary file is left behind.
    assert_eq!(fs::read_dir(root.join("Edge")).unwrap().count(), 2);
}

#[test]
fn a_host_is_found_by_its_id_and_gives_its_ssh_target() {
    let workspace = Workspace::open(fixture("nested")).unwrap();

    let fw = workspace
        .resolve_host(&HostId::new("Europe/Madrid/DC1/Rack-A1/fw1.yml"))
        .unwrap();
    let target = fw.ssh_target("admin".into(), "secret".into());
    assert_eq!((target.host.as_str(), target.port), ("10.1.0.254", 2222));
    assert_eq!(target.username, "admin");

    // Without a port anywhere, SSH's default applies.
    let border = workspace
        .resolve_host(&HostId::new("Europe/Lisbon/border-r1.yaml"))
        .unwrap();
    assert_eq!(border.ssh_target("a".into(), "b".into()).port, 22);

    assert!(workspace.host(&HostId::new("nope.yaml")).is_none());
}

#[tokio::test]
async fn connecting_to_a_host_from_the_tree_opens_an_ssh_session() {
    let server = TestSshServer::start().await;
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join("Lab")).unwrap();
    fs::write(
        temp.path().join("Lab/test-r1.yaml"),
        format!(
            "name: test-r1\naddress: 127.0.0.1\nport: {}\n",
            server.port()
        ),
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    let host = workspace
        .resolve_host(&HostId::new("Lab/test-r1.yaml"))
        .unwrap();

    let manager = support::manager();
    let target = SessionTarget::Ssh(host.ssh_target(USERNAME.into(), PASSWORD.into()));
    let (_id, mut events) = manager.open(target, TerminalSize { cols: 80, rows: 24 });

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[test]
fn a_host_named_by_its_file_stays_that_way_when_rewritten() {
    let (_temp, root) = fixture_copy("rewrite");
    let mut workspace = Workspace::open(&root).unwrap();
    let mut host = workspace
        .host(&HostId::new("Edge/edge-r2.yaml"))
        .unwrap()
        .clone();
    assert_eq!(host.name, "edge-r2");

    host.settings.port = Some(830);
    workspace.write_host(&host).unwrap();

    assert_eq!(
        fs::read_to_string(root.join("Edge/edge-r2.yaml")).unwrap(),
        "address: 10.9.0.3\nport: 830\n"
    );
}

#[test]
fn only_hosts_of_the_workspace_can_be_written() {
    let (_temp, root) = fixture_copy("rewrite");
    let mut workspace = Workspace::open(&root).unwrap();
    let mut host = workspace
        .host(&HostId::new("Edge/edge-r1.yaml"))
        .unwrap()
        .clone();

    host.id = HostId::new("../outside.yaml");
    let error = workspace.write_host(&host).unwrap_err();

    assert!(error.to_string().contains("../outside.yaml"), "{error}");
    assert!(!root.parent().unwrap().join("outside.yaml").exists());
}
