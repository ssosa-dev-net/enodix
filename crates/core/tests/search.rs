//! Host search, as the Ctrl+K palette uses it: an in-memory index over the
//! workspace, rebuilt from the files, that matches part of a host's name,
//! management address, tags (inherited ones included), built-in fields and
//! custom field values.

mod support;

use std::fs;
use std::time::{Duration, Instant};

use netterm_core::workspace::{
    Condition, HostId, SavedFilter, SearchResults, Workspace, FILTERS_FILE,
};
use support::workspaces::{fixture, fixture_copy};
use tempfile::TempDir;

fn search_fixture() -> Workspace {
    let workspace = Workspace::open(fixture("search")).unwrap();
    // The bad filters are reported; every host loads.
    assert!(workspace
        .problems()
        .iter()
        .all(|problem| problem.file == FILTERS_FILE));
    workspace
}

fn ids(results: &SearchResults) -> Vec<&str> {
    results.hits.iter().map(|hit| hit.id.as_str()).collect()
}

/// The hosts `query` finds, best first.
fn found(workspace: &Workspace, query: &str) -> Vec<String> {
    ids(&workspace.search(query, 50))
        .into_iter()
        .map(String::from)
        .collect()
}

/// The fields of `id` that matched `query`, as `field=value`.
fn matched(workspace: &Workspace, query: &str, id: &str) -> Vec<String> {
    let results = workspace.search(query, 50);
    let hit = results
        .hits
        .iter()
        .find(|hit| hit.id.as_str() == id)
        .unwrap_or_else(|| panic!("{query:?} did not find {id}: {:?}", ids(&results)));
    hit.matches
        .iter()
        .map(|found| format!("{}={}", found.field, found.value))
        .collect()
}

#[test]
fn part_of_a_name_finds_the_host_ignoring_case() {
    let workspace = search_fixture();

    assert_eq!(
        found(&workspace, "MAD-"),
        ["Madrid/mad-fw1.yaml", "Madrid/mad-sw1.yaml"]
    );
    assert_eq!(found(&workspace, "fw1"), ["Madrid/mad-fw1.yaml"]);
    assert_eq!(
        matched(&workspace, "fw1", "Madrid/mad-fw1.yaml"),
        ["name=mad-fw1"]
    );
    let hit = &workspace.search("lis", 50).hits[0];
    assert_eq!(hit.name, "lis-sw1");
    assert_eq!(hit.address, "10.2.0.2");
}

#[test]
fn part_of_a_management_address_finds_the_host() {
    let workspace = search_fixture();

    assert_eq!(found(&workspace, "10.2.0"), ["Lisbon/lis-sw1.yaml"]);
    assert_eq!(found(&workspace, "EXAMPLE.net"), ["core-r1.yaml"]);
    assert_eq!(
        matched(&workspace, "10.1.0.2", "Madrid/mad-sw1.yaml"),
        ["address=10.1.0.2"]
    );
    // An address prefix finds every host in the range.
    assert_eq!(
        found(&workspace, "10.1."),
        ["Madrid/mad-fw1.yaml", "Madrid/mad-sw1.yaml"]
    );
}

#[test]
fn a_tag_finds_its_hosts_including_tags_inherited_from_folders() {
    let workspace = search_fixture();

    assert_eq!(found(&workspace, "perim"), ["Madrid/mad-fw1.yaml"]);
    assert_eq!(
        matched(&workspace, "perimeter", "Madrid/mad-fw1.yaml"),
        ["tag=perimeter"]
    );
    // `spain` is set on the Madrid folder.
    assert_eq!(
        found(&workspace, "spain"),
        ["Madrid/mad-fw1.yaml", "Madrid/mad-sw1.yaml"]
    );
    assert_eq!(
        matched(&workspace, "spain", "Madrid/mad-sw1.yaml"),
        ["tag=spain"]
    );
}

#[test]
fn each_built_in_field_is_searched() {
    let workspace = search_fixture();

    let cases = [
        ("fortinet", "Madrid/mad-fw1.yaml", "vendor=Fortinet"),
        ("eos", "Lisbon/lis-sw1.yaml", "platform=eos"),
        ("7050", "Madrid/mad-sw1.yaml", "model=7050SX"),
        ("jpe154", "Madrid/mad-sw1.yaml", "serial_number=JPE15400001"),
        ("lisbon", "Lisbon/lis-sw1.yaml", "site=Lisbon"),
        ("torre", "Lisbon/lis-sw1.yaml", "building=Torre B"),
        ("sala 101", "Lisbon/lis-sw1.yaml", "room=Sala 101"),
        ("r12", "Madrid/mad-sw1.yaml", "rack=R12"),
        ("40", "Madrid/mad-sw1.yaml", "rack_unit=40"),
        (
            "backbone router",
            "core-r1.yaml",
            "description=Backbone router, north",
        ),
        // A role is found by its id and by its name.
        ("firewall", "Madrid/mad-fw1.yaml", "role=Firewall"),
        ("juniper", "core-r1.yaml", "vendor=Juniper"),
    ];
    for (query, id, field) in cases {
        assert_eq!(found(&workspace, query), [id], "{query}");
        assert!(
            matched(&workspace, query, id).contains(&field.to_string()),
            "{query}: {:?}",
            matched(&workspace, query, id)
        );
    }
    assert_eq!(found(&workspace, "Access"), ["Lisbon/lis-sw1.yaml"]);
    assert_eq!(
        matched(&workspace, "access", "Lisbon/lis-sw1.yaml"),
        ["role=Access"]
    );
}

#[test]
fn a_role_is_found_by_its_name() {
    let (_temp, root) = fixture_copy("search");
    fs::write(
        root.join("_roles.yaml"),
        "spine:\n  name: Spine switch\n  icon: switch\n  color: '#2563eb'\n",
    )
    .unwrap();
    fs::write(root.join("sp1.yaml"), "address: 10.9.0.1\nrole: spine\n").unwrap();
    let workspace = Workspace::open(&root).unwrap();

    assert_eq!(found(&workspace, "spine switch"), ["sp1.yaml"]);
    assert_eq!(
        matched(&workspace, "spine switch", "sp1.yaml"),
        ["role=spine", "role=Spine switch"]
    );
}

#[test]
fn custom_field_values_are_searched() {
    let workspace = search_fixture();

    assert_eq!(found(&workspace, "netops"), ["Madrid/mad-sw1.yaml"]);
    assert_eq!(
        matched(&workspace, "netops", "Madrid/mad-sw1.yaml"),
        ["custom.owner=netops"]
    );
    assert_eq!(found(&workspace, "cid-100"), ["Madrid/mad-fw1.yaml"]);
}

#[test]
fn every_word_must_match_somewhere() {
    let workspace = search_fixture();

    assert_eq!(
        found(&workspace, "arista"),
        ["Lisbon/lis-sw1.yaml", "Madrid/mad-sw1.yaml"]
    );
    assert_eq!(found(&workspace, "arista  madrid"), ["Madrid/mad-sw1.yaml"]);
    assert_eq!(
        matched(&workspace, "arista madrid", "Madrid/mad-sw1.yaml"),
        ["vendor=Arista", "site=Madrid"]
    );
    assert!(found(&workspace, "arista juniper").is_empty());
    assert!(found(&workspace, "nothing-like-this").is_empty());
}

#[test]
fn name_matches_come_before_matches_in_other_fields() {
    let (_temp, root) = fixture_copy("search");
    // `edge` is this host's description, and part of another's name.
    fs::write(
        root.join("aaa.yaml"),
        "address: 10.9.0.1\ndescription: edge of the lab\n",
    )
    .unwrap();
    fs::write(root.join("lab-edge1.yaml"), "address: 10.9.0.2\n").unwrap();
    fs::write(root.join("edge.yaml"), "address: 10.9.0.3\n").unwrap();
    let workspace = Workspace::open(&root).unwrap();

    // The exact name first, then a name containing it, then the rest.
    assert_eq!(
        found(&workspace, "edge"),
        ["edge.yaml", "lab-edge1.yaml", "aaa.yaml"]
    );
}

#[test]
fn an_empty_query_lists_every_host_by_name_up_to_the_limit() {
    let workspace = search_fixture();

    let all = workspace.search("  ", 50);
    assert_eq!(all.total, 4);
    assert_eq!(
        ids(&all),
        [
            "core-r1.yaml",
            "Lisbon/lis-sw1.yaml",
            "Madrid/mad-fw1.yaml",
            "Madrid/mad-sw1.yaml"
        ]
    );
    let first = workspace.search("", 2);
    assert_eq!(first.total, 4);
    assert_eq!(ids(&first), ["core-r1.yaml", "Lisbon/lis-sw1.yaml"]);
    // The limit applies to matches too; the total counts them all.
    let limited = workspace.search("1", 1);
    assert_eq!(limited.total, 4);
    assert_eq!(limited.hits.len(), 1);
}

#[test]
fn the_index_follows_edits_to_the_files() {
    let (_temp, root) = fixture_copy("search");
    let mut workspace = Workspace::open(&root).unwrap();

    let id = HostId::new("Lisbon/lis-sw1.yaml");
    let mut host = workspace.host(&id).unwrap().clone();
    host.vendor = Some("Nokia".into());
    workspace.write_host(&host).unwrap();
    assert_eq!(found(&workspace, "nokia"), ["Lisbon/lis-sw1.yaml"]);
    assert_eq!(found(&workspace, "arista"), ["Madrid/mad-sw1.yaml"]);

    let new = workspace
        .create_host("Lisbon", "lis-fw1", "10.2.0.1")
        .unwrap();
    assert_eq!(found(&workspace, "lis-fw"), [new.as_str()]);
    workspace.delete_host(&new).unwrap();
    assert!(found(&workspace, "lis-fw").is_empty());

    // A folder tag applies to the hosts moved below it.
    workspace
        .move_host(&HostId::new("core-r1.yaml"), "Madrid")
        .unwrap();
    assert_eq!(found(&workspace, "spain core"), ["Madrid/core-r1.yaml"]);
}

/// A workspace of `count` hosts in 50 sites of 4 rooms each, with every
/// kind of field set.
fn generated_workspace(count: usize) -> TempDir {
    let temp = TempDir::new().unwrap();
    for index in 0..count {
        let site = format!("site{:02}", index % 50);
        let room = format!("room{}", index % 4);
        let folder = temp.path().join(&site).join(&room);
        fs::create_dir_all(&folder).unwrap();
        let vendor = ["Cisco", "Arista", "Juniper", "Fortinet"][index % 4];
        let role = ["core", "access", "firewall", "server"][index % 4];
        fs::write(
            folder.join(format!("host-{index:05}.yaml")),
            format!(
                "address: 10.{}.{}.{}\nvendor: {vendor}\nmodel: M{}\nserial_number: SN{index:08}\n\
                 site: {site}\nroom: {room}\nrack: R{}\nrole: {role}\n\
                 description: generated host number {index}\ntags: [gen, t{}]\n\
                 custom:\n  circuit: CID-{index}\n  owner: team{}\n",
                index / 65536,
                (index / 256) % 256,
                index % 256,
                index % 30,
                index % 40,
                index % 10,
                index % 7,
            ),
        )
        .unwrap();
    }
    temp
}

#[test]
fn searching_5000_hosts_takes_no_perceptible_time() {
    let temp = generated_workspace(5000);
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(workspace.problems().is_empty());
    assert_eq!(workspace.search("", 1).total, 5000);

    // As typed in the palette, one keystroke at a time, plus queries that
    // match everything, nothing, and a field deep in each host.
    let mut queries: Vec<String> = Vec::new();
    for typed in ["host-04999", "10.0.19", "cid-4321", "arista site07 room2"] {
        for end in 1..=typed.len() {
            queries.push(typed[..end].to_string());
        }
    }
    queries.extend([
        "gen".into(),
        "zzz-nothing".into(),
        "team6".into(),
        "e".into(),
    ]);

    // In a debug build this is well under the budget; a release build is
    // many times faster still.
    let budget = Duration::from_millis(100);
    let mut slowest = Duration::ZERO;
    for query in &queries {
        let started = Instant::now();
        let results = workspace.search(query, 50);
        let took = started.elapsed();
        slowest = slowest.max(took);
        assert!(took < budget, "{query:?} took {took:?}");
        assert!(results.hits.len() <= 50);
    }
    println!("slowest of {} queries: {slowest:?}", queries.len());

    assert_eq!(
        ids(&workspace.search("host-04999", 50)),
        ["site49/room3/host-04999.yaml"]
    );
    assert_eq!(workspace.search("gen", 50).total, 5000);
    assert_eq!(workspace.search("arista site07 room2", 50).total, 0);
    assert_eq!(workspace.search("arista site07 room1", 50).total, 50);
    assert_eq!(
        ids(&workspace.search("cid-4321", 50)),
        ["site21/room1/host-04321.yaml"]
    );

    // Viewing the tree through a saved filter is as quick.
    let filters = [
        filter(&[("vendor", "arista")]),
        filter(&[("tag", "t3"), ("custom.owner", "team*")]),
        filter(&[("text", "generated 4")]),
    ];
    for saved in &filters {
        let started = Instant::now();
        let view = workspace.apply_filter(saved);
        let took = started.elapsed();
        assert!(took < budget, "{saved:?} took {took:?}");
        assert!(!view.hosts.is_empty());
    }
}

fn filter(conditions: &[(&str, &str)]) -> SavedFilter {
    SavedFilter {
        name: "Timed".into(),
        conditions: conditions
            .iter()
            .map(|(field, value)| Condition::new(*field, *value))
            .collect(),
    }
}

#[test]
fn an_inherited_platform_is_searched_and_a_hosts_own_wins() {
    let (_temp, root) = fixture_copy("search");
    fs::write(
        root.join("Madrid/_folder.yaml"),
        "tags: [spain]\nplatform: nxos\n",
    )
    .unwrap();
    // Lisbon's switch sets `eos` itself.
    fs::write(root.join("Lisbon/_folder.yaml"), "platform: ios\n").unwrap();
    let workspace = Workspace::open(&root).unwrap();

    assert_eq!(
        found(&workspace, "nxos"),
        ["Madrid/mad-fw1.yaml", "Madrid/mad-sw1.yaml"]
    );
    assert_eq!(
        matched(&workspace, "nxos", "Madrid/mad-sw1.yaml"),
        ["platform=nxos"]
    );
    assert_eq!(found(&workspace, "eos"), ["Lisbon/lis-sw1.yaml"]);
    assert!(found(&workspace, "ios").is_empty());
}
