//! Snippets and the templating they share with web links: variables filled
//! from a resolved host's fields, variables without a value, multi-line
//! expansion, and the snippets file (editable from the app).

mod support;

use std::fs;

use netterm_core::template::{MissingVariables, Template, Variables};
use netterm_core::workspace::{EditError, ExpandError, HostId, Snippet, Workspace, SNIPPETS_FILE};
use support::workspaces::read;
use tempfile::TempDir;

/// A workspace with a host in a folder (which tags it and sets a web link)
/// and, if given, a snippets file holding `snippets`.
fn workspace_with(snippets: Option<&str>) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("Madrid")).unwrap();
    fs::write(
        root.join("Madrid/_folder.yaml"),
        "tags: [spain]\nweb_links:\n  GUI: https://{{address}}:{{custom.gui_port}}/login\n",
    )
    .unwrap();
    fs::write(
        root.join("Madrid/r1.yaml"),
        "address: 10.0.0.1\nvendor: cisco\nsite: MAD1\nrack_unit: 12\ntags: [core]\ncustom:\n  circuit: CID-42\n  gui_port: 8443\n  blank: ''\n",
    )
    .unwrap();
    if let Some(snippets) = snippets {
        fs::write(root.join(SNIPPETS_FILE), snippets).unwrap();
    }
    let workspace = Workspace::open(root).unwrap();
    (temp, workspace)
}

fn r1() -> HostId {
    HostId::new("Madrid/r1.yaml")
}

fn host_variables(workspace: &Workspace) -> Variables {
    workspace.resolve_host(&r1()).unwrap().variables()
}

fn fill(template: &str, values: &Variables) -> Result<String, MissingVariables> {
    Template::parse(template).fill(values)
}

fn names(missing: MissingVariables) -> Vec<String> {
    missing.names
}

fn lines(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|line| line.to_string()).collect()
}

// --- Templating ---

#[test]
fn built_in_and_custom_fields_fill_variables() {
    let (_temp, workspace) = workspace_with(None);
    let values = host_variables(&workspace);
    assert_eq!(
        fill(
            "ping {{address}} from {{ name }} at {{Site}} ({{vendor}}, U{{rack_unit}}) on {{circuit}} / {{custom.circuit}}",
            &values,
        )
        .unwrap(),
        "ping 10.0.0.1 from r1 at MAD1 (cisco, U12) on CID-42 / CID-42"
    );
    // Tags include the inherited ones.
    assert_eq!(fill("{{tags}}", &values).unwrap(), "spain, core");
}

#[test]
fn fields_without_a_value_are_missing() {
    let (_temp, workspace) = workspace_with(None);
    let values = host_variables(&workspace);
    // `model` is not set, `blank` is set to nothing, `vlan` is no field.
    let missing = fill("{{model}} {{vlan}} {{blank}} {{VLAN}} {{address}}", &values).unwrap_err();
    assert_eq!(names(missing), ["model", "vlan", "blank"]);
}

#[test]
fn a_template_lists_its_variables_once_each_in_order() {
    let template = Template::parse("{{b}} {{a}} {{ B }} {{custom.x}} {{a}}");
    assert_eq!(template.variables(), ["b", "a", "custom.x"]);
    assert!(Template::parse("show version").variables().is_empty());
}

#[test]
fn text_that_is_not_a_variable_is_kept_as_written() {
    let values: Variables = [("a", "1")].into_iter().collect();
    for text in [
        "{{",
        "}}",
        "{{}}",
        "{{  }}",
        "{a}",
        "{ {a} }",
        "{{a\nb}}",
        "{{a}",
        "{{ {a} }}",
        "printf '%s' {{",
    ] {
        assert_eq!(fill(text, &values).unwrap(), text, "{text:?}");
    }
    assert_eq!(fill("{{{a}}}", &values).unwrap(), "{1}");
    assert_eq!(fill("x {{ a }}{{a}} }} {{", &values).unwrap(), "x 11 }} {{");
}

#[test]
fn values_are_inserted_as_they_are() {
    let values: Variables = [("a", "{{b}}"), ("b", "no")].into_iter().collect();
    assert_eq!(fill("<{{a}}>", &values).unwrap(), "<{{b}}>");
}

#[test]
fn later_values_replace_earlier_ones_ignoring_case() {
    let mut values = Variables::new();
    values.set("VLAN", "10");
    values.set("vlan", "20");
    assert_eq!(values.get("Vlan"), Some("20"));
    assert_eq!(fill("{{vlan}}", &values).unwrap(), "20");
}

#[test]
fn web_links_fill_with_the_same_templating() {
    let (_temp, workspace) = workspace_with(None);
    let host = workspace.resolve_host(&r1()).unwrap();
    let link = &host.settings.web_links[0].value;
    assert_eq!(
        link.fill(&host.variables()).unwrap(),
        "https://10.0.0.1:8443/login"
    );
    let missing = link.fill(&Variables::new()).unwrap_err();
    assert_eq!(names(missing), ["address", "custom.gui_port"]);
}

// --- Multi-line expansion ---

#[test]
fn a_snippet_expands_to_its_lines() {
    let values: Variables = [("int", "Gi0/1")].into_iter().collect();
    let snippet = Snippet::new(
        "Interface",
        "conf t\ninterface {{int}}\r\n shutdown\r\rend\n",
    );
    assert_eq!(
        snippet.expand(&values).unwrap(),
        lines(&["conf t", "interface Gi0/1", " shutdown", "", "end"])
    );
    assert_eq!(
        Snippet::new("One", "show version").expand(&values).unwrap(),
        lines(&["show version"])
    );
    // Only the last line break ends the text; blank lines before it are sent.
    assert_eq!(
        Snippet::new("Blank", "a\n\n").expand(&values).unwrap(),
        lines(&["a", ""])
    );
}

#[test]
fn a_snippet_with_missing_variables_does_not_expand() {
    let snippet = Snippet::new("Vlan", "vlan {{vlan}}\n name {{vlan_name}}\n");
    let missing = snippet.expand(&Variables::new()).unwrap_err();
    assert_eq!(names(missing), ["vlan", "vlan_name"]);
    assert_eq!(snippet.variables(), ["vlan", "vlan_name"]);
}

#[test]
fn a_value_with_line_breaks_becomes_lines_of_its_own() {
    let values: Variables = [("acl", "permit 1\npermit 2")].into_iter().collect();
    assert_eq!(
        Snippet::new("Acl", "{{acl}}\nend").expand(&values).unwrap(),
        lines(&["permit 1", "permit 2", "end"])
    );
}

// --- Expanding for a session ---

const SNIPPETS: &str = "\
Show version: show version
Ping:
  group: Diagnostics
  text: ping {{address}} source {{source}}
Vlan:
  group: Config
  text: |
    vlan {{vlan}}
     name {{site}}-{{vlan}}
";

#[test]
fn a_hosts_snippet_is_filled_from_its_fields_and_the_answers() {
    let (_temp, workspace) = workspace_with(Some(SNIPPETS));
    let expanded = workspace.expand_snippet("vlan", Some(&r1()), &Variables::new());
    let Err(ExpandError::Missing(missing)) = expanded else {
        panic!("expected missing variables, got {expanded:?}");
    };
    assert_eq!(names(missing), ["vlan"]);

    let answers: Variables = [("vlan", "42"), ("site", "ignored")].into_iter().collect();
    assert_eq!(
        workspace
            .expand_snippet("Vlan", Some(&r1()), &answers)
            .unwrap(),
        lines(&["vlan 42", " name MAD1-42"])
    );
}

#[test]
fn without_a_host_every_variable_is_asked_for() {
    let (_temp, workspace) = workspace_with(Some(SNIPPETS));
    let expanded = workspace.expand_snippet("Ping", None, &Variables::new());
    let Err(ExpandError::Missing(missing)) = expanded else {
        panic!("expected missing variables, got {expanded:?}");
    };
    assert_eq!(names(missing), ["address", "source"]);
    let answers: Variables = [("address", "192.0.2.1"), ("source", "lo0")]
        .into_iter()
        .collect();
    assert_eq!(
        workspace.expand_snippet("Ping", None, &answers).unwrap(),
        lines(&["ping 192.0.2.1 source lo0"])
    );
    // An answer may be empty: it is still an answer.
    let answers: Variables = [("address", "x"), ("source", "")].into_iter().collect();
    assert_eq!(
        workspace.expand_snippet("Ping", None, &answers).unwrap(),
        lines(&["ping x source "])
    );
}

#[test]
fn expanding_an_unknown_snippet_or_host_fails() {
    let (_temp, workspace) = workspace_with(Some(SNIPPETS));
    assert!(matches!(
        workspace.expand_snippet("nope", None, &Variables::new()),
        Err(ExpandError::NoSuchSnippet(name)) if name == "nope"
    ));
    let gone = HostId::new("Madrid/gone.yaml");
    assert!(matches!(
        workspace.expand_snippet("Ping", Some(&gone), &Variables::new()),
        Err(ExpandError::NoSuchHost(id)) if id == gone
    ));
}

// --- Expanding for each member of the multi-send group (#36) ---

/// The `workspace_with` workspace plus a second host, `Lisbon/r2.yaml`,
/// with a site of its own and no `circuit`.
fn two_hosts() -> (TempDir, Workspace) {
    let (temp, _) = workspace_with(Some(SNIPPETS));
    fs::create_dir(temp.path().join("Lisbon")).unwrap();
    fs::write(
        temp.path().join("Lisbon/r2.yaml"),
        "address: 10.0.0.2\nsite: LIS1\n",
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    (temp, workspace)
}

fn r2() -> HostId {
    HostId::new("Lisbon/r2.yaml")
}

#[test]
fn a_snippet_for_the_group_is_filled_with_each_members_own_values() {
    let (_temp, workspace) = two_hosts();
    let answers: Variables = [("vlan", "42")].into_iter().collect();
    let expanded = workspace
        .expand_snippet_for_each("Vlan", &[Some(r1()), Some(r2())], &answers)
        .unwrap();
    assert_eq!(
        expanded,
        [
            lines(&["vlan 42", " name MAD1-42"]),
            lines(&["vlan 42", " name LIS1-42"]),
        ]
    );
}

#[test]
fn variables_any_member_lacks_are_asked_once_for_the_whole_group() {
    let (temp, _) = two_hosts();
    let snippets = "Circuit: |\n  description {{circuit}} {{site}} {{vlan}}\n";
    fs::write(temp.path().join(SNIPPETS_FILE), snippets).unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();
    // r1 has a circuit, r2 does not; neither has a vlan. Each name is
    // asked once, in the snippet's order.
    let expanded =
        workspace.expand_snippet_for_each("Circuit", &[Some(r1()), Some(r2())], &Variables::new());
    let Err(ExpandError::Missing(missing)) = expanded else {
        panic!("expected missing variables, got {expanded:?}");
    };
    assert_eq!(names(missing), ["circuit", "vlan"]);

    // The answers apply to every member; host values still come first.
    let answers: Variables = [("circuit", "NONE"), ("vlan", "7")].into_iter().collect();
    assert_eq!(
        workspace
            .expand_snippet_for_each("Circuit", &[Some(r1()), Some(r2())], &answers)
            .unwrap(),
        [
            lines(&["description CID-42 MAD1 7"]),
            lines(&["description NONE LIS1 7"]),
        ]
    );
}

#[test]
fn members_without_a_live_host_get_only_the_answers() {
    let (_temp, workspace) = two_hosts();
    // A quick connect in the group: the host's variables are asked for.
    let expanded =
        workspace.expand_snippet_for_each("Ping", &[Some(r1()), None], &Variables::new());
    let Err(ExpandError::Missing(missing)) = expanded else {
        panic!("expected missing variables, got {expanded:?}");
    };
    assert_eq!(names(missing), ["address", "source"]);

    let answers: Variables = [("address", "192.0.2.9"), ("source", "lo0")]
        .into_iter()
        .collect();
    assert_eq!(
        workspace
            .expand_snippet_for_each("Ping", &[Some(r1()), None], &answers)
            .unwrap(),
        [
            lines(&["ping 10.0.0.1 source lo0"]),
            lines(&["ping 192.0.2.9 source lo0"]),
        ]
    );
}

#[test]
fn expanding_for_the_group_fails_for_an_unknown_snippet_or_host() {
    let (_temp, workspace) = two_hosts();
    assert!(matches!(
        workspace.expand_snippet_for_each("nope", &[None], &Variables::new()),
        Err(ExpandError::NoSuchSnippet(name)) if name == "nope"
    ));
    let gone = HostId::new("Madrid/gone.yaml");
    assert!(matches!(
        workspace.expand_snippet_for_each("Show version", &[Some(r1()), Some(gone.clone())], &Variables::new()),
        Err(ExpandError::NoSuchHost(id)) if id == gone
    ));
    // No members: nothing to send.
    assert_eq!(
        workspace
            .expand_snippet_for_each("Show version", &[], &Variables::new())
            .unwrap(),
        Vec::<Vec<String>>::new()
    );
}

// --- The snippets file ---

#[test]
fn snippets_load_in_file_order_with_their_groups() {
    let (_temp, workspace) = workspace_with(Some(SNIPPETS));
    assert_eq!(
        workspace.snippets(),
        [
            Snippet::new("Show version", "show version"),
            Snippet::new("Ping", "ping {{address}} source {{source}}").in_group("Diagnostics"),
            Snippet::new("Vlan", "vlan {{vlan}}\n name {{site}}-{{vlan}}\n").in_group("Config"),
        ]
    );
    assert!(workspace.problems().is_empty());
    assert_eq!(
        workspace.snippet("PING").map(|s| s.name.as_str()),
        Some("Ping")
    );
}

#[test]
fn bad_snippets_are_problems_and_left_out() {
    let (_temp, workspace) = workspace_with(Some(
        "Good: show clock\nNo text:\n  group: x\nEmpty: ''\nList: [a, b]\nOdd:\n  text: x\n  colour: red\ngood: twice\nGrouped:\n  group: [x]\n  text: y\n",
    ));
    let loaded: Vec<&str> = workspace
        .snippets()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(loaded, ["Good"]);
    let problems: Vec<(&str, &str)> = workspace
        .problems()
        .iter()
        .map(|p| (p.file.as_str(), p.message.as_str()))
        .collect();
    assert_eq!(
        problems,
        [
            (SNIPPETS_FILE, "snippet `No text`: missing `text`"),
            (SNIPPETS_FILE, "snippet `Empty`: it has no text"),
            (
                SNIPPETS_FILE,
                "snippet `List`: expected its text, or `text` and `group`"
            ),
            (SNIPPETS_FILE, "snippet `Odd`: unknown key `colour`"),
            (SNIPPETS_FILE, "snippet `good` is defined twice"),
            (SNIPPETS_FILE, "snippet `Grouped`: `group` must be text"),
        ]
    );
}

#[test]
fn a_new_snippet_is_saved_after_the_others() {
    let (temp, mut workspace) = workspace_with(Some("Show version: show version\n"));
    workspace
        .save_snippet(
            None,
            &Snippet::new("  Interfaces ", "show ip int brief\nshow int status\n")
                .in_group(" Diagnostics "),
        )
        .unwrap();
    assert_eq!(
        read(temp.path(), SNIPPETS_FILE),
        "Show version: show version\nInterfaces:\n  group: Diagnostics\n  text: |\n    show ip int brief\n    show int status\n"
    );
    assert_eq!(
        workspace.snippets()[1],
        Snippet::new("Interfaces", "show ip int brief\nshow int status\n").in_group("Diagnostics")
    );
}

#[test]
fn the_first_snippet_creates_the_file() {
    let (temp, mut workspace) = workspace_with(None);
    workspace
        .save_snippet(None, &Snippet::new("Clock", "show clock").in_group("  "))
        .unwrap();
    assert_eq!(
        read(temp.path(), SNIPPETS_FILE),
        "Clock:\n  text: show clock\n"
    );
    assert_eq!(workspace.snippets(), [Snippet::new("Clock", "show clock")]);
}

#[test]
fn editing_a_snippet_keeps_its_place_and_can_rename_it() {
    let (temp, mut workspace) = workspace_with(Some(SNIPPETS));
    workspace
        .save_snippet(
            Some("ping"),
            &Snippet::new("Ping from loopback", "ping {{address}} source lo0")
                .in_group("Diagnostics"),
        )
        .unwrap();
    let names: Vec<&str> = workspace
        .snippets()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["Show version", "Ping from loopback", "Vlan"]);
    let file = read(temp.path(), SNIPPETS_FILE);
    assert!(
        file.starts_with("Show version: show version\nPing from loopback:\n  group: Diagnostics\n  text: ping {{address}} source lo0\nVlan:\n"),
        "{file}"
    );
    // Moved to another group, same name in another case.
    workspace
        .save_snippet(Some("Vlan"), &Snippet::new("VLAN", "vlan {{vlan}}"))
        .unwrap();
    assert_eq!(
        workspace.snippets()[2],
        Snippet::new("VLAN", "vlan {{vlan}}")
    );
}

#[test]
fn snippets_that_cannot_be_saved_are_refused_without_writing() {
    let (temp, mut workspace) = workspace_with(Some(SNIPPETS));
    let refused = [
        (None, Snippet::new("ping", "x")),
        (Some("Vlan"), Snippet::new("Ping", "x")),
        (None, Snippet::new(" ", "x")),
        (None, Snippet::new("Blank", " \n ")),
    ];
    for (previous, snippet) in refused {
        let error = workspace.save_snippet(previous, &snippet).unwrap_err();
        assert!(
            matches!(error, EditError::AlreadyExists(_) | EditError::Invalid(_)),
            "{snippet:?}: {error:?}"
        );
    }
    assert!(matches!(
        workspace.save_snippet(Some("nope"), &Snippet::new("Nope", "x")),
        Err(EditError::NotFound(_))
    ));
    assert_eq!(read(temp.path(), SNIPPETS_FILE), SNIPPETS);
}

#[test]
fn a_badly_defined_entry_is_never_written_over() {
    let original = "Ping: ping x\nOdd: [a, b]\n";
    let (temp, mut workspace) = workspace_with(Some(original));
    for previous in [None, Some("Ping")] {
        let error = workspace
            .save_snippet(previous, &Snippet::new("odd", "y"))
            .unwrap_err();
        assert!(matches!(error, EditError::AlreadyExists(_)), "{error:?}");
    }
    assert_eq!(read(temp.path(), SNIPPETS_FILE), original);
}

#[test]
fn deleting_the_last_snippet_removes_the_file() {
    let (temp, mut workspace) = workspace_with(Some("A: a\nB: b\n"));
    workspace.delete_snippet("a").unwrap();
    assert_eq!(read(temp.path(), SNIPPETS_FILE), "B: b\n");
    assert!(matches!(
        workspace.delete_snippet("a"),
        Err(EditError::NotFound(_))
    ));
    workspace.delete_snippet("B").unwrap();
    assert!(!temp.path().join(SNIPPETS_FILE).exists());
    assert!(workspace.snippets().is_empty());
}

#[test]
fn snippets_changed_outside_the_app_are_picked_up() {
    let (temp, mut workspace) = workspace_with(Some("A: a\n"));
    fs::write(temp.path().join(SNIPPETS_FILE), "A: a\nB: b\n").unwrap();
    assert!(workspace.refresh().unwrap());
    assert_eq!(workspace.snippets().len(), 2);
    assert!(!workspace.refresh().unwrap());
}

#[test]
fn finding_snippets_matches_every_word_in_name_group_or_text() {
    let (_temp, workspace) = workspace_with(Some(SNIPPETS));
    let found = |query: &str| -> Vec<String> {
        workspace
            .find_snippets(query)
            .into_iter()
            .map(|s| s.name.clone())
            .collect()
    };
    assert_eq!(found(""), ["Show version", "Ping", "Vlan"]);
    assert_eq!(found("diag"), ["Ping"]);
    assert_eq!(found("PING source"), ["Ping"]);
    // Name matches come before text matches.
    assert_eq!(found("v"), ["Show version", "Vlan"]);
    assert_eq!(found("ver"), ["Show version"]);
    assert_eq!(found("name"), ["Vlan"]);
    assert!(found("ping vlan").is_empty());
}
