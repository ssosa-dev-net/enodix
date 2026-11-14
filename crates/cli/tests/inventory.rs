//! The headless binary's command-line contract for `inventory`: the built
//! binary is started as a process against the core's fixture workspaces,
//! and its output and exit status are checked.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../core/tests/fixtures/workspaces")
        .join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_netterm-cli"))
        .args(args)
        .output()
        .expect("the binary starts")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Runs `inventory` on the `export` fixture with `limit`; it must succeed.
fn inventory(limit: &[&str]) -> (Value, String) {
    let root = fixture("export");
    let mut args = vec!["inventory", root.to_str().unwrap()];
    args.extend_from_slice(limit);
    let output = run(&args);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let json = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    (json, stderr(&output))
}

fn ids(json: &Value) -> Vec<&str> {
    json["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|host| host["id"].as_str().unwrap())
        .collect()
}

fn host<'a>(json: &'a Value, id: &str) -> &'a Value {
    json["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|host| host["id"] == id)
        .unwrap_or_else(|| panic!("no host {id}"))
}

/// Fails with status 1 and says `expected` on stderr, printing nothing.
fn refused(args: &[&str], expected: &str) {
    let output = run(args);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    let message = stderr(&output);
    assert!(message.contains(expected), "{message}");
}

#[test]
fn it_prints_every_resolved_host_as_json() {
    let (json, _) = inventory(&[]);
    assert_eq!(json["format"], 1);
    assert_eq!(json["workspace"]["id"], "export-fixture");
    assert_eq!(json["workspace"]["name"], "export");
    let root = json["workspace"]["root"].as_str().unwrap();
    assert!(Path::new(root).is_absolute(), "{root}");
    assert_eq!(
        ids(&json),
        [
            "Madrid/Lab/lab-sw1.yaml",
            "Madrid/core-r1.yaml",
            "Madrid/old-ts1.yaml",
            "bastion.yaml",
            "old-sw1.yaml"
        ]
    );

    let core = host(&json, "Madrid/core-r1.yaml");
    assert_eq!(core["address"], "10.1.0.1");
    assert_eq!(core["settings"]["port"], 2222);
    assert_eq!(core["settings"]["protocol"], "ssh");
    assert_eq!(core["settings"]["platform"], "eos");
    assert_eq!(core["settings"]["credential_profile"], "madrid-ops");
    assert_eq!(core["jump_hosts"][0]["id"], "bastion.yaml");
    assert_eq!(core["jump_hosts"][0]["address"], "192.0.2.10");
    assert_eq!(core["jump_hosts"][0]["port"], 22);
    assert_eq!(core["folders"], serde_json::json!(["Madrid"]));
    assert_eq!(
        core["tags"],
        serde_json::json!(["acme", "madrid", "core", "edge"])
    );
    assert_eq!(
        core["custom"],
        serde_json::json!({"owner": "noc", "circuit": "CID-1"})
    );
    assert_eq!(host(&json, "old-sw1.yaml")["settings"]["port"], 23);
}

#[test]
fn workspace_problems_are_reported_without_stopping_the_export() {
    let (json, warnings) = inventory(&[]);
    assert_eq!(json["problems"][0]["file"], "broken.yaml");
    assert!(warnings.contains("warning: broken.yaml"), "{warnings}");
    let lab = host(&json, "Madrid/Lab/lab-sw1.yaml");
    assert!(lab["jump_chain_error"]
        .as_str()
        .unwrap()
        .contains("gone.yaml"));
}

#[test]
fn it_holds_no_secrets() {
    let root = fixture("export");
    let output = run(&["inventory", root.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
    // The fixture has a secret-named custom field and a web link with a
    // login in it.
    for word in ["password", "secret", "passphrase", "hunter2", "s3cr3t"] {
        assert!(!text.contains(word), "{word}");
    }
}

#[test]
fn it_limits_to_a_folder_a_saved_filter_or_host_ids() {
    let (madrid, _) = inventory(&["--folder", "Madrid"]);
    assert_eq!(
        ids(&madrid),
        [
            "Madrid/Lab/lab-sw1.yaml",
            "Madrid/core-r1.yaml",
            "Madrid/old-ts1.yaml"
        ]
    );
    let (lab, _) = inventory(&["--folder", "madrid/lab"]);
    assert_eq!(ids(&lab), ["Madrid/Lab/lab-sw1.yaml"]);

    let (edge, _) = inventory(&["--filter", "Edge"]);
    assert_eq!(ids(&edge), ["Madrid/core-r1.yaml"]);

    let (chosen, _) = inventory(&["--host", "old-sw1.yaml", "--host", "bastion.yaml"]);
    assert_eq!(ids(&chosen), ["old-sw1.yaml", "bastion.yaml"]);
}

#[test]
fn a_missing_or_unreadable_workspace_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("nowhere");
    refused(
        &["inventory", missing.to_str().unwrap()],
        "cannot open workspace",
    );
    let file = temp.path().join("file.txt");
    std::fs::write(&file, "not a folder").unwrap();
    refused(&["inventory", file.to_str().unwrap()], "not a folder");
}

#[test]
fn an_unknown_folder_filter_or_host_is_an_error() {
    let root = fixture("export");
    let root = root.to_str().unwrap();
    refused(
        &["inventory", root, "--folder", "Lisbon"],
        "no folder \"Lisbon\"",
    );
    refused(
        &["inventory", root, "--filter", "Firewalls"],
        "saved filters: Edge, Arista",
    );
    refused(
        &[
            "inventory",
            root,
            "--host",
            "bastion.yaml",
            "--host",
            "gone.yaml",
        ],
        "gone.yaml",
    );
}

#[test]
fn a_command_line_it_does_not_understand_gives_status_2() {
    let root = fixture("export");
    let root = root.to_str().unwrap();
    for args in [
        &[][..],
        &["export"][..],
        &["inventory"][..],
        &["inventory", root, "--folder"][..],
        &["inventory", root, "--host", "--folder", "Madrid"][..],
        &["inventory", root, "-x"][..],
        &["inventory", root, "--folder", "Madrid", "--filter", "Edge"][..],
        &["inventory", root, "--colour", "red"][..],
        &["inventory", root, "another"][..],
    ] {
        let output = run(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(stderr(&output).contains("--help"), "{args:?}");
    }
}

#[test]
fn help_and_version_print_and_succeed() {
    let help = run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("inventory <workspace folder>"));
    let version = run(&["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")));
}
