//! Edge tests (#117): malformed and half-written inventory files, unusual
//! encodings, hostile YAML, and edits made in the app over files it could
//! not read or that changed since it read them.
//!
//! Every file kind (a host, a folder's settings, the platform definitions,
//! saved filters, saved jobs, snippets, highlight rules, roles) is cut off at
//! every line (and inside each line), given wrong types, unknown keys, a
//! byte order mark, CRLF line ends, tabs, and text that is not UTF-8. What
//! must hold: the rest of the workspace loads; a file that does not load is
//! named in a problem; an edit the app makes never writes over a file it
//! could not read, nor drops keys it does not understand. Windows and Linux
//! alike unless a test says otherwise.

mod support;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use netterm_core::highlight::HighlightRule;
use netterm_core::job::{JobStep, JobTarget};
use netterm_core::platform::PlatformDefinition;
use netterm_core::workspace::{
    Condition, EditError, HostId, SavedFilter, SavedJob, Snippet, Workspace,
};
use serde_norway::{Mapping, Value};
use tempfile::TempDir;

const HOST: &str = "\
# A core router.
name: core-r1
address: 10.0.0.1
vendor: arista
platform: eos
tags:
  - core
  - madrid
custom:
  circuit: CID-1
  owner: noc
serial:
  baud_rate: 9600
port: 22
";

const FOLDER: &str = "\
description: Madrid site
tags: [madrid, spain]
credential_profile: lab
connect_timeout: 30
web_links:
  Grafana: https://grafana.example/d/{{name}}
";

const PLATFORMS: &str = "\
eos:
  disable_paging: terminal pager 0
mikrotik:
  prompt: '\\[\\S+@\\S+\\] >'
  other_names: [routeros]
  show_config: /export
";

const FILTERS: &str = "\
Arista:
  vendor: arista
Madrid core:
  site: Madrid
  tags: [core]
";

const JOBS: &str = "\
Versions:
  snippet: Show version
  folder: Madrid
Interfaces:
  snippet: Interface
  answers:
    interface: Ethernet1
  folder: ''
";

const SNIPPETS: &str = "\
Show version: show version
Interface: show interface {{interface}}
";

const HIGHLIGHTS: &str = "\
Down:
  pattern: '\\bdown\\b'
  color: red
Up:
  pattern: '\\bup\\b'
  color: green
  enabled: false
";

const ROLES: &str = "\
spine:
  name: Spine
  icon: switch
  color: '#2563eb'
";

/// A file kind: where it goes and its good text.
struct Kind {
    file: &'static str,
    good: &'static str,
}

const KINDS: [Kind; 8] = [
    Kind {
        file: "Madrid/core-r1.yaml",
        good: HOST,
    },
    Kind {
        file: "Madrid/_folder.yaml",
        good: FOLDER,
    },
    Kind {
        file: "_platforms.yaml",
        good: PLATFORMS,
    },
    Kind {
        file: "_filters.yaml",
        good: FILTERS,
    },
    Kind {
        file: "_jobs.yaml",
        good: JOBS,
    },
    Kind {
        file: "_snippets.yaml",
        good: SNIPPETS,
    },
    Kind {
        file: "_highlights.yaml",
        good: HIGHLIGHTS,
    },
    Kind {
        file: "_roles.yaml",
        good: ROLES,
    },
];

/// A workspace with every kind of file in its good form, plus hosts that
/// must always load: `sentinel` at the top, `Madrid/edge-r2`.
fn workspace() -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("Madrid")).unwrap();
    fs::write(root.join("_workspace.yaml"), "id: edge-inventory\n").unwrap();
    fs::write(root.join("sentinel.yaml"), "address: 192.0.2.1\n").unwrap();
    fs::write(
        root.join("Madrid/edge-r2.yaml"),
        "address: 192.0.2.2\nrole: spine\n",
    )
    .unwrap();
    for kind in &KINDS {
        fs::write(root.join(kind.file), kind.good).unwrap();
    }
    temp
}

fn open(root: &Path) -> Workspace {
    let workspace = Workspace::open(root).unwrap();
    assert!(
        workspace.host(&HostId::new("sentinel.yaml")).is_some(),
        "the rest did not load: {:?}",
        workspace.problems()
    );
    assert!(
        workspace
            .host(&HostId::new("Madrid/edge-r2.yaml"))
            .is_some(),
        "the rest did not load: {:?}",
        workspace.problems()
    );
    workspace
}

fn problems_on<'a>(workspace: &'a Workspace, file: &str) -> Vec<&'a str> {
    workspace
        .problems()
        .iter()
        .filter(|problem| problem.file == file)
        .map(|problem| problem.message.as_str())
        .collect()
}

/// Whether `text` reads as YAML whose top level is a mapping (or nothing).
fn as_mapping(text: &str) -> Option<Mapping> {
    if text.trim().is_empty() {
        return Some(Mapping::new());
    }
    match serde_norway::from_str::<Value>(text).ok()? {
        Value::Mapping(mapping) => Some(mapping),
        Value::Null => Some(Mapping::new()),
        _ => None,
    }
}

/// The edit the app makes that writes the file of `kind`.
fn edit(workspace: &mut Workspace, kind: &Kind) -> Option<Result<(), EditError>> {
    Some(match kind.file {
        "Madrid/core-r1.yaml" => workspace
            .create_host("Madrid", "core-r1", "10.9.9.9")
            .map(drop),
        "Madrid/_folder.yaml" => {
            workspace.set_folder_description("Madrid", Some("Changed in the app"))
        }
        "_platforms.yaml" => workspace.save_platform(
            None,
            &PlatformDefinition {
                name: "edge-os".into(),
                other_names: Vec::new(),
                prompt: "#".into(),
                disable_paging: None,
                privilege_command: None,
                privilege_prompt: None,
                show_config: None,
                ignored_lines: None,
                file_folder: None,
            },
        ),
        "_filters.yaml" => workspace.save_filter(&SavedFilter {
            name: "Juniper".into(),
            conditions: vec![Condition::new("vendor", "juniper")],
        }),
        "_jobs.yaml" => workspace.save_job(
            None,
            &SavedJob {
                name: "Everything".into(),
                step: JobStep::Snippet {
                    name: "Show version".into(),
                    answers: Default::default(),
                },
                target: JobTarget::Folder {
                    path: String::new(),
                },
            },
        ),
        "_snippets.yaml" => workspace.save_snippet(
            None,
            &Snippet {
                name: "Clock".into(),
                group: None,
                text: "show clock".into(),
            },
        ),
        "_highlights.yaml" => workspace.save_highlight_rule(
            None,
            &HighlightRule {
                name: "Error".into(),
                pattern: "error".into(),
                color: "red".into(),
                enabled: true,
            },
        ),
        // Nothing in the app writes the roles file.
        _ => return None,
    })
}

/// Opens the workspace with `text` as the file of `kind` and checks what
/// must hold; `named`: the file must be named in a problem.
fn check_variant(temp: &TempDir, kind: &Kind, text: &[u8], named: bool, what: &str) {
    let root = temp.path();
    let path = root.join(kind.file);
    fs::write(&path, text).unwrap();
    let mut workspace = open(root);
    let problems = problems_on(&workspace, kind.file);
    for problem in workspace.problems() {
        assert_eq!(
            problem.file, kind.file,
            "{what}: a problem on another file: {problem}"
        );
    }
    if named {
        assert!(!problems.is_empty(), "{what}: not named as a problem");
    }
    if problems.is_empty() {
        return;
    }
    // The app's edit of that file: refused, or made keeping all it had.
    let before = fs::read(&path).unwrap();
    match edit(&mut workspace, kind) {
        None => {}
        Some(Err(_)) => assert_eq!(fs::read(&path).unwrap(), before, "{what}: written over"),
        Some(Ok(())) => {
            let old = std::str::from_utf8(&before).ok().and_then(as_mapping);
            let old = old.unwrap_or_else(|| panic!("{what}: an unreadable file was written over"));
            let new = as_mapping(&fs::read_to_string(&path).unwrap()).unwrap();
            for key in old.keys() {
                assert!(new.contains_key(key), "{what}: the edit dropped {key:?}");
            }
        }
    }
    // Put it back for the next variant.
    fs::write(&path, kind.good).unwrap();
}

#[test]
fn the_good_files_load_without_problems() {
    let temp = workspace();
    let workspace = open(temp.path());
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    assert_eq!(workspace.filters().len(), 2);
    assert_eq!(workspace.saved_jobs().len(), 2);
    assert_eq!(workspace.snippets().len(), 2);
    assert!(workspace.platform("routeros").is_some());
    assert!(workspace.role("spine").is_some());
    let host = workspace.host(&HostId::new("Madrid/core-r1.yaml")).unwrap();
    assert_eq!(host.tags, ["core", "madrid"]);
}

#[test]
fn every_file_kind_cut_off_anywhere_leaves_the_rest_names_itself_and_is_not_written_over() {
    let temp = workspace();
    for kind in &KINDS {
        let good = kind.good.as_bytes();
        let mut cuts: Vec<usize> = vec![0];
        let mut start = 0;
        for (at, byte) in good.iter().enumerate() {
            if *byte == b'\n' {
                cuts.push(start + (at - start) / 2);
                cuts.push(at);
                cuts.push(at + 1);
                start = at + 1;
            }
        }
        cuts.dedup();
        for cut in cuts {
            let what = format!("{} cut at byte {cut}", kind.file);
            // A cut file may read as a shorter good one: only a file that
            // does not read must be named.
            check_variant(&temp, kind, &good[..cut], false, &what);
        }
    }
}

#[test]
fn wrong_types_are_named_and_the_rest_loads() {
    let temp = workspace();
    let variants: [(&str, &[&str]); 8] = [
        (
            "Madrid/core-r1.yaml",
            &[
                "address: [10.0.0.1]\n",
                "address: 10.0.0.1\ntags: 5\n",
                "address: 10.0.0.1\nport: twenty-two\n",
                "address: 10.0.0.1\ncustom: [a, b]\n",
                "address: 10.0.0.1\nprotocol: smoke-signals\n",
                "address: 10.0.0.1\nserial: COM3\n",
                "- address: 10.0.0.1\n",
                "just text\n",
                "address: 10.0.0.1\naddress: 10.0.0.2\n",
                "name: ''\naddress: 10.0.0.1\n",
            ],
        ),
        (
            "Madrid/_folder.yaml",
            &[
                "tags: 7\n",
                "connect_timeout: forever\n",
                "web_links: [a]\n",
                "- a\n- b\n",
                "description: [a]\n",
            ],
        ),
        (
            "_platforms.yaml",
            &["eos: 5\n", "- eos\n", "x:\n  prompt: [a]\n"],
        ),
        (
            "_filters.yaml",
            &["Arista: 5\n", "- a\n", "X:\n  colour: red\n"],
        ),
        (
            "_jobs.yaml",
            &[
                "Versions: 5\n",
                "- a\n",
                "J:\n  snippet: [a]\n  folder: x\n",
            ],
        ),
        ("_snippets.yaml", &["S: [a]\n", "- a\n"]),
        (
            "_highlights.yaml",
            &[
                "Down: 5\n",
                "- a\n",
                "X:\n  pattern: '(open'\n  color: red\n",
            ],
        ),
        (
            "_roles.yaml",
            &["spine: 5\n", "- a\n", "x:\n  color: blue-ish\n"],
        ),
    ];
    for (file, texts) in variants {
        let kind = KINDS.iter().find(|kind| kind.file == file).unwrap();
        for text in texts {
            check_variant(
                &temp,
                kind,
                text.as_bytes(),
                true,
                &format!("{file}: {text:?}"),
            );
        }
    }
}

#[test]
fn a_byte_order_mark_crlf_line_ends_and_tabs_in_values_read_as_the_same_file() {
    let temp = workspace();
    let root = temp.path();
    for kind in &KINDS {
        let crlf = kind.good.replace('\n', "\r\n");
        let bom = format!("\u{feff}{}", kind.good);
        let bom_crlf = format!("\u{feff}{crlf}");
        for (form, text) in [
            ("CRLF", &crlf),
            ("a BOM", &bom),
            ("a BOM and CRLF", &bom_crlf),
        ] {
            fs::write(root.join(kind.file), text).unwrap();
            let workspace = open(root);
            assert!(
                workspace.problems().is_empty(),
                "{} with {form}: {:?}",
                kind.file,
                workspace.problems()
            );
        }
        fs::write(root.join(kind.file), kind.good).unwrap();
    }
    let workspace = open(root);
    let host = workspace.host(&HostId::new("Madrid/core-r1.yaml")).unwrap();
    assert_eq!(host.address, "10.0.0.1");

    // A tab inside a value is text; a tab as indentation is not YAML.
    fs::write(
        root.join("Madrid/core-r1.yaml"),
        "address: 10.0.0.1\ndescription: \"rack\tA\"\n",
    )
    .unwrap();
    let workspace = open(root);
    let host = workspace.host(&HostId::new("Madrid/core-r1.yaml")).unwrap();
    assert_eq!(host.description.as_deref(), Some("rack\tA"));
    let kind = &KINDS[0];
    check_variant(
        &temp,
        kind,
        b"address: 10.0.0.1\ntags:\n\t- core\n",
        true,
        "a tab as indentation",
    );
}

#[test]
fn a_byte_order_mark_is_dropped_when_the_app_writes_the_file() {
    let temp = workspace();
    let root = temp.path();
    fs::write(
        root.join("Madrid/_folder.yaml"),
        format!("\u{feff}{FOLDER}"),
    )
    .unwrap();
    let mut workspace = open(root);
    workspace
        .set_folder_description("Madrid", Some("Changed"))
        .unwrap();
    let text = fs::read_to_string(root.join("Madrid/_folder.yaml")).unwrap();
    assert!(!text.starts_with('\u{feff}'), "{text:?}");
    assert!(text.contains("credential_profile: lab"), "{text}");
}

#[test]
fn a_file_that_is_not_utf8_is_named_and_never_written_over() {
    let temp = workspace();
    // Windows-1252 (an editor's "ANSI"), and UTF-16 with its BOM (what
    // Windows PowerShell 5's `>` writes).
    let ansi = b"address: 10.0.0.1\ndescription: Caf\xe9 rack\n".to_vec();
    let mut utf16 = vec![0xff, 0xfe];
    for unit in "address: 10.0.0.1\n".encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    for kind in &KINDS {
        for (form, text) in [("ANSI", &ansi), ("UTF-16", &utf16)] {
            check_variant(&temp, kind, text, true, &format!("{} in {form}", kind.file));
        }
    }
}

#[test]
fn unknown_keys_and_entries_the_app_cannot_use_survive_its_edits() {
    let temp = workspace();
    let root = temp.path();
    let extra = [
        (
            "Madrid/_folder.yaml",
            format!("{FOLDER}owner_team: noc\n"),
            "owner_team",
        ),
        (
            "_platforms.yaml",
            format!("{PLATFORMS}odd:\n  prompt: '#'\n  colour: red\n"),
            "odd",
        ),
        (
            "_filters.yaml",
            format!("{FILTERS}Colour:\n  colour: red\n"),
            "Colour",
        ),
        (
            "_jobs.yaml",
            format!("{JOBS}Broken:\n  snippet: Show version\n"),
            "Broken",
        ),
        ("_snippets.yaml", format!("{SNIPPETS}Odd: [a]\n"), "Odd"),
        ("_highlights.yaml", format!("{HIGHLIGHTS}Odd: 5\n"), "Odd"),
    ];
    for (file, text, key) in extra {
        fs::write(root.join(file), &text).unwrap();
        let mut workspace = open(root);
        let kind = KINDS.iter().find(|kind| kind.file == file).unwrap();
        edit(&mut workspace, kind)
            .unwrap()
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        let written = as_mapping(&fs::read_to_string(root.join(file)).unwrap()).unwrap();
        assert!(written.contains_key(key), "{file} lost {key}");
        let old = as_mapping(&text).unwrap();
        for (old_key, value) in &old {
            if written.get(old_key) != Some(value) && old_key.as_str() != Some("description") {
                panic!("{file}: {old_key:?} changed to {:?}", written.get(old_key));
            }
        }
        fs::write(root.join(file), kind.good).unwrap();
    }
}

#[test]
fn hostile_yaml_is_a_problem_at_once_not_a_hang_or_a_crash() {
    let temp = workspace();
    // Aliases that expand to a billion entries.
    let mut laughs = String::from(
        "a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n",
    );
    for (name, previous) in ["b", "c", "d", "e", "f", "g", "h", "i"]
        .iter()
        .zip(["a", "b", "c", "d", "e", "f", "g", "h"])
    {
        let list = vec![format!("*{previous}"); 9].join(",");
        laughs.push_str(&format!("{name}: &{name} [{list}]\n"));
    }
    laughs.push_str("address: 10.0.0.1\ncustom:\n  x: *i\n");
    // Nesting deeper than any parser's stack.
    let depth: usize = std::env::var("EDGE_NESTING")
        .ok()
        .and_then(|depth| depth.parse().ok())
        .unwrap_or(10_000);
    let deep = format!(
        "address: 10.0.0.1\ncustom: {}{}\n",
        "[".repeat(depth),
        "]".repeat(depth)
    );
    for (what, text) in [("aliases", laughs), ("nesting", deep)] {
        for kind in &KINDS {
            let started = Instant::now();
            check_variant(
                &temp,
                kind,
                text.as_bytes(),
                false,
                &format!("{} {what}", kind.file),
            );
            println!("{} {what}: {:?}", kind.file, started.elapsed());
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{} {what} took {:?}",
                kind.file,
                started.elapsed()
            );
        }
    }
}

// Edits over files that changed since the app read them.

#[test]
fn saving_a_host_whose_file_was_broken_since_it_was_loaded_never_writes_over_it() {
    let temp = workspace();
    let root = temp.path();
    let mut workspace = open(root);
    let id = HostId::new("Madrid/core-r1.yaml");
    let mut host = workspace.host(&id).unwrap().clone();
    host.description = Some("set in the app".into());

    // A half-written save by an editor (or a pull mid-way), not yet seen.
    let broken = "address: 10.0.0.1\ntags: [core,\n";
    fs::write(root.join(id.as_str()), broken).unwrap();
    let error = workspace.write_host(&host).unwrap_err();
    assert_eq!(fs::read_to_string(root.join(id.as_str())).unwrap(), broken);
    assert!(error.to_string().contains("core-r1"), "{error}");

    // The same for a rename that writes the name in the file.
    let error = workspace.rename_host(&id, "core-r9").unwrap_err();
    assert!(error.to_string().contains("core-r1"), "{error}");
    assert_eq!(fs::read_to_string(root.join(id.as_str())).unwrap(), broken);
    assert!(!root.join("Madrid/core-r9.yaml").exists());
}

#[test]
fn saving_a_host_changed_outside_since_it_was_loaded_does_not_write_over_that_change() {
    let temp = workspace();
    let root = temp.path();
    let mut workspace = open(root);
    let id = HostId::new("Madrid/core-r1.yaml");
    let mut host = workspace.host(&id).unwrap().clone();
    host.description = Some("set in the app".into());

    // Someone else's change (a pull, an editor) that the watcher has not
    // reported yet.
    let theirs = HOST.replace("10.0.0.1", "10.0.0.99");
    fs::write(root.join(id.as_str()), &theirs).unwrap();
    let error = workspace.write_host(&host).unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
    assert_eq!(fs::read_to_string(root.join(id.as_str())).unwrap(), theirs);

    // Once reloaded the edit goes through, over the new contents.
    assert!(workspace.refresh().unwrap());
    let mut host = workspace.host(&id).unwrap().clone();
    host.description = Some("set in the app".into());
    workspace.write_host(&host).unwrap();
    let text = fs::read_to_string(root.join(id.as_str())).unwrap();
    assert!(
        text.contains("10.0.0.99") && text.contains("set in the app"),
        "{text}"
    );
}

#[test]
fn saving_a_host_deleted_outside_since_it_was_loaded_does_not_bring_it_back() {
    let temp = workspace();
    let root = temp.path();
    let mut workspace = open(root);
    let id = HostId::new("Madrid/core-r1.yaml");
    let mut host = workspace.host(&id).unwrap().clone();
    host.description = Some("set in the app".into());
    fs::remove_file(root.join(id.as_str())).unwrap();

    let error = workspace.write_host(&host).unwrap_err();
    assert!(error.to_string().contains("core-r1"), "{error}");
    assert!(
        !root.join(id.as_str()).exists(),
        "the deleted host came back"
    );
}
