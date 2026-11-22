//! Device roles: the built-in list, the roles a workspace adds in its roles
//! file, and the role each host shows (a neutral one when the workspace
//! does not define it).

mod support;

use std::fs;

use enodix_core::workspace::{HostId, Role, RoleIcon, RoleSource, Workspace, NEUTRAL_ROLE_COLOR};
use support::workspaces::{fixture, fixture_copy, read};
use tempfile::TempDir;

fn ids(roles: &[Role]) -> Vec<&str> {
    roles.iter().map(|role| role.id.as_str()).collect()
}

#[test]
fn every_workspace_starts_with_the_built_in_roles() {
    let temp = TempDir::new().unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    let roles = workspace.roles();
    assert_eq!(
        ids(roles),
        [
            "core",
            "distribution",
            "access",
            "edge_router",
            "firewall",
            "load_balancer",
            "wireless_controller",
            "access_point",
            "console_server",
            "server",
        ]
    );
    let names: Vec<&str> = roles.iter().map(|role| role.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Core",
            "Distribution",
            "Access",
            "Edge/WAN router",
            "Firewall",
            "Load balancer",
            "Wireless controller",
            "Access point",
            "Out-of-band/console server",
            "Server",
        ]
    );
    let firewall = workspace.role("firewall").unwrap();
    assert_eq!(firewall.icon, RoleIcon::Firewall);
    assert_eq!(firewall.source, RoleSource::BuiltIn);
    for role in roles {
        assert!(is_colour(&role.color), "{}: {}", role.id, role.color);
        assert_ne!(role.icon, RoleIcon::Unknown, "{}", role.id);
    }
}

#[test]
fn a_workspace_adds_its_own_roles_from_its_roles_file() {
    let workspace = Workspace::open(fixture("roles")).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );

    // Its own roles come after the built-in ones; one with a built-in id
    // replaces that one in place. Only the root's roles file counts.
    let roles = workspace.roles();
    assert_eq!(
        &ids(roles)[..5],
        ["core", "distribution", "access", "edge_router", "firewall"]
    );
    assert_eq!(&ids(roles)[10..], ["spine", "leaf"]);
    assert!(workspace.role("ignored").is_none());

    assert_eq!(
        workspace.role("spine"),
        Some(&Role {
            id: "spine".into(),
            name: "Spine".into(),
            icon: RoleIcon::Switch,
            color: "#2563eb".into(),
            source: RoleSource::Workspace,
        })
    );
    let firewall = workspace.role("firewall").unwrap();
    assert_eq!(firewall.name, "Perimeter firewall");
    assert_eq!(firewall.color, "#b91c1c");
    assert_eq!(firewall.source, RoleSource::Workspace);

    // Without a name, the id is the name; `#rgb` is spelled out.
    let leaf = workspace.role("LEAF").unwrap();
    assert_eq!(leaf.name, "leaf");
    assert_eq!(leaf.icon, RoleIcon::Access);
    assert_eq!(leaf.color, "#00aa00");
}

#[test]
fn a_host_shows_the_role_its_file_names() {
    let workspace = Workspace::open(fixture("roles")).unwrap();
    let role_of = |id: &str| workspace.host_role(&HostId::new(id));

    // The workspace's own firewall role replaced the built-in one.
    let firewall = role_of("edge-fw.yaml").unwrap();
    assert_eq!(firewall.name, "Perimeter firewall");
    assert_eq!(firewall.color, "#b91c1c");
    // Named ignoring case.
    assert_eq!(
        workspace
            .host(&HostId::new("spine1.yaml"))
            .unwrap()
            .role
            .as_deref(),
        Some("Spine")
    );
    assert_eq!(role_of("spine1.yaml").unwrap().id, "spine");
    // No role.
    assert_eq!(
        workspace.host(&HostId::new("Site/sw1.yaml")).unwrap().role,
        None
    );
    assert_eq!(role_of("Site/sw1.yaml"), None);
    assert_eq!(role_of("missing.yaml"), None);
}

#[test]
fn a_host_whose_role_is_not_defined_loads_with_a_neutral_role() {
    let workspace = Workspace::open(fixture("roles")).unwrap();

    let legacy = workspace.host(&HostId::new("legacy.yaml")).unwrap();
    assert_eq!(legacy.role.as_deref(), Some("mainframe"));
    assert_eq!(
        workspace.host_role(&legacy.id),
        Some(Role {
            id: "mainframe".into(),
            name: "mainframe".into(),
            icon: RoleIcon::Unknown,
            color: NEUTRAL_ROLE_COLOR.into(),
            source: RoleSource::Undefined,
        })
    );
    assert!(workspace.problems().is_empty());
}

#[test]
fn every_host_with_a_role_is_listed_with_the_role_it_shows() {
    let workspace = Workspace::open(fixture("roles")).unwrap();

    let host_roles = workspace.host_roles();
    let shown: Vec<(&str, &str, RoleIcon)> = host_roles
        .iter()
        .map(|(id, role)| (id.as_str(), role.id.as_str(), role.icon))
        .collect();
    assert_eq!(
        shown,
        [
            ("edge-fw.yaml", "firewall", RoleIcon::Firewall),
            ("legacy.yaml", "mainframe", RoleIcon::Unknown),
            ("spine1.yaml", "spine", RoleIcon::Switch),
        ]
    );
}

#[test]
fn a_chosen_role_is_saved_after_the_rack_fields_and_can_be_cleared() {
    let (_temp, root) = fixture_copy("roles");
    let mut workspace = Workspace::open(&root).unwrap();
    let id = HostId::new("Site/sw1.yaml");

    let mut sw = workspace.host(&id).unwrap().clone();
    sw.role = Some(" access_point ".into());
    workspace.write_host(&sw).unwrap();
    assert_eq!(
        read(&root, "Site/sw1.yaml"),
        "\
address: 192.0.2.21
vendor: Cisco
rack: R3
rack_unit: 40
role: access_point
description: Access, floor 2
"
    );
    assert_eq!(workspace.host_role(&id).unwrap().name, "Access point");

    let mut sw = workspace.host(&id).unwrap().clone();
    sw.role = None;
    workspace.write_host(&sw).unwrap();
    assert!(!read(&root, "Site/sw1.yaml").contains("role"));
    assert_eq!(workspace.host_role(&id), None);
}

#[test]
fn badly_defined_roles_are_reported_and_the_others_load() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("_roles.yaml"),
        "\
spine:
  icon: switch
  color: '#2563eb'
blinky:
  icon: lava_lamp
  color: '#ff0000'
pale:
  icon: server
  color: blue
nothing: here
",
    )
    .unwrap();
    fs::write(
        temp.path().join("r1.yaml"),
        "address: 192.0.2.1\nrole: blinky\n",
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    let problems: Vec<String> = workspace
        .problems()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems[0].starts_with("_roles.yaml: role `blinky`: `icon` must be one of core,"));
    assert_eq!(
        problems[1],
        "_roles.yaml: role `pale`: `color` must be a colour like \"#2563eb\""
    );
    assert_eq!(
        problems[2],
        "_roles.yaml: role `nothing`: expected `name`, `icon` and `color`"
    );
    assert_eq!(&ids(workspace.roles())[10..], ["spine"]);
    // The host naming a badly defined role still loads.
    assert!(workspace.host(&HostId::new("r1.yaml")).is_some());
}

#[test]
fn an_unreadable_roles_file_leaves_the_built_in_roles() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("_roles.yaml"), "- not\n- a mapping\n").unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    assert_eq!(workspace.problems().len(), 1);
    assert_eq!(workspace.problems()[0].file, "_roles.yaml");
    assert_eq!(workspace.roles().len(), 10);
}

/// `#rrggbb` in lower case.
fn is_colour(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text[1..]
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}
