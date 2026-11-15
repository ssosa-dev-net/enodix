//! What the app keeps of each session (#84), so that any window can show
//! it: its name and title, the host it goes to, the facts its header shows
//! and its paste safety, as they were when it opened.

use std::fs;

use netterm_core::paste::PasteSafety;
use netterm_core::session::{SessionId, SessionRecord, SessionRecords, Transport};
use netterm_core::workspace::{Environment, HostId, Workspace};
use serde_json::json;
use tempfile::TempDir;

/// `Prod/edge-fw` is a production firewall reached through `bastion`,
/// with a paste delay; `lab-r1` has nothing set.
fn workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let write = |file: &str, text: &str| {
        let path = temp.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "Prod/_folder.yaml",
        "environment: production\njump_host: bastion.yaml\npaste_line_delay: 150\n",
    );
    write("Prod/edge-fw.yaml", "address: 10.0.0.1\nrole: firewall\n");
    write("bastion.yaml", "address: 10.0.0.9\n");
    write("lab-r1.yaml", "address: 10.0.0.5\n");
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[test]
fn a_host_sessions_record_holds_what_its_header_shows_as_the_workspace_set_it() {
    let (_temp, workspace) = workspace();
    let id = HostId::new("Prod/edge-fw.yaml");

    let record = SessionRecord::for_host(&workspace, &id, Transport::Ssh, "edge-fw", "edge-fw")
        .expect("the host is in the workspace");

    assert_eq!(record.name, "edge-fw");
    assert_eq!(record.title, "edge-fw");
    assert_eq!(record.host, Some(id));
    assert_eq!(record.workspace.as_deref(), Some(workspace.root()));
    // The title does not show the address, so the header does.
    assert_eq!(record.address.as_deref(), Some("10.0.0.1"));
    assert_eq!(record.jump_host.as_deref(), Some("bastion.yaml"));
    assert_eq!(
        record.safety,
        PasteSafety {
            environment: Some(Environment::Production),
            line_delay_ms: 150,
        }
    );
    assert_eq!(record.role.map(|role| role.id), Some("firewall".into()));
}

#[test]
fn a_title_showing_the_address_leaves_it_out_and_telnet_never_has_a_jump_host() {
    let (_temp, workspace) = workspace();
    let id = HostId::new("Prod/edge-fw.yaml");

    let record = SessionRecord::for_host(
        &workspace,
        &id,
        Transport::Telnet,
        "edge-fw",
        "edge-fw (10.0.0.1:23)",
    )
    .unwrap();

    assert_eq!(record.address, None);
    assert_eq!(record.jump_host, None);
}

#[test]
fn a_host_with_nothing_set_and_a_host_not_in_the_workspace() {
    let (_temp, workspace) = workspace();

    let plain = SessionRecord::for_host(
        &workspace,
        &HostId::new("lab-r1.yaml"),
        Transport::Ssh,
        "lab-r1",
        "lab-r1",
    )
    .unwrap();

    assert_eq!(plain.safety, PasteSafety::default());
    assert_eq!(plain.role, None);
    assert_eq!(plain.jump_host, None);
    assert!(SessionRecord::for_host(
        &workspace,
        &HostId::new("gone.yaml"),
        Transport::Ssh,
        "gone",
        "gone"
    )
    .is_none());
}

#[test]
fn a_quick_connection_has_only_its_name_and_title() {
    let record = SessionRecord::new("10.1.1.1", "admin@10.1.1.1:22");

    assert_eq!(
        serde_json::to_value(&record).unwrap(),
        json!({
            "name": "10.1.1.1",
            "title": "admin@10.1.1.1:22",
            "address": null,
            "jumpHost": null,
            "host": null,
            "workspace": null,
            "safety": { "environment": null, "lineDelayMs": 0 },
            "role": null,
        })
    );
}

#[test]
fn the_app_keeps_each_sessions_record_until_it_is_forgotten() {
    let records = SessionRecords::new();
    records.record(SessionId(1), SessionRecord::new("a", "a"));

    assert_eq!(records.get(SessionId(1)).map(|r| r.name), Some("a".into()));
    assert!(records.get(SessionId(2)).is_none());

    // A duplicate shows what its original showed.
    assert!(records.copy(SessionId(1), SessionId(2)));
    assert_eq!(records.get(SessionId(2)).map(|r| r.name), Some("a".into()));
    assert!(!records.copy(SessionId(9), SessionId(10)));

    records.forget(SessionId(1));
    assert!(records.get(SessionId(1)).is_none());
    assert!(records.get(SessionId(2)).is_some());
}

#[test]
fn a_records_host_counts_only_in_its_own_workspace_while_it_is_there() {
    let (temp, workspace) = workspace();
    let id = HostId::new("Prod/edge-fw.yaml");
    let record =
        SessionRecord::for_host(&workspace, &id, Transport::Ssh, "edge-fw", "edge-fw").unwrap();
    let (_other_temp, other) = self::workspace();

    assert_eq!(record.host_in(&workspace), Some(&id));
    assert_eq!(record.host_in(&other), None);
    assert_eq!(SessionRecord::new("a", "a").host_in(&workspace), None);

    fs::remove_file(temp.path().join("Prod/edge-fw.yaml")).unwrap();
    let reopened = Workspace::open(temp.path()).unwrap();
    assert_eq!(record.host_in(&reopened), None);
    // What it opened with stays: moving cannot weaken a safeguard.
    assert_eq!(record.safety.environment, Some(Environment::Production));
}

#[test]
fn a_records_paste_safety_is_its_hosts_as_set_now_else_as_it_opened() {
    let (temp, workspace) = workspace();
    let id = HostId::new("Prod/edge-fw.yaml");
    let record =
        SessionRecord::for_host(&workspace, &id, Transport::Ssh, "edge-fw", "edge-fw").unwrap();
    let opened = record.safety;
    assert_eq!(record.safety_in(&workspace), opened);

    // The workspace changed the host: its pastes follow the change.
    fs::write(
        temp.path().join("Prod/_folder.yaml"),
        "environment: staging
",
    )
    .unwrap();
    let edited = Workspace::open(temp.path()).unwrap();
    assert_eq!(
        record.safety_in(&edited),
        PasteSafety {
            environment: Some(Environment::Staging),
            ..PasteSafety::default()
        }
    );

    // Open in another workspace (whose host of that id is production
    // too): what it opened with, not that host's.
    let (_other_temp, other) = self::workspace();
    let mut opened_lax = record.clone();
    opened_lax.safety = PasteSafety::default();
    assert_eq!(opened_lax.safety_in(&other), PasteSafety::default());
    // The host gone: as it opened.
    fs::remove_file(temp.path().join("Prod/edge-fw.yaml")).unwrap();
    let removed = Workspace::open(temp.path()).unwrap();
    assert_eq!(record.safety_in(&removed), opened);
}

#[test]
fn the_app_gives_each_sessions_paste_safety_with_or_without_a_workspace() {
    let (_temp, workspace) = workspace();
    let id = HostId::new("Prod/edge-fw.yaml");
    let record =
        SessionRecord::for_host(&workspace, &id, Transport::Ssh, "edge-fw", "edge-fw").unwrap();
    let opened = record.safety;
    let records = SessionRecords::new();
    records.record(SessionId(1), record);

    assert_eq!(records.paste_safety(SessionId(1), Some(&workspace)), opened);
    // No workspace open: as it opened.
    assert_eq!(records.paste_safety(SessionId(1), None), opened);
    // A session the app has no record of: no safeguard to apply.
    assert_eq!(
        records.paste_safety(SessionId(2), Some(&workspace)),
        PasteSafety::default()
    );
}
