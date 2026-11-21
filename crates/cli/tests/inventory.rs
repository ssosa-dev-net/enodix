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

/// A folder, a host id or a workspace folder whose name starts with `-`:
/// a value after `--` is never an option, and `--option=<value>` takes any
/// value.
#[test]
fn values_that_start_with_a_dash_come_after_a_double_dash_or_an_equals_sign() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("-lab");
    std::fs::create_dir_all(root.join("-old")).unwrap();
    std::fs::write(root.join("-old/-spare.yaml"), "address: 192.0.2.7\n").unwrap();
    std::fs::write(root.join("r1.yaml"), "address: 192.0.2.1\n").unwrap();
    let in_temp = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_netterm-cli"))
            .args(args)
            .current_dir(temp.path())
            .output()
            .unwrap()
    };
    let shown = |args: &[&str]| {
        let output = in_temp(args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut found: Vec<String> = ids(&json).into_iter().map(str::to_owned).collect();
        found.sort();
        found
    };

    assert_eq!(
        shown(&["inventory", "--", "-lab"]),
        ["-old/-spare.yaml", "r1.yaml"]
    );
    assert_eq!(
        shown(&["inventory", "--folder=-old", "--", "-lab"]),
        ["-old/-spare.yaml"]
    );
    assert_eq!(
        shown(&["inventory", "--host=-old/-spare.yaml", "--", "-lab"]),
        ["-old/-spare.yaml"]
    );
    // `=` works for any value, with the workspace before or after.
    let lab = root.to_str().unwrap();
    assert_eq!(
        shown(&["inventory", lab, "--folder=-old"]),
        ["-old/-spare.yaml"]
    );
    assert_eq!(shown(&["inventory", "--host=r1.yaml", lab]), ["r1.yaml"]);

    for args in [
        // After `--` nothing is an option.
        &["inventory", "--", "-lab", "--folder", "-old"][..],
        // A value of its own still cannot start with `-`: it says how.
        &["inventory", lab, "--folder", "-old"][..],
        &["inventory", lab, "--folder="][..],
        &["inventory", "-lab"][..],
    ] {
        let output = in_temp(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
    let said = stderr(&in_temp(&["inventory", lab, "--folder", "-old"]));
    assert!(said.contains("--folder=<value>"), "{said}");
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

/// Runs `inventory` on the `export` fixture as a script job whose targets
/// file holds `hosts` for the workspace `workspace` (the fixture if `None`).
fn as_job(hosts: &[&str], workspace: Option<&Path>, limit: &[&str]) -> Output {
    let temp = tempfile::TempDir::new().unwrap();
    let root = fixture("export");
    let targets = temp.path().join("targets.json");
    std::fs::write(
        &targets,
        serde_json::json!({
            "format": 1,
            "workspace": workspace.unwrap_or(&root),
            "hosts": hosts,
        })
        .to_string(),
    )
    .unwrap();
    let mut args = vec!["inventory", root.to_str().unwrap()];
    args.extend_from_slice(limit);
    Command::new(env!("CARGO_BIN_EXE_netterm-cli"))
        .args(&args)
        .env("NETTERM_JOB_TARGETS", &targets)
        .output()
        .expect("the binary starts")
}

fn job_ids(output: &Output) -> Vec<String> {
    assert_eq!(output.status.code(), Some(0), "{}", stderr(output));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    ids(&json).into_iter().map(str::to_owned).collect()
}

#[test]
fn run_by_a_script_job_it_shows_only_the_jobs_hosts() {
    let job = ["Madrid/core-r1.yaml", "bastion.yaml"];
    assert_eq!(job_ids(&as_job(&job, None, &[])), job);
    // A narrower limit of the script's own still applies.
    assert_eq!(
        job_ids(&as_job(&job, None, &["--folder", "Madrid"])),
        ["Madrid/core-r1.yaml"]
    );
    let output = as_job(&job, None, &["--folder", "Madrid"]);
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Only the folders of the hosts shown.
    assert_eq!(json["folders"].as_array().unwrap().len(), 1);
    assert_eq!(
        job_ids(&as_job(&job, None, &["--host", "bastion.yaml"])),
        ["bastion.yaml"]
    );
}

#[test]
fn run_by_a_script_job_it_refuses_other_hosts_and_workspaces() {
    let job = ["bastion.yaml"];
    let outside = as_job(&job, None, &["--host", "old-sw1.yaml"]);
    assert_eq!(outside.status.code(), Some(1));
    assert!(
        stderr(&outside).contains("not among the hosts of the job"),
        "{}",
        stderr(&outside)
    );
    let other = fixture("inherit");
    let elsewhere = as_job(&job, Some(&other), &[]);
    assert_eq!(elsewhere.status.code(), Some(1));
    assert!(
        stderr(&elsewhere).contains("reads no other workspace"),
        "{}",
        stderr(&elsewhere)
    );

    // A targets file that cannot be read never shows everything.
    let root = fixture("export");
    let missing = Command::new(env!("CARGO_BIN_EXE_netterm-cli"))
        .args(["inventory", root.to_str().unwrap()])
        .env("NETTERM_JOB_TARGETS", root.join("no-such-targets.json"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert!(
        stderr(&missing).contains("targets file"),
        "{}",
        stderr(&missing)
    );
}
