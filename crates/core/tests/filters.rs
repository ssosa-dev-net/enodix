//! Saved filters: named sets of conditions kept in the workspace's
//! `_filters.yaml`, listed, applied to the tree, saved and deleted.

mod support;

use std::fs;

use enodix_core::workspace::{
    Condition, EditError, FilterView, HostId, SavedFilter, Workspace, FILTERS_FILE,
};
use support::workspaces::{files, fixture, fixture_copy, read};
use tempfile::TempDir;

fn filter(name: &str, conditions: &[(&str, &str)]) -> SavedFilter {
    SavedFilter {
        name: name.into(),
        conditions: conditions
            .iter()
            .map(|(field, value)| Condition::new(*field, *value))
            .collect(),
    }
}

fn named<'a>(workspace: &'a Workspace, name: &str) -> &'a SavedFilter {
    workspace
        .filters()
        .iter()
        .find(|filter| filter.name == name)
        .unwrap_or_else(|| panic!("no filter {name}"))
}

fn hosts(view: &FilterView) -> Vec<&str> {
    view.hosts.iter().map(HostId::as_str).collect()
}

#[test]
fn saved_filters_are_listed_in_file_order() {
    let workspace = Workspace::open(fixture("search")).unwrap();

    let names: Vec<&str> = workspace
        .filters()
        .iter()
        .map(|filter| filter.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Firewalls",
            "Arista in Madrid",
            "Circuits",
            "Spanish switches",
            "Core text"
        ]
    );
    assert_eq!(
        named(&workspace, "Arista in Madrid"),
        &filter(
            "Arista in Madrid",
            &[("vendor", "arista"), ("site", "Madrid")]
        )
    );
    assert_eq!(
        named(&workspace, "Circuits"),
        &filter("Circuits", &[("custom.circuit", "CID-*")])
    );
    assert_eq!(
        named(&workspace, "Spanish switches"),
        &filter("Spanish switches", &[("tag", "spain"), ("name", "*-sw*")])
    );
}

#[test]
fn badly_defined_filters_are_reported_and_left_out() {
    let workspace = Workspace::open(fixture("search")).unwrap();

    let problems: Vec<String> = workspace
        .problems()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        problems,
        [
            "_filters.yaml: filter `Colour`: unknown field `colour`",
            "_filters.yaml: filter `Empty`: it has no conditions",
        ]
    );
}

#[test]
fn an_unreadable_filters_file_is_a_problem() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(FILTERS_FILE), "- a\n- list\n").unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    assert!(workspace.filters().is_empty());
    assert_eq!(workspace.problems().len(), 1);
    assert_eq!(workspace.problems()[0].file, FILTERS_FILE);
}

#[test]
fn a_filter_shows_the_hosts_it_matches_and_the_folders_holding_them() {
    let workspace = Workspace::open(fixture("search")).unwrap();
    let apply = |name: &str| workspace.apply_filter(named(&workspace, name));

    let firewalls = apply("Firewalls");
    assert_eq!(hosts(&firewalls), ["Madrid/mad-fw1.yaml"]);
    assert_eq!(firewalls.folders, ["Madrid"]);

    // Values match whole, ignoring case.
    assert_eq!(hosts(&apply("Arista in Madrid")), ["Madrid/mad-sw1.yaml"]);
    // `*` stands for any text.
    assert_eq!(hosts(&apply("Circuits")), ["Madrid/mad-fw1.yaml"]);
    // Tags include those inherited from folders.
    assert_eq!(hosts(&apply("Spanish switches")), ["Madrid/mad-sw1.yaml"]);
    // `text` searches as the palette does.
    let text = apply("Core text");
    assert_eq!(hosts(&text), ["core-r1.yaml"]);
    assert!(text.folders.is_empty());
}

#[test]
fn each_kind_of_condition_matches() {
    let workspace = Workspace::open(fixture("search")).unwrap();
    let matching = |conditions: &[(&str, &str)]| {
        let view = workspace.apply_filter(&filter("Test", conditions));
        view.hosts
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
    };

    assert_eq!(matching(&[("name", "LIS-SW1")]), ["Lisbon/lis-sw1.yaml"]);
    assert_eq!(matching(&[("address", "10.1.*")]).len(), 2);
    assert_eq!(matching(&[("vendor", "Aris")]), Vec::<String>::new());
    assert_eq!(matching(&[("platform", "eos")]), ["Lisbon/lis-sw1.yaml"]);
    assert_eq!(matching(&[("model", "7050*")]), ["Madrid/mad-sw1.yaml"]);
    assert_eq!(
        matching(&[("serial_number", "jpe15400001")]),
        ["Madrid/mad-sw1.yaml"]
    );
    assert_eq!(matching(&[("site", "madrid")]).len(), 2);
    assert_eq!(
        matching(&[("building", "Torre B")]),
        ["Lisbon/lis-sw1.yaml"]
    );
    assert_eq!(matching(&[("room", "*101")]), ["Lisbon/lis-sw1.yaml"]);
    assert_eq!(matching(&[("rack", "R12")]), ["Madrid/mad-sw1.yaml"]);
    assert_eq!(matching(&[("rack_unit", "40")]), ["Madrid/mad-sw1.yaml"]);
    assert_eq!(matching(&[("description", "*north")]), ["core-r1.yaml"]);
    // A role by id or by name.
    assert_eq!(matching(&[("role", "Firewall")]), ["Madrid/mad-fw1.yaml"]);
    assert_eq!(matching(&[("tag", "perimeter")]), ["Madrid/mad-fw1.yaml"]);
    assert_eq!(
        matching(&[("custom.owner", "netops")]),
        ["Madrid/mad-sw1.yaml"]
    );
    // Every condition must hold.
    assert_eq!(
        matching(&[("vendor", "arista"), ("tag", "spain")]),
        ["Madrid/mad-sw1.yaml"]
    );
    assert_eq!(
        matching(&[("text", "10.")]),
        [
            "Lisbon/lis-sw1.yaml",
            "Madrid/mad-fw1.yaml",
            "Madrid/mad-sw1.yaml"
        ]
    );
}

#[test]
fn a_saved_filter_is_written_to_the_filters_file() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("r1.yaml"),
        "address: 10.0.0.1\nvendor: Cisco\n",
    )
    .unwrap();
    let mut workspace = Workspace::open(temp.path()).unwrap();

    workspace
        .save_filter(&filter(
            " Cisco lab ",
            &[
                ("vendor", " Cisco "),
                ("tag", "core"),
                ("custom. owner ", "noc"),
                ("tag", "lab"),
                ("text", "edge"),
            ],
        ))
        .unwrap();
    assert_eq!(
        read(temp.path(), FILTERS_FILE),
        "\
Cisco lab:
  vendor: Cisco
  tags:
  - core
  - lab
  custom:
    owner: noc
  text: edge
"
    );
    assert_eq!(
        workspace.filters(),
        [filter(
            "Cisco lab",
            &[
                ("vendor", "Cisco"),
                ("tag", "core"),
                ("tag", "lab"),
                ("custom.owner", "noc"),
                ("text", "edge"),
            ]
        )]
    );

    workspace
        .save_filter(&filter("Ciscos", &[("vendor", "cisco")]))
        .unwrap();
    assert_eq!(
        read(temp.path(), FILTERS_FILE),
        "\
Cisco lab:
  vendor: Cisco
  tags:
  - core
  - lab
  custom:
    owner: noc
  text: edge
Ciscos:
  vendor: cisco
"
    );
    let ciscos = workspace.apply_filter(&workspace.filters()[1]);
    assert_eq!(hosts(&ciscos), ["r1.yaml"]);
}

#[test]
fn saving_under_an_existing_name_replaces_that_filter_in_place() {
    let (_temp, root) = fixture_copy("search");
    let mut workspace = Workspace::open(&root).unwrap();

    workspace
        .save_filter(&filter(
            "FIREWALLS",
            &[("role", "firewall"), ("site", "Lisbon")],
        ))
        .unwrap();
    let names: Vec<&str> = workspace
        .filters()
        .iter()
        .map(|filter| filter.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "FIREWALLS",
            "Arista in Madrid",
            "Circuits",
            "Spanish switches",
            "Core text"
        ]
    );
    assert!(workspace
        .apply_filter(&workspace.filters()[0])
        .hosts
        .is_empty());
    // The badly defined filters are still in the file, still reported.
    assert_eq!(workspace.problems().len(), 2);
    assert!(read(&root, FILTERS_FILE).contains("Colour:\n  colour: red\n"));
}

#[test]
fn a_deleted_filter_leaves_the_file_and_the_last_one_removes_it() {
    let (_temp, root) = fixture_copy("search");
    let mut workspace = Workspace::open(&root).unwrap();

    workspace.delete_filter("arista in madrid").unwrap();
    assert!(!read(&root, FILTERS_FILE).contains("Arista"));
    assert_eq!(workspace.filters().len(), 4);

    for name in [
        "Firewalls",
        "Circuits",
        "Spanish switches",
        "Core text",
        "Colour",
        "Empty",
    ] {
        workspace.delete_filter(name).unwrap();
    }
    assert!(workspace.filters().is_empty());
    assert!(!root.join(FILTERS_FILE).exists());
    assert!(workspace.problems().is_empty());

    assert!(matches!(
        workspace.delete_filter("Firewalls"),
        Err(EditError::NotFound(_))
    ));
}

#[test]
fn a_bad_filter_is_refused_and_no_file_changes() {
    let (_temp, root) = fixture_copy("search");
    let mut workspace = Workspace::open(&root).unwrap();
    let before = files(&root);
    let text = read(&root, FILTERS_FILE);

    let refused = [
        filter("  ", &[("vendor", "Cisco")]),
        filter("None", &[]),
        filter("Blank", &[("vendor", "  ")]),
        filter("Unknown", &[("colour", "red")]),
        filter("No key", &[("custom.", "x")]),
        filter("Twice", &[("vendor", "Cisco"), ("vendor", "Arista")]),
    ];
    for bad in refused {
        let result = workspace.save_filter(&bad);
        assert!(
            matches!(result, Err(EditError::Invalid(_))),
            "{}: {result:?}",
            bad.name
        );
    }
    assert_eq!(files(&root), before);
    assert_eq!(read(&root, FILTERS_FILE), text);
    assert_eq!(workspace.filters().len(), 5);
}

#[test]
fn a_platform_condition_matches_the_platform_that_applies() {
    let (_temp, root) = fixture_copy("search");
    fs::write(
        root.join("Madrid/_folder.yaml"),
        "tags: [spain]\nplatform: nxos\n",
    )
    .unwrap();
    fs::write(root.join("Lisbon/_folder.yaml"), "platform: ios\n").unwrap();
    let workspace = Workspace::open(&root).unwrap();
    let matching = |value: &str| {
        let view = workspace.apply_filter(&filter("Test", &[("platform", value)]));
        view.hosts
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
    };

    assert_eq!(
        matching("NXOS"),
        ["Madrid/mad-fw1.yaml", "Madrid/mad-sw1.yaml"]
    );
    // A host's own platform replaces its folder's.
    assert_eq!(matching("eos"), ["Lisbon/lis-sw1.yaml"]);
    assert!(matching("ios").is_empty());
}
