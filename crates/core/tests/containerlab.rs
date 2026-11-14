//! The containerlab device suite: real sessions to network operating systems
//! (Nokia SR Linux, Arista cEOS, FRR, and whatever else the lab file lists)
//! on a containerlab server.
//!
//! Opt-in. The `lab_*` tests are `#[ignore]`d, so `cargo test` and
//! `npm run check` never run them; `npm run test:lab` does. They read the lab
//! from the file `NETTERM_LAB_CONFIG` names, or `lab/lab.yaml` (ignored by
//! git); with neither, each prints [`lab::SKIP_MESSAGE`] and passes. A lab
//! file that is present but unusable (a missing password, a typo) fails.
//! See `docs/containerlab.md`.
//!
//! The other tests here run on every `cargo test`: they check the lab file
//! parsing and the skip rule, and run the same checks against the
//! in-process SSH and telnet servers on the loopback address, so the
//! suite's own logic is tested even where no lab exists.

mod lab;
mod support;

use std::path::Path;
use std::time::Duration;

use lab::{Check, Lab, LabRun, Outcome, ALL_CHECKS};
use netterm_core::credentials::ProfileAuth;
use support::telnet::TestTelnetServer;
use support::TestSshServer;

/// The lab, or `None` after printing why the suite is skipped.
fn configured_lab() -> Option<LabRun> {
    match lab::from_environment() {
        Ok(Some(lab)) => Some(LabRun::new(lab).expect("the suite's workspace")),
        Ok(None) => {
            eprintln!("{}", lab::SKIP_MESSAGE);
            None
        }
        Err(error) => panic!("{error}"),
    }
}

async fn lab_check(check: Check) {
    let Some(run) = configured_lab() else {
        return;
    };
    let outcomes = lab::run_checks(&run, &[check]).await;
    let checked = outcomes.checked;
    let failures = outcomes.failures;
    if checked == 0 {
        eprintln!(
            "[lab] {}: NOT CHECKED on any device (see the reasons above)",
            check.name()
        );
    }
    assert!(
        failures.is_empty(),
        "{} failed on {} device(s):\n{}",
        check.name(),
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_ssh_login_and_command() {
    lab_check(Check::LoginAndCommand).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_host_keys() {
    lab_check(Check::HostKeys).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_wrong_password() {
    lab_check(Check::WrongPassword).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_terminal_size() {
    lab_check(Check::Resize).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_large_output_with_flow_control() {
    lab_check(Check::LargeOutput).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_session_log() {
    lab_check(Check::SessionLog).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_telnet() {
    lab_check(Check::Telnet).await;
}

// The suite's own tests (no lab needed).

fn env_with(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

#[test]
fn without_a_lab_file_the_suite_is_skipped() {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/no-such-lab.yaml");
    assert_eq!(lab::locate(None, &missing), Ok(None));
    assert_eq!(lab::locate(Some("  "), &missing), Ok(None));
}

#[test]
fn a_lab_file_named_in_the_environment_must_exist() {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/no-such-lab.yaml");
    let error = lab::locate(Some(missing.to_str().unwrap()), &missing).unwrap_err();
    assert!(error.contains(lab::CONFIG_ENV), "{error}");
}

#[test]
fn the_default_lab_file_is_used_when_present() {
    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join("lab.yaml");
    std::fs::write(&file, "devices: []\n").unwrap();
    assert_eq!(lab::locate(None, &file), Ok(Some(file.clone())));
}

#[test]
fn the_example_lab_file_parses_with_the_passwords_in_the_environment() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lab/lab.example.yaml");
    let text = std::fs::read_to_string(path).unwrap();
    let lab = Lab::parse(
        &text,
        &env_with(&[
            ("NETTERM_LAB_SRL_PASSWORD", "srl secret"),
            ("NETTERM_LAB_CEOS_PASSWORD", "ceos secret"),
            ("NETTERM_LAB_PASSWORD", "frr secret"),
        ]),
    )
    .unwrap();
    let devices: Vec<(&str, Option<&str>)> = lab
        .devices
        .iter()
        .map(|d| (d.name.as_str(), d.password.as_deref()))
        .collect();
    assert_eq!(
        devices,
        [
            ("srl1", "srl secret"),
            ("ceos1", "ceos secret"),
            ("frr1", "frr secret")
        ]
        .map(|(name, password)| (name, Some(password)))
    );
    // The server logs in with this computer's SSH agent.
    let jump = lab.jump_host.unwrap();
    assert!(matches!(jump.auth, ProfileAuth::Agent));
}

#[test]
fn a_device_without_a_password_names_the_variable_to_set() {
    let text = "devices:\n  - name: r1\n    platform: frr\n    address: 192.0.2.1\n";
    let error = Lab::parse(text, &env_with(&[])).unwrap_err();
    assert!(error.contains("NETTERM_LAB_PASSWORD"), "{error}");

    let text = format!("{text}    password_env: R1_PASSWORD\n");
    let error = Lab::parse(&text, &env_with(&[])).unwrap_err();
    assert!(error.contains("R1_PASSWORD"), "{error}");
    let lab = Lab::parse(&text, &env_with(&[("R1_PASSWORD", "x")])).unwrap();
    assert_eq!(lab.devices[0].password.as_deref(), Some("x"));
}

#[test]
fn logins_use_the_agent_unless_a_password_is_given() {
    let text = "jump_host:
  address: 192.0.2.10
  username: root
devices:
  - name: r1
    platform: frr
    address: 192.0.2.1
    agent: true
";
    let lab = Lab::parse(text, &env_with(&[])).unwrap();
    assert!(matches!(lab.jump_host.unwrap().auth, ProfileAuth::Agent));
    assert_eq!(lab.devices[0].password, None);

    let text = "jump_host:
  address: 192.0.2.10
  username: root
  password_env: JUMP
devices:
  - name: r1
    platform: frr
    address: 192.0.2.1
    agent: true
";
    let error = Lab::parse(text, &env_with(&[])).unwrap_err();
    assert!(error.contains("JUMP"), "{error}");
    let lab = Lab::parse(text, &env_with(&[("JUMP", "pw")])).unwrap();
    assert!(matches!(
        lab.jump_host.unwrap().auth,
        ProfileAuth::Password { password } if password == "pw"
    ));

    let text = "devices:
  - name: r1
    platform: frr
    address: 192.0.2.1
    agent: true
    password_env: X
";
    let error = Lab::parse(text, &env_with(&[])).unwrap_err();
    assert!(error.contains("not both"), "{error}");
}

#[test]
fn mistakes_in_a_lab_file_are_refused() {
    let env = env_with(&[("NETTERM_LAB_PASSWORD", "x")]);
    let device = "devices:\n  - name: r1\n    address: 192.0.2.1\n";
    for (text, says) in [
        (format!("{device}    platform: frr\n    pasword: x\n"), "pasword"),
        (format!("{device}    platform: junos\n"), "junos"),
        (format!("{device}    platform: other\n"), "set `username`"),
        (
            format!("{device}    platform: other\n    username: u\n"),
            "set `prompt`",
        ),
        (
            format!("{device}    platform: frr\n    size_command: stty size\n    size_output: null\n    size_command_typo: 1\n"),
            "size_command_typo",
        ),
        ("devices: []\n".to_owned(), "no devices"),
        (
            "devices:\n  - name: _folder\n    platform: frr\n    address: 192.0.2.1\n".to_owned(),
            "reserved",
        ),
        (
            "devices:\n  - name: clab-server\n    platform: frr\n    address: 192.0.2.1\n"
                .to_owned(),
            "reserved",
        ),
        (
            "devices:\n  - name: r1\n    platform: frr\n    address: 'a b'\n".to_owned(),
            "bad address",
        ),
        (
            "devices:\n  - name: ../r1\n    platform: frr\n    address: 192.0.2.1\n".to_owned(),
            "device name",
        ),
    ] {
        let error = Lab::parse(&text, &env).unwrap_err();
        assert!(error.contains(says), "{text:?}: {error}");
    }
}

/// A lab file for the in-process servers, as platform `other`. Both
/// scripted devices understand `size` (the SSH shell prints the pty size,
/// the telnet device the last window size it was sent).
fn loopback_lab(ssh_port: u16, telnet_port: Option<u16>, jump_port: Option<u16>) -> Lab {
    let mut text = String::from("timeout: 10\n");
    if let Some(port) = jump_port {
        text += &format!(
            "jump_host:\n  address: 127.0.0.1\n  port: {port}\n  username: {}\n  password_env: LAB_JUMP\n",
            support::USERNAME
        );
    }
    text += &format!(
        "devices:
  - name: test-device
    platform: other
    address: 127.0.0.1
    port: {ssh_port}
    username: {user}
    password_env: LAB_DEVICE
    prompt: 'test-(ssh\\$|telnet>)'
    command: size
    expect: '(?m)^\\d+x\\d+$'
    large_command: lines 50000
    large_min_bytes: 500000
    size_command: size
    size_output: '{{cols}}x{{rows}}'
    exit: exit
",
        user = support::USERNAME
    );
    if let Some(port) = telnet_port {
        text += &format!("    telnet: {port}\n");
    }
    Lab::parse(
        &text,
        &env_with(&[
            ("LAB_DEVICE", support::PASSWORD),
            ("LAB_JUMP", support::PASSWORD),
        ]),
    )
    .unwrap()
}

#[tokio::test]
async fn every_check_passes_against_the_in_process_servers() {
    let ssh = TestSshServer::start().await;
    let telnet = TestTelnetServer::start().await;
    let run = LabRun::new(loopback_lab(ssh.port(), Some(telnet.port()), None)).unwrap();
    let failures = lab::run_checks(&run, ALL_CHECKS).await.failures;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn every_check_passes_through_a_jump_host() {
    let server = TestSshServer::start().await;
    let device = TestSshServer::start().await;
    // Telnet is asked for, but it is not tunnelled, so it is not checked.
    let telnet_port = support::closed_port().await;
    let run = LabRun::new(loopback_lab(
        device.port(),
        Some(telnet_port),
        Some(server.port()),
    ))
    .unwrap();
    let failures = lab::run_checks(&run, ALL_CHECKS).await.failures;
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(server.forwarded() > 0, "nothing went through the jump host");
    let device = &run.lab.devices[0];
    let outcome = Check::Telnet.run(&run, device).await;
    assert!(
        matches!(&outcome, Outcome::NotApplicable(why) if why.contains("not tunnelled")),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_device_that_does_not_answer_as_expected_fails_the_check() {
    let ssh = TestSshServer::start().await;
    let mut lab = loopback_lab(ssh.port(), None, None);
    lab.timeout = Duration::from_secs(2);
    lab.devices[0].expect = regex::Regex::new("no such output").unwrap();
    let run = LabRun::new(lab).unwrap();
    let device = &run.lab.devices[0];
    let outcome = Check::LoginAndCommand.run(&run, device).await;
    assert!(
        matches!(&outcome, Outcome::Failed(why) if why.contains("no such output")),
        "{outcome:?}"
    );
    let outcome = Check::SessionLog.run(&run, device).await;
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
}

#[tokio::test]
async fn an_unreachable_lab_device_fails_rather_than_passing() {
    let port = support::closed_port().await;
    let mut lab = loopback_lab(port, None, None);
    lab.timeout = Duration::from_secs(5);
    let run = LabRun::new(lab).unwrap();
    let failures = lab::run_checks(&run, &[Check::LoginAndCommand, Check::WrongPassword])
        .await
        .failures;
    assert_eq!(failures.len(), 2, "{failures:#?}");
}
