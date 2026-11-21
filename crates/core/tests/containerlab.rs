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

use lab::Platform;
use lab::{Check, Lab, LabRun, Outcome, ALL_CHECKS};
use netterm_core::credentials::ProfileAuth;
use support::scp::ScpQuirk;
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

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_paging() {
    lab_check(Check::Paging).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_config_backup() {
    lab_check(Check::ConfigBackup).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_privilege() {
    lab_check(Check::Privilege).await;
}

#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_second_channel() {
    lab_check(Check::SecondChannel).await;
}

/// Files: each device with what `docs/containerlab.md` says it offers (SFTP:
/// list, upload, download and compare, rename, delete; SCP on the device
/// without SFTP: send, fetch and compare), through the jump host.
#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_files() {
    lab_check(Check::Files).await;
}

/// Host enrichment: the shipped script fetches the facts of the devices
/// NAPALM has a driver for, and applying the preview fills them in.
#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_enrichment() {
    let Ok(Some(lab)) = lab::from_environment().map_err(|error| panic!("{error}")) else {
        eprintln!("{}", lab::SKIP_MESSAGE);
        return;
    };
    match lab::enrichment::check(&lab).await {
        Err(reason) => eprintln!("[lab] enrichment: NOT CHECKED: {reason}"),
        Ok(checked) => {
            for note in &checked.notes {
                eprintln!("[lab] enrichment: {note}");
            }
            assert!(
                checked.failures.is_empty(),
                "enrichment failed:\n{}",
                checked.failures.join("\n")
            );
            eprintln!("[lab] enrichment: {} device(s) filled in", checked.filled);
        }
    }
}

/// Tunnel ports: a script reaches NETCONF on the Junos device's port 830
/// (ncclient, the device key checked) and eAPI on the cEOS device's port
/// 443 (HTTPS) through the jump host.
#[tokio::test]
#[ignore = "containerlab suite: needs a lab (npm run test:lab, docs/containerlab.md)"]
async fn lab_tunnel_ports() {
    let Ok(Some(lab)) = lab::from_environment().map_err(|error| panic!("{error}")) else {
        eprintln!("{}", lab::SKIP_MESSAGE);
        return;
    };
    match lab::tunnel_ports::check(&lab).await {
        Err(reason) => eprintln!("[lab] tunnel ports: NOT CHECKED: {reason}"),
        Ok(reported) => {
            let mut failures = Vec::new();
            for r in &reported {
                eprintln!(
                    "[lab] tunnel ports: {} {}: {}: {}",
                    r.check,
                    r.host,
                    r.result.to_uppercase(),
                    r.detail
                );
                if r.result == "failed" {
                    failures.push(format!("{} {}: {}", r.check, r.host, r.detail));
                }
            }
            assert!(
                failures.is_empty(),
                "tunnel ports failed:\n{}",
                failures.join("\n")
            );
        }
    }
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
            ("NETTERM_LAB_CSR_PASSWORD", "csr secret"),
            ("NETTERM_LAB_JUNOS_PASSWORD", "junos secret"),
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
            ("frr1", "frr secret"),
            ("frr1-vtysh", "frr secret"),
            ("csr1", "csr secret"),
            ("cjunos1", "junos secret"),
        ]
        .map(|(name, password)| (name, Some(password)))
    );
    // Each device is checked against its built-in platform definition,
    // except FRR's root (a shell).
    let definitions: Vec<Option<&str>> = lab
        .devices
        .iter()
        .map(|d| d.definition.as_deref())
        .collect();
    assert_eq!(
        definitions,
        [
            Some("srlinux"),
            Some("eos"),
            None,
            Some("frr"),
            Some("ios"),
            Some("junos")
        ]
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
        (format!("{device}    platform: vyos\n"), "vyos"),
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
    let files = tempfile::TempDir::new().unwrap();
    ssh.serve_sftp(files.path());
    let telnet = TestTelnetServer::start().await;
    let run = LabRun::new(loopback_lab(ssh.port(), Some(telnet.port()), None)).unwrap();
    let failures = lab::run_checks(&run, ALL_CHECKS).await.failures;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn every_check_passes_through_a_jump_host() {
    let server = TestSshServer::start().await;
    let device = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    device.serve_sftp(files.path());
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
async fn telnet_behind_a_jump_host_is_checked_at_its_own_address() {
    // A lab reached through its server, whose telnet is published on an
    // address of its own (containerlab's `ports:`): telnet goes there
    // directly while SSH still goes through the jump host.
    let server = TestSshServer::start().await;
    let device = TestSshServer::start().await;
    let telnet = TestTelnetServer::start().await;
    let mut lab = loopback_lab(device.port(), Some(telnet.port()), Some(server.port()));
    let text = "devices:\n  - name: d\n    platform: frr\n    address: 10.9.9.9\n    password: p\n    telnet: 2323\n    telnet_address: 192.0.2.1\n";
    let parsed = Lab::parse(text, &env_with(&[])).unwrap();
    assert_eq!(
        parsed.devices[0].telnet_address.as_deref(),
        Some("192.0.2.1")
    );
    lab.devices[0].telnet_address = Some("127.0.0.1".into());
    let run = LabRun::new(lab).unwrap();
    let outcome = Check::Telnet.run(&run, &run.lab.devices[0]).await;
    assert!(matches!(outcome, Outcome::Passed(_)), "{outcome:?}");
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

/// What a folder holds, by name.
fn names_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn the_files_check_moves_a_file_over_sftp_and_leaves_nothing_behind() {
    let ssh = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    std::fs::write(files.path().join("startup-config"), "hostname r1\n").unwrap();
    // What an earlier run that failed half way left behind is cleared.
    std::fs::write(files.path().join(lab::files::NAME), "old").unwrap();
    ssh.serve_sftp(files.path());
    let run = LabRun::new(loopback_lab(ssh.port(), None, None)).unwrap();

    let outcome = Check::Files.run(&run, &run.lab.devices[0]).await;

    let Outcome::Passed(note) = &outcome else {
        panic!("{outcome:?}");
    };
    for said in ["SFTP", "uploaded", "downloaded", "renamed", "deleted"] {
        assert!(note.contains(said), "{note}");
    }
    assert_eq!(names_in(files.path()), ["startup-config"]);
}

/// A lab of one device as the built-in definition `definition` drives it
/// (its file folder, its hints), checked as lab platform `platform`.
fn files_lab(port: u16, platform: Platform, definition: &str) -> Lab {
    let mut lab = loopback_lab(port, None, None);
    lab.devices[0].platform = platform;
    lab.devices[0].definition = Some(definition.into());
    lab
}

#[tokio::test]
async fn the_files_check_falls_back_to_scp_on_a_device_without_sftp() {
    // As IOS-XE: the SFTP subsystem is accepted, its channel closed and
    // the connection with it; SCP serves `flash:` (the built-in
    // definition's file folder).
    let ssh = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    ssh.close_sftp_and_hang_up();
    ssh.serve_scp(files.path());
    let run = LabRun::new(files_lab(ssh.port(), Platform::Ios, "ios")).unwrap();

    let outcome = Check::Files.run(&run, &run.lab.devices[0]).await;

    let Outcome::Passed(note) = &outcome else {
        panic!("{outcome:?}");
    };
    for said in ["no SFTP", "SCP", "flash:", "fetched"] {
        assert!(note.contains(said), "{note}");
    }
    // SCP cannot delete: the one small file stays, under a fixed name.
    assert_eq!(names_in(files.path()), [lab::files::NAME]);
    let kept = std::fs::metadata(files.path().join(lab::files::NAME)).unwrap();
    assert_eq!(kept.len(), lab::files::SIZE as u64);
}

#[tokio::test]
async fn a_device_with_scp_turned_off_is_not_checked_and_names_what_to_paste() {
    // IOS before `ip scp server enable`.
    let ssh = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    ssh.close_sftp_and_hang_up();
    ssh.serve_scp(files.path());
    ssh.scp_quirk(ScpQuirk::AdministrativelyDisabled);
    let run = LabRun::new(files_lab(ssh.port(), Platform::Ios, "ios")).unwrap();

    let outcome = Check::Files.run(&run, &run.lab.devices[0]).await;

    let Outcome::NotApplicable(why) = &outcome else {
        panic!("{outcome:?}");
    };
    for said in [
        "Administratively disabled",
        "ip scp server enable",
        "lab/configs/csr1-files.cfg",
    ] {
        assert!(why.contains(said), "{why}");
    }
}

#[tokio::test]
async fn a_junos_device_without_its_sftp_server_is_not_checked_and_names_what_to_paste() {
    // A stock Junos: the SFTP subsystem is refused, SCP works.
    let ssh = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    ssh.refuse_sftp();
    ssh.serve_scp(files.path());
    let run = LabRun::new(files_lab(ssh.port(), Platform::Junos, "junos")).unwrap();

    let outcome = Check::Files.run(&run, &run.lab.devices[0]).await;

    let Outcome::NotApplicable(why) = &outcome else {
        panic!("{outcome:?}");
    };
    for said in [
        "set system services ssh sftp-server",
        "lab/configs/cjunos1-files.set",
    ] {
        assert!(why.contains(said), "{why}");
    }
    assert_eq!(names_in(files.path()), [] as [&str; 0], "nothing was sent");
}

#[tokio::test]
async fn a_device_that_should_offer_sftp_and_offers_none_fails_the_files_check() {
    let ssh = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    ssh.close_sftp_at_once();
    ssh.serve_scp(files.path());
    let run = LabRun::new(files_lab(ssh.port(), Platform::Ceos, "eos")).unwrap();

    let outcome = Check::Files.run(&run, &run.lab.devices[0]).await;

    assert!(
        matches!(&outcome, Outcome::Failed(why) if why.contains("SFTP")),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_device_that_refuses_scp_in_its_own_words_fails_the_files_check_with_them() {
    // cEOS below privilege 15: no SFTP for the account, and SCP answers a
    // plain line. Not a setting the lab's files turn on: a failure.
    let ssh = TestSshServer::start().await;
    let files = tempfile::TempDir::new().unwrap();
    ssh.close_sftp_at_once();
    ssh.serve_scp(files.path());
    ssh.scp_quirk(ScpQuirk::PlainText);
    let run = LabRun::new(files_lab(ssh.port(), Platform::Ios, "ios")).unwrap();

    let outcome = Check::Files.run(&run, &run.lab.devices[0]).await;

    assert!(
        matches!(&outcome, Outcome::Failed(why) if why.contains(support::scp::PLAIN_TEXT_REFUSAL.trim())),
        "{outcome:?}"
    );
}

/// A lab of one device-mode server (`r1`, unprivileged until `enable`),
/// checked against the built-in EOS definition.
fn device_lab(port: u16) -> Lab {
    let text = format!(
        "timeout: 10
devices:
  - name: r1
    platform: other
    definition: eos
    address: 127.0.0.1
    port: {port}
    username: {user}
    password_env: LAB_DEVICE
    enable_secret_env: LAB_ENABLE
    prompt: 'r1[>#]'
    command: show hostname
    expect: r1
    exit: exit
    paging_command: lines 300
    privilege_check: show hostname
    privilege_expect: '(?m)^r1# show hostname'
    config_end: '^end$'
",
        user = support::USERNAME
    );
    Lab::parse(
        &text,
        &env_with(&[
            ("LAB_DEVICE", support::PASSWORD),
            ("LAB_ENABLE", support::ENABLE_SECRET),
        ]),
    )
    .unwrap()
}

const PLATFORM_CHECKS: &[Check] = &[
    Check::Paging,
    Check::ConfigBackup,
    Check::Privilege,
    Check::SecondChannel,
];

#[tokio::test]
async fn the_platform_checks_pass_against_a_device_mode_server() {
    let device = TestSshServer::start_unprivileged_device("r1").await;
    let run = LabRun::new(device_lab(device.port())).unwrap();
    let device_entry = &run.lab.devices[0];
    for check in PLATFORM_CHECKS {
        let outcome = check.run(&run, device_entry).await;
        assert!(
            matches!(outcome, Outcome::Passed(_)),
            "{check:?}: {outcome:?}"
        );
    }
    let outcome = Check::SecondChannel.run(&run, device_entry).await;
    assert!(
        matches!(&outcome, Outcome::Passed(note) if note.contains("second channel accepted")),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_device_refusing_a_second_channel_is_reported_and_still_backed_up() {
    let device = TestSshServer::start_unprivileged_device("r1").await;
    device.refuse_second_channel();
    let run = LabRun::new(device_lab(device.port())).unwrap();
    let outcome = Check::SecondChannel.run(&run, &run.lab.devices[0]).await;
    assert!(
        matches!(&outcome, Outcome::Passed(note) if note.contains("REFUSED")),
        "{outcome:?}"
    );
}
