//! Nornir import: reading a simple inventory (hosts, groups and defaults
//! files), group and defaults inheritance, data as custom fields, passwords
//! never written, the no-overwrite rule and the report. Each import is
//! checked by reading the workspace files on disk afterwards.

mod support;

use std::fs;

use netterm_core::import::nornir::{
    NornirError, NornirFile, NornirImport, NornirImportError, NornirInventory,
};
use netterm_core::import::{ImportReport, PreviewOutcome, ReportEntry};
use netterm_core::workspace::{CustomField, HostId, Workspace};
use support::workspaces::{files, fixture_copy, read};
use tempfile::TempDir;

/// The `edit` fixture (Access/sw1, Core/core-r1, Core/core-r2.yml, Empty) as
/// a writable copy, opened.
fn workspace() -> (TempDir, std::path::PathBuf, Workspace) {
    let (temp, root) = fixture_copy("edit");
    let workspace = Workspace::open(&root).unwrap();
    (temp, root, workspace)
}

fn import(hosts: &str, groups: Option<&str>, defaults: Option<&str>, target: &str) -> NornirImport {
    NornirImport {
        inventory: NornirInventory::parse(hosts, groups, defaults).unwrap(),
        target: target.into(),
    }
}

/// `(line, name, reason)` of each entry.
fn entries(entries: &[ReportEntry]) -> Vec<(Option<usize>, String, Option<String>)> {
    entries
        .iter()
        .map(|e| (e.line, e.name.clone(), e.reason.clone()))
        .collect()
}

fn names(entries: &[ReportEntry]) -> Vec<String> {
    entries.iter().map(|e| e.name.clone()).collect()
}

fn created(report: &ImportReport) -> Vec<String> {
    report
        .created
        .iter()
        .map(|e| e.host.as_ref().unwrap().to_string())
        .collect()
}

fn id(path: &str) -> HostId {
    HostId::new(path)
}

/// The host's own tags, in order.
fn tags(workspace: &Workspace, host: &str) -> Vec<String> {
    workspace.host(&id(host)).unwrap().tags.clone()
}

fn custom(workspace: &Workspace, host: &str) -> Vec<(String, String)> {
    workspace
        .host(&id(host))
        .unwrap()
        .custom
        .iter()
        .map(|f| (f.key.clone(), f.value.clone()))
        .collect()
}

/// Every file of the workspace, concatenated: for checking that a value
/// was written nowhere.
fn all_text(root: &std::path::Path) -> String {
    files(root)
        .iter()
        .filter(|path| !path.ends_with('/'))
        .map(|path| read(root, path))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- Hosts files alone -----------------------------------------------------

#[test]
fn a_hosts_file_alone_imports_its_hosts_into_the_target_folder() {
    let (_temp, root, mut ws) = workspace();
    let hosts = "\
---
edge-r1:
  hostname: 10.1.0.1
  port: 2222
  platform: ios
edge-r2:
  hostname: edge-r2.example.net
";
    let report = import(hosts, None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();

    assert_eq!(
        created(&report),
        ["Empty/edge-r1.yaml", "Empty/edge-r2.yaml"]
    );
    assert_eq!(entries(&report.created)[0].0, Some(2));
    assert_eq!(entries(&report.created)[1].0, Some(6));
    assert!(report.skipped.is_empty() && report.failed.is_empty());

    let r1 = ws.resolve_host(&id("Empty/edge-r1.yaml")).unwrap();
    assert_eq!(r1.host.name, "edge-r1");
    assert_eq!(r1.host.address, "10.1.0.1");
    assert_eq!(r1.host.platform.as_deref(), Some("ios"));
    assert_eq!(r1.settings.port.map(|port| port.value), Some(2222));
    let r2 = ws.host(&id("Empty/edge-r2.yaml")).unwrap();
    assert_eq!(r2.address, "edge-r2.example.net");
    assert_eq!(r2.settings.port, None);
    assert_eq!(r2.platform, None);
    assert_eq!(
        read(&root, "Empty/edge-r1.yaml"),
        "address: 10.1.0.1\nport: 2222\nplatform: ios\n"
    );
}

#[test]
fn without_a_groups_file_group_membership_still_becomes_tags() {
    let (_temp, _root, mut ws) = workspace();
    let hosts = "\
r1:
  hostname: 10.1.0.1
  groups: [madrid, core]
";
    let report = import(hosts, None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml"]);
    assert_eq!(tags(&ws, "Empty/r1.yaml"), ["madrid", "core"]);
}

#[test]
fn an_empty_hosts_file_imports_nothing() {
    let (_temp, root, mut ws) = workspace();
    let before = files(&root);
    let report = import("# nothing yet\n", None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(report, ImportReport::default());
    assert_eq!(files(&root), before);
}

// --- Groups and defaults ---------------------------------------------------

const GROUPS: &str = "\
---
core:
  port: 830
  platform: junos
  groups:
    - dc1
  data:
    layer: core
    owner: core-team
madrid:
  hostname: madrid-default.example.net
  platform: eos
  data:
    site_code: MAD
    owner: madrid-noc
dc1:
  port: 2200
  data:
    datacenter: DC1
    layer: dc
";

const DEFAULTS: &str = "\
---
port: 22
platform: ios
data:
  owner: noc
  domain: example.net
";

#[test]
fn values_come_from_the_host_then_its_groups_in_order_then_defaults() {
    let (_temp, _root, mut ws) = workspace();
    let hosts = "\
r1:
  hostname: 10.1.0.1
  groups: [core, madrid]
r2:
  groups: [madrid, core]
  data:
    owner: r2-owner
r3:
  hostname: 10.1.0.3
  port: 23
r4:
  hostname: 10.1.0.4
  groups: [dc1]
";
    let report = import(hosts, Some(GROUPS), Some(DEFAULTS), "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(
        created(&report),
        [
            "Empty/r1.yaml",
            "Empty/r2.yaml",
            "Empty/r3.yaml",
            "Empty/r4.yaml"
        ]
    );

    // r1: its own hostname; core's port and platform (before madrid's).
    let r1 = ws.host(&id("Empty/r1.yaml")).unwrap();
    assert_eq!(r1.address, "10.1.0.1");
    assert_eq!(r1.settings.port, Some(830));
    assert_eq!(r1.platform.as_deref(), Some("junos"));
    // A parent group (core → dc1) counts as membership too.
    assert_eq!(tags(&ws, "Empty/r1.yaml"), ["core", "dc1", "madrid"]);

    // r2: madrid first, so its hostname and platform; core's port.
    let r2 = ws.host(&id("Empty/r2.yaml")).unwrap();
    assert_eq!(r2.address, "madrid-default.example.net");
    assert_eq!(r2.platform.as_deref(), Some("eos"));
    assert_eq!(r2.settings.port, Some(830));
    assert_eq!(tags(&ws, "Empty/r2.yaml"), ["madrid", "core", "dc1"]);

    // r3: no groups; its own port, the defaults' platform.
    let r3 = ws.host(&id("Empty/r3.yaml")).unwrap();
    assert_eq!(r3.settings.port, Some(23));
    assert_eq!(r3.platform.as_deref(), Some("ios"));
    assert!(r3.tags.is_empty());

    // r4: the group's port over the defaults'.
    let r4 = ws.host(&id("Empty/r4.yaml")).unwrap();
    assert_eq!(r4.settings.port, Some(2200));
    assert_eq!(r4.platform.as_deref(), Some("ios"));
}

#[test]
fn data_is_inherited_key_by_key_nearest_first() {
    let (_temp, _root, mut ws) = workspace();
    let hosts = "\
r1:
  hostname: 10.1.0.1
  groups: [core, madrid]
r2:
  hostname: 10.1.0.2
  groups: [madrid, core]
  data:
    owner: r2-owner
r3:
  hostname: 10.1.0.3
";
    import(hosts, Some(GROUPS), Some(DEFAULTS), "Empty")
        .run(&mut ws, &[])
        .unwrap();

    let field = |k: &str, v: &str| (k.to_string(), v.to_string());
    assert_eq!(
        custom(&ws, "Empty/r1.yaml"),
        [
            field("layer", "core"),
            field("owner", "core-team"),
            field("datacenter", "DC1"),
            field("site_code", "MAD"),
            field("domain", "example.net"),
        ]
    );
    assert_eq!(
        custom(&ws, "Empty/r2.yaml"),
        [
            field("owner", "r2-owner"),
            field("site_code", "MAD"),
            field("layer", "core"),
            field("datacenter", "DC1"),
            field("domain", "example.net"),
        ]
    );
    assert_eq!(
        custom(&ws, "Empty/r3.yaml"),
        [field("owner", "noc"), field("domain", "example.net")]
    );
}

#[test]
fn a_group_missing_from_the_groups_file_is_a_tag_and_a_report_note() {
    let (_temp, _root, mut ws) = workspace();
    let hosts = "\
r1:
  hostname: 10.1.0.1
  groups: [core, lab]
";
    let report = import(hosts, Some(GROUPS), None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml"]);
    assert_eq!(tags(&ws, "Empty/r1.yaml"), ["core", "dc1", "lab"]);
    assert_eq!(
        entries(&report.skipped),
        [(
            None,
            "group lab".to_string(),
            Some("not defined in the groups file: its hosts get the tag only".to_string())
        )]
    );
}

#[test]
fn groups_naming_each_other_in_a_loop_do_not_hang() {
    let (_temp, _root, mut ws) = workspace();
    let groups = "\
a:
  groups: [b]
  data: { from_a: 1 }
b:
  groups: [a]
  data: { from_b: 2 }
";
    let hosts = "r1:\n  hostname: 10.1.0.1\n  groups: [a]\n";
    import(hosts, Some(groups), None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(tags(&ws, "Empty/r1.yaml"), ["a", "b"]);
    assert_eq!(custom(&ws, "Empty/r1.yaml").len(), 2);
}

#[test]
fn yaml_anchors_and_merge_keys_are_followed() {
    let (_temp, _root, mut ws) = workspace();
    let hosts = "\
base: &base
  hostname: 10.9.9.9
  platform: nxos
r1:
  <<: *base
  hostname: 10.1.0.1
";
    import(hosts, None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    let r1 = ws.host(&id("Empty/r1.yaml")).unwrap();
    assert_eq!(r1.address, "10.1.0.1");
    assert_eq!(r1.platform.as_deref(), Some("nxos"));
}

// --- Data ------------------------------------------------------------------

#[test]
fn entries_under_data_become_custom_fields() {
    let (_temp, root, mut ws) = workspace();
    let hosts = "\
r1:
  hostname: 10.1.0.1
  data:
    site: Madrid
    asn: 65001
    managed: true
    vlans: [10, 20, 30]
    snmp:
      location: Rack 4
      contact: noc@example.net
    empty:
    routes:
      - prefix: 10.0.0.0/8
";
    let report = import(hosts, None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml"]);
    let host = ws.host(&id("Empty/r1.yaml")).unwrap();
    // Data never fills a built-in field: `site` stays a custom field.
    assert_eq!(host.site, None);
    assert_eq!(
        host.custom,
        [
            CustomField::new("site", "Madrid"),
            CustomField::new("asn", "65001"),
            CustomField::new("managed", "true"),
            CustomField::new("vlans", "10, 20, 30"),
            CustomField::new("snmp.location", "Rack 4"),
            CustomField::new("snmp.contact", "noc@example.net"),
            CustomField::new("routes", r#"[{"prefix":"10.0.0.0/8"}]"#),
        ]
    );
    assert!(read(&root, "Empty/r1.yaml").contains("custom:\n  site: Madrid\n"));
}

// --- Passwords -------------------------------------------------------------

#[test]
fn passwords_are_never_written_and_are_listed_as_skipped() {
    let (_temp, root, mut ws) = workspace();
    let defaults = "\
username: admin
password: DefaultPass1
";
    let groups = "\
core:
  password: GroupPass2
  connection_options:
    netmiko:
      extras:
        secret: EnableSecret3
";
    let hosts = "\
r1:
  hostname: 10.1.0.1
  password: HostPass4
  groups: [core]
  data:
    snmp_community: Community7
    snmp:
      auth_password: SnmpPass5
    enable_secret: EnableSecret6
    api_token: Token8
    tacacs_key: Key9
    users:
      - name: ops
        password: ListPass10
    site: Madrid
";
    let preview = import(hosts, Some(groups), Some(defaults), "Empty")
        .preview(&ws)
        .unwrap();
    let skipped = [
        "defaults: password",
        "group core: password",
        "group core: connection_options.netmiko.extras.secret",
        "host r1: password",
        "host r1: data.snmp_community",
        "host r1: data.snmp.auth_password",
        "host r1: data.enable_secret",
        "host r1: data.api_token",
        "host r1: data.tacacs_key",
        "host r1: data.users.0.password",
    ];
    let secrets: Vec<String> = preview
        .skipped
        .iter()
        .filter(|e| e.reason.as_deref() == Some("passwords are never written to the workspace"))
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(secrets, skipped);

    let report = import(hosts, Some(groups), Some(defaults), "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml"]);
    for name in skipped {
        assert!(names(&report.skipped).contains(&name.to_string()), "{name}");
    }
    let text = all_text(&root);
    for secret in [
        "DefaultPass1",
        "GroupPass2",
        "EnableSecret3",
        "HostPass4",
        "SnmpPass5",
        "EnableSecret6",
        "Community7",
        "Token8",
        "Key9",
        "ListPass10",
    ] {
        assert!(!text.contains(secret), "{secret} was written");
    }
    // Other data is imported; a list keeps its entries' other values.
    assert_eq!(
        custom(&ws, "Empty/r1.yaml"),
        [
            ("users".to_string(), r#"[{"name":"ops"}]"#.to_string()),
            ("site".to_string(), "Madrid".to_string()),
        ]
    );
}

#[test]
fn usernames_and_connection_options_are_not_imported_and_say_so() {
    let (_temp, root, mut ws) = workspace();
    let hosts = "\
r1:
  hostname: 10.1.0.1
  username: admin
  connection_options:
    napalm:
      platform: ios
      extras:
        optional_args: { transport: ssh }
  unknown_field: x
";
    let report = import(hosts, None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml"]);
    assert_eq!(
        entries(&report.skipped),
        [
            (
                Some(1),
                "host r1: username".to_string(),
                Some("usernames belong in a credential profile; not imported".to_string())
            ),
            (
                Some(1),
                "host r1: connection_options".to_string(),
                Some("connection options are not imported".to_string())
            ),
            (
                Some(1),
                "host r1: unknown_field".to_string(),
                Some("not part of a Nornir simple inventory; not imported".to_string())
            ),
        ]
    );
    assert_eq!(read(&root, "Empty/r1.yaml"), "address: 10.1.0.1\n");
}

// --- Failures --------------------------------------------------------------

#[test]
fn hosts_that_cannot_be_imported_fail_with_a_reason_and_the_rest_go_in() {
    let (_temp, _root, mut ws) = workspace();
    let hosts = "\
no-address:
  platform: ios
bad-port:
  hostname: 10.1.0.2
  port: 70000
'bad:name':
  hostname: 10.1.0.3
ok:
  hostname: 10.1.0.4
OK:
  hostname: 10.1.0.5
not-a-mapping: [1, 2]
";
    let report = import(hosts, None, None, "Empty")
        .run(&mut ws, &[])
        .unwrap();
    assert_eq!(created(&report), ["Empty/ok.yaml"]);
    let failed = entries(&report.failed);
    let failed: Vec<(Option<usize>, &str)> = failed
        .iter()
        .map(|(line, name, _)| (*line, name.as_str()))
        .collect();
    assert_eq!(
        failed,
        [
            (Some(1), "no-address"),
            (Some(3), "bad-port"),
            (Some(6), "bad:name"),
            (Some(10), "OK"),
            (Some(12), "not-a-mapping"),
        ]
    );
    let reason = |i: usize| report.failed[i].reason.clone().unwrap();
    assert_eq!(
        reason(0),
        "no hostname: not set on the host, its groups or the defaults"
    );
    assert_eq!(reason(1), "port 70000 is not a port number (1 to 65535)");
    assert!(reason(2).contains("cannot contain"), "{}", reason(2));
    assert_eq!(
        reason(3),
        "ok (line 8) already imports a host with this name"
    );
    assert_eq!(reason(4), "the host's entry is not a mapping");
}

#[test]
fn files_that_are_not_an_inventory_are_refused_naming_the_file() {
    let error = NornirInventory::parse("r1: [unclosed\n", None, None).unwrap_err();
    assert!(
        matches!(
            &error,
            NornirError::Yaml {
                file: NornirFile::Hosts,
                ..
            }
        ),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .starts_with("the hosts file is not valid YAML"),
        "{error}"
    );

    let error = NornirInventory::parse("r1: {}\n", Some("- a\n- b\n"), None).unwrap_err();
    assert!(
        matches!(
            &error,
            NornirError::NotAMapping {
                file: NornirFile::Groups
            }
        ),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "the groups file is not a mapping of names to entries"
    );

    let error = NornirInventory::parse("r1: {}\n", None, Some("just text\n")).unwrap_err();
    assert!(matches!(
        &error,
        NornirError::NotAMapping {
            file: NornirFile::Defaults
        }
    ));
}

#[test]
fn inventories_are_read_from_files() {
    let temp = TempDir::new().unwrap();
    let hosts = temp.path().join("hosts.yaml");
    let groups = temp.path().join("groups.yaml");
    fs::write(&hosts, "r1:\n  groups: [core]\n").unwrap();
    fs::write(&groups, "core:\n  hostname: 10.0.0.1\n").unwrap();
    let inventory = NornirInventory::read(&hosts, Some(&groups), None).unwrap();

    let (_temp, _root, mut ws) = workspace();
    let report = NornirImport {
        inventory,
        target: "Empty".into(),
    }
    .run(&mut ws, &[])
    .unwrap();
    assert_eq!(created(&report), ["Empty/r1.yaml"]);
    assert_eq!(ws.host(&id("Empty/r1.yaml")).unwrap().address, "10.0.0.1");

    let missing = temp.path().join("defaults.yaml");
    let error = NornirInventory::read(&hosts, None, Some(&missing)).unwrap_err();
    assert!(matches!(
        &error,
        NornirError::Io {
            file: NornirFile::Defaults,
            ..
        }
    ));
    assert!(
        error
            .to_string()
            .starts_with("cannot read the defaults file"),
        "{error}"
    );
}

#[test]
fn the_target_folder_must_exist() {
    let (_temp, _root, mut ws) = workspace();
    let error = import("r1:\n  hostname: 10.1.0.1\n", None, None, "Nowhere")
        .run(&mut ws, &[])
        .unwrap_err();
    assert!(matches!(error, NornirImportError::NoSuchFolder(ref f) if f == "Nowhere"));
}

// --- Existing hosts and the report -----------------------------------------

#[test]
fn the_preview_shows_what_each_host_would_do_and_writes_nothing() {
    let (_temp, root, ws) = workspace();
    let before = files(&root);
    let hosts = "\
core-r1:
  hostname: 10.9.0.1
CORE-R2:
  hostname: 10.9.0.2
core-r3:
  hostname: 10.9.0.3
broken: {}
";
    let preview = import(hosts, None, None, "Core").preview(&ws).unwrap();
    let outcomes: Vec<(Option<usize>, &str, &PreviewOutcome)> = preview
        .hosts
        .iter()
        .map(|h| (h.line, h.name.as_str(), &h.outcome))
        .collect();
    assert_eq!(
        outcomes[..3],
        [
            (
                Some(1),
                "core-r1",
                &PreviewOutcome::Exists {
                    id: id("Core/core-r1.yaml")
                }
            ),
            (
                Some(3),
                "CORE-R2",
                &PreviewOutcome::Exists {
                    id: id("Core/core-r2.yml")
                }
            ),
            (
                Some(5),
                "core-r3",
                &PreviewOutcome::Create {
                    id: id("Core/core-r3.yaml")
                }
            ),
        ]
    );
    assert!(matches!(outcomes[3].2, PreviewOutcome::Fail { .. }));
    let host = preview.hosts[2].host.as_ref().unwrap();
    assert_eq!(host.address, "10.9.0.3");
    assert_eq!(host.folder, "Core");
    assert_eq!(files(&root), before);
}

#[test]
fn existing_hosts_are_skipped_unless_confirmed() {
    let (_temp, root, mut ws) = workspace();
    let r1_before = read(&root, "Core/core-r1.yaml");
    let r2_before = read(&root, "Core/core-r2.yml");
    let hosts = "\
core-r1:
  hostname: 10.9.0.1
  platform: eos
  groups: [imported]
  data:
    owner: nornir
core-r2:
  hostname: 10.9.0.2
core-r3:
  hostname: 10.9.0.3
";
    let report = import(hosts, None, None, "Core").run(&mut ws, &[]).unwrap();
    assert_eq!(created(&report), ["Core/core-r3.yaml"]);
    assert_eq!(
        entries(&report.skipped),
        [
            (
                Some(1),
                "core-r1".to_string(),
                Some(
                    "Core/core-r1.yaml already exists; it is only replaced when you confirm it"
                        .to_string()
                )
            ),
            (
                Some(7),
                "core-r2".to_string(),
                Some(
                    "Core/core-r2.yml already exists; it is only replaced when you confirm it"
                        .to_string()
                )
            ),
        ]
    );
    assert_eq!(read(&root, "Core/core-r1.yaml"), r1_before);
    assert_eq!(read(&root, "Core/core-r2.yml"), r2_before);

    // Confirmed: it takes the imported values and keeps the rest.
    let report = import(hosts, None, None, "Core")
        .run(&mut ws, &[id("Core/core-r1.yaml")])
        .unwrap();
    assert!(report.created.is_empty());
    assert_eq!(names(&report.updated), ["core-r1"]);
    assert_eq!(names(&report.skipped), ["core-r2", "core-r3"]);
    let r1 = ws.host(&id("Core/core-r1.yaml")).unwrap();
    assert_eq!(r1.address, "10.9.0.1");
    assert_eq!(r1.platform.as_deref(), Some("eos"));
    assert_eq!(r1.vendor.as_deref(), Some("Arista"));
    assert_eq!(r1.tags, ["imported"]);
    assert_eq!(
        custom(&ws, "Core/core-r1.yaml"),
        [
            ("contract".to_string(), "ACME-1234".to_string()),
            ("owner".to_string(), "nornir".to_string()),
        ]
    );
    assert_eq!(read(&root, "Core/core-r2.yml"), r2_before);
}

#[test]
fn a_host_created_after_the_preview_is_not_overwritten() {
    let (_temp, root, mut ws) = workspace();
    let nornir = import("late:\n  hostname: 10.9.0.9\n", None, None, "Empty");
    let preview = nornir.preview(&ws).unwrap();
    assert!(matches!(
        preview.hosts[0].outcome,
        PreviewOutcome::Create { .. }
    ));
    // Someone else writes it meanwhile (the workspace is not reloaded).
    fs::write(root.join("Empty/LATE.yaml"), "address: 10.0.0.99\n").unwrap();

    let report = nornir.run(&mut ws, &[]).unwrap();
    assert!(report.created.is_empty());
    assert_eq!(names(&report.skipped), ["late"]);
    assert_eq!(read(&root, "Empty/LATE.yaml"), "address: 10.0.0.99\n");
}
