//! Jobs: a saved snippet run on several hosts of a temporary workspace,
//! each an in-process SSH server whose shell runs in device mode, through
//! the same credential profiles, jump hosts and known-hosts store as
//! terminal sessions.

mod support;

use std::collections::BTreeMap;
use std::fs;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::job::{
    self, HostState, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget, SkipReason,
    StartError,
};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::SessionManager;
use netterm_core::workspace::{HostId, Protocol, Workspace};
use support::{TestSshServer, CODE_QUESTION, OTP_USERNAME, PASSWORD, USERNAME, WAIT};
use tempfile::TempDir;

/// A workspace made of `files` (relative path, text), whose
/// `_snippets.yaml` holds `snippets`.
fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("jobs");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

/// A host file for a device-mode server on the loopback address, driven
/// by the frr definition.
fn device(server: &TestSshServer, extra: &str) -> String {
    device_on(server, "frr", extra)
}

/// A host file for a device-mode server, driven by `platform`.
fn device_on(server: &TestSshServer, platform: &str, extra: &str) -> String {
    format!(
        "address: 127.0.0.1\nport: {}\nplatform: {platform}\n{extra}",
        server.port()
    )
}

fn profile(name: &str, username: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: username.into(),
        auth: ProfileAuth::Password {
            password: PASSWORD.into(),
        },
        enable_secret: None,
    }
}

/// A store holding `profiles` for `workspace`.
fn store(workspace: &Workspace, profiles: &[CredentialProfile]) -> MemoryStore {
    let store = MemoryStore::new();
    for profile in profiles {
        store.put(&workspace.credential_scope(), profile).unwrap();
    }
    store
}

fn snippet(name: &str, answers: &[(&str, &str)]) -> JobStep {
    JobStep::Snippet {
        name: name.into(),
        answers: answers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn folder(path: &str) -> JobTarget {
    JobTarget::Folder { path: path.into() }
}

fn plan(workspace: &Workspace, store: &MemoryStore, target: &JobTarget, step: &JobStep) -> JobPlan {
    job::plan(workspace, &Profiles::new(store, workspace), target, step).unwrap()
}

fn ids(plan: &JobPlan) -> Vec<&str> {
    plan.hosts.iter().map(|host| host.id.as_str()).collect()
}

/// Runs `plan` to the end; its report and each host's states in order.
async fn run(
    manager: &SessionManager,
    plan: JobPlan,
) -> (JobReport, BTreeMap<HostId, Vec<HostState>>) {
    let mut job = job::start(manager, plan, JobOptions::default()).unwrap();
    let mut states: BTreeMap<HostId, Vec<HostState>> = BTreeMap::new();
    let mut streamed: BTreeMap<HostId, String> = BTreeMap::new();
    let report = tokio::time::timeout(WAIT * 3, async {
        loop {
            match job.next().await.expect("the job ended without a report") {
                JobEvent::State { host, state } => states.entry(host).or_default().push(state),
                JobEvent::Output { host, text } => {
                    streamed.entry(host).or_default().push_str(&text)
                }
                JobEvent::Backup { host, .. } => panic!("a snippet job backed up {host}"),
                JobEvent::Progress { host, .. } => panic!("a snippet job sent a file to {host}"),
                JobEvent::Finished(report) => return report,
            }
        }
    })
    .await
    .expect("the job did not finish in time");
    assert!(job.next().await.is_none(), "events after the report");
    for host in &report.hosts {
        assert_eq!(
            streamed.get(&host.id).map(String::as_str).unwrap_or(""),
            host.output,
            "the streamed output of {} is its output",
            host.id
        );
    }
    (report, states)
}

fn result<'a>(report: &'a JobReport, id: &str) -> &'a job::HostResult {
    report
        .hosts
        .iter()
        .find(|host| host.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is not in the report"))
}

fn failure(report: &JobReport, id: &str) -> String {
    match &result(report, id).state {
        HostState::Failed { reason } => reason.clone(),
        other => panic!("{id} should have failed, but is {other:?}"),
    }
}

/// Three devices in a folder; the snippet uses each host's name, a custom
/// field only two of them have (asked once for the third), and a line
/// printing colours (turned into plain text).
#[tokio::test]
async fn a_snippet_runs_on_every_host_of_a_folder_with_its_own_values() {
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let r3 = TestSshServer::start_device("r3").await;
    let (_temp, workspace) = workspace(&[
        ("Site/_folder.yaml", "credential_profile: lab\n".into()),
        ("Site/r1.yaml", device(&r1, "custom:\n  vlan: \"10\"\n")),
        ("Site/r2.yaml", device(&r2, "custom:\n  vlan: \"20\"\n")),
        ("Site/Core/r3.yaml", device(&r3, "")),
        (
            "_snippets.yaml",
            "Check: |\n  echo host {{name}} vlan {{vlan}}\n  colors\n".into(),
        ),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);

    let asking = plan(&workspace, &store, &folder("Site"), &snippet("Check", &[]));
    assert_eq!(asking.missing_variables, ["vlan"]);
    assert!(!asking.ready());
    assert!(matches!(
        job::start(&support::manager(), asking, JobOptions::default()),
        Err(StartError::MissingVariables(names)) if names == ["vlan"]
    ));

    let planned = plan(
        &workspace,
        &store,
        &folder("Site"),
        &snippet("Check", &[("vlan", "99")]),
    );
    assert!(planned.ready());
    assert_eq!(
        ids(&planned),
        ["Site/r1.yaml", "Site/r2.yaml", "Site/Core/r3.yaml"]
    );
    assert!(planned.skipped.is_empty());
    assert!(planned.production.is_empty());
    assert!(planned.hosts.iter().all(|host| host.platform == "frr"));

    let manager = support::manager();
    let (report, states) = run(&manager, planned).await;
    assert_eq!(report.hosts.len(), 3);
    for (id, name, vlan) in [
        ("Site/r1.yaml", "r1", "10"),
        ("Site/r2.yaml", "r2", "20"),
        ("Site/Core/r3.yaml", "r3", "99"),
    ] {
        let host = result(&report, id);
        assert_eq!(host.state, HostState::Done, "{id}: {}", host.output);
        assert_eq!(
            host.output,
            format!(
                "{name}# terminal length 0\n{name}# echo host {name} vlan {vlan}\nhost {name} vlan {vlan}\n{name}# colors\ndown up\n{name}#\n"
            )
        );
        assert_eq!(
            states[&HostId::new(id)],
            [
                HostState::Waiting,
                HostState::Connecting,
                HostState::Running,
                HostState::Done
            ]
        );
    }
}

/// Every reason a host is skipped, in target order, and production hosts
/// named among those that run.
#[tokio::test]
async fn pre_flight_lists_skipped_hosts_with_their_reasons() {
    let r1 = TestSshServer::start_device("r1").await;
    let lab = |extra: &str| device(&r1, &format!("credential_profile: lab\n{extra}"));
    let (_temp, workspace) = workspace(&[
        ("prod.yaml", lab("environment: production\n")),
        ("ok.yaml", lab("environment: lab\n")),
        ("telnet.yaml", lab("protocol: telnet\n")),
        ("serial.yaml", lab("protocol: serial\n")),
        (
            "unplatformed.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\ncredential_profile: lab\n",
                r1.port()
            ),
        ),
        (
            "unknown-platform.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\nplatform: toaster\ncredential_profile: lab\n",
                r1.port()
            ),
        ),
        ("no-profile.yaml", device(&r1, "")),
        (
            "not-set-up.yaml",
            device(&r1, "credential_profile: elsewhere\n"),
        ),
        ("broken-jump.yaml", lab("jump_host: gone.yaml\n")),
        (
            "bastion.yaml",
            device(&r1, "credential_profile: missing-here\n"),
        ),
        ("behind.yaml", lab("jump_host: bastion.yaml\n")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);

    let target = JobTarget::Hosts {
        ids: [
            "prod.yaml",
            "telnet.yaml",
            "serial.yaml",
            "unplatformed.yaml",
            "unknown-platform.yaml",
            "no-profile.yaml",
            "not-set-up.yaml",
            "broken-jump.yaml",
            "behind.yaml",
            "ok.yaml",
            "prod.yaml",
            "deleted-since.yaml",
        ]
        .into_iter()
        .map(HostId::new)
        .collect(),
    };
    let planned = plan(&workspace, &store, &target, &snippet("Show", &[]));
    assert_eq!(ids(&planned), ["prod.yaml", "ok.yaml"], "each host once");
    assert_eq!(planned.production, [HostId::new("prod.yaml")]);
    let skipped: Vec<(&str, &SkipReason)> = planned
        .skipped
        .iter()
        .map(|host| (host.id.as_str(), &host.reason))
        .collect();
    assert_eq!(
        skipped,
        [
            (
                "telnet.yaml",
                &SkipReason::NotSsh {
                    protocol: Protocol::Telnet
                }
            ),
            (
                "serial.yaml",
                &SkipReason::NotSsh {
                    protocol: Protocol::Serial
                }
            ),
            (
                "unplatformed.yaml",
                &SkipReason::NoPlatform { platform: None }
            ),
            (
                "unknown-platform.yaml",
                &SkipReason::NoPlatform {
                    platform: Some("toaster".into())
                }
            ),
            ("no-profile.yaml", &SkipReason::NoProfile),
            (
                "not-set-up.yaml",
                &SkipReason::ProfileNotSetUp {
                    profile: "elsewhere".into()
                }
            ),
            (
                "broken-jump.yaml",
                &SkipReason::JumpChain {
                    error: workspace
                        .jump_chain(
                            &workspace
                                .resolve_host(&HostId::new("broken-jump.yaml"))
                                .unwrap()
                        )
                        .unwrap_err()
                        .to_string()
                }
            ),
            (
                "behind.yaml",
                &SkipReason::JumpHostNeedsLogin {
                    jump_host: "bastion.yaml".into()
                }
            ),
            ("deleted-since.yaml", &SkipReason::Gone),
        ]
    );
    let messages: Vec<&str> = planned.skipped.iter().map(|h| h.message.as_str()).collect();
    assert!(messages[0].contains("telnet"), "{messages:?}");
    assert!(messages[3].contains("toaster"), "{messages:?}");
    assert!(messages[5].contains("elsewhere"), "{messages:?}");
    assert!(messages[7].contains("bastion.yaml"), "{messages:?}");

    let missing = job::plan(
        &workspace,
        &Profiles::new(&store, &workspace),
        &target,
        &snippet("Nope", &[]),
    );
    assert!(missing.is_err());
    let nothing = plan(
        &workspace,
        &store,
        &JobTarget::Hosts {
            ids: vec![HostId::new("telnet.yaml")],
        },
        &snippet("Show", &[]),
    );
    assert!(!nothing.ready());
    assert!(matches!(
        job::start(&support::manager(), nothing, JobOptions::default()),
        Err(StartError::NothingToRun)
    ));
}

/// Hosts whose key is unknown or changed fail, naming the key; the host
/// whose key is trusted runs, and nothing was trusted for the others.
#[tokio::test]
async fn untrusted_host_keys_fail_only_their_hosts() {
    let trusted = TestSshServer::start_device("trusted").await;
    let unknown = TestSshServer::start_device("unknown").await;
    let changed = TestSshServer::start_device("changed").await;
    let known_hosts = KnownHosts::in_memory();
    known_hosts
        .remember("127.0.0.1", trusted.port(), trusted.host_key())
        .unwrap();
    // Another server's key, as if the device had been replaced.
    known_hosts
        .remember("127.0.0.1", changed.port(), trusted.host_key())
        .unwrap();
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("a-trusted.yaml", device(&trusted, "")),
        ("b-unknown.yaml", device(&unknown, "")),
        ("c-changed.yaml", device(&changed, "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Show", &[]));

    let manager = SessionManager::new(known_hosts.clone());
    let (report, _) = run(&manager, planned).await;
    assert_eq!(result(&report, "a-trusted.yaml").state, HostState::Done);
    assert!(result(&report, "a-trusted.yaml")
        .output
        .contains("\ntrusted\n"));

    let unknown_reason = failure(&report, "b-unknown.yaml");
    assert!(
        unknown_reason.contains(&format!("127.0.0.1 port {}", unknown.port()))
            && unknown_reason.contains("not trusted yet"),
        "{unknown_reason}"
    );
    let changed_reason = failure(&report, "c-changed.yaml");
    assert!(
        changed_reason.contains(&format!("127.0.0.1 port {}", changed.port()))
            && changed_reason.contains("has changed"),
        "{changed_reason}"
    );
    use netterm_core::known_hosts::HostKeyStatus;
    assert!(matches!(
        known_hosts
            .check("127.0.0.1", unknown.port(), unknown.host_key())
            .unwrap(),
        HostKeyStatus::Unknown
    ));
    assert!(matches!(
        known_hosts
            .check("127.0.0.1", changed.port(), changed.host_key())
            .unwrap(),
        HostKeyStatus::Changed { .. }
    ));
}

/// A login that asks a one-time code fails that host, naming the
/// question; the other host runs.
#[tokio::test]
async fn a_login_question_fails_only_its_host() {
    let r1 = TestSshServer::start_device("r1").await;
    let otp = TestSshServer::start_device("otp").await;
    let (_temp, workspace) = workspace(&[
        ("r1.yaml", device(&r1, "credential_profile: lab\n")),
        ("otp.yaml", device(&otp, "credential_profile: otp\n")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let store = store(
        &workspace,
        &[profile("lab", USERNAME), profile("otp", OTP_USERNAME)],
    );
    let planned = plan(&workspace, &store, &folder(""), &snippet("Show", &[]));
    assert_eq!(planned.hosts.len(), 2);

    let (report, _) = run(&support::manager(), planned).await;
    assert_eq!(result(&report, "r1.yaml").state, HostState::Done);
    let reason = failure(&report, "otp.yaml");
    assert!(
        reason.contains(CODE_QUESTION.trim()) && reason.contains("never answers"),
        "{reason}"
    );
}

/// A device behind a bastion (set on its folder) is reached through it,
/// with each its own profile, as a terminal session would.
#[tokio::test]
async fn a_job_reaches_a_device_through_its_jump_host() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("edge1").await;
    let (_temp, workspace) = workspace(&[
        (
            "Site/_folder.yaml",
            "jump_host: bastion.yaml\ncredential_profile: lab\n".into(),
        ),
        (
            "bastion.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\ncredential_profile: jump\n",
                bastion.port()
            ),
        ),
        ("Site/edge1.yaml", device(&router, "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let store = store(
        &workspace,
        &[profile("lab", USERNAME), profile("jump", USERNAME)],
    );
    let planned = plan(&workspace, &store, &folder("Site"), &snippet("Show", &[]));
    assert_eq!(ids(&planned), ["Site/edge1.yaml"]);

    let (report, _) = run(&support::manager(), planned).await;
    let host = result(&report, "Site/edge1.yaml");
    assert_eq!(host.state, HostState::Done, "{}", host.output);
    assert!(host.output.contains("\nedge1\n"), "{}", host.output);
    assert_eq!(bastion.forwarded(), 1);
}

/// A host whose device cannot be reached fails with the session's reason;
/// a line the device rejects still counts as answered (the prompt came
/// back).
#[tokio::test]
async fn an_unreachable_host_fails_and_the_others_run() {
    let r1 = TestSshServer::start_device("r1").await;
    let closed = support::closed_port().await;
    let (_temp, workspace) = workspace(&[
        (
            "_folder.yaml",
            "credential_profile: lab\nplatform: frr\n".into(),
        ),
        ("r1.yaml", device(&r1, "")),
        ("gone.yaml", format!("address: 127.0.0.1\nport: {closed}\n")),
        ("_snippets.yaml", "Odd: frobnicate now\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Odd", &[]));
    assert_eq!(planned.hosts.len(), 2);

    let (report, states) = run(&support::manager(), planned).await;
    let r1 = result(&report, "r1.yaml");
    assert_eq!(r1.state, HostState::Done);
    assert!(
        r1.output
            .contains(&format!("{}frobnicate now", support::UNKNOWN_COMMAND)),
        "{}",
        r1.output
    );
    let reason = failure(&report, "gone.yaml");
    assert!(reason.contains("refused"), "{reason}");
    assert_eq!(
        states[&HostId::new("gone.yaml")][..2],
        [HostState::Waiting, HostState::Connecting]
    );
}

/// Cancelling closes every session of the run: a host still waiting for
/// its prompt and a host still waiting for room end cancelled, the one
/// waiting for its prompt keeping the output it had; a host that finished
/// first stays done.
#[tokio::test]
async fn a_cancelled_job_keeps_the_output_collected() {
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let r3 = TestSshServer::start_device("r3").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("r1.yaml", device(&r1, "custom:\n  command: echo hi\n")),
        ("r2.yaml", device(&r2, "custom:\n  command: flood\n")),
        ("r3.yaml", device(&r3, "custom:\n  command: echo hi\n")),
        ("_snippets.yaml", "Run: \"{{command}}\"\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Run", &[]));
    assert_eq!(ids(&planned), ["r1.yaml", "r2.yaml", "r3.yaml"]);

    let options = JobOptions {
        parallel: 1,
        ..JobOptions::default()
    };
    let mut job = job::start(&support::manager(), planned, options).unwrap();
    let mut states: BTreeMap<HostId, Vec<HostState>> = BTreeMap::new();
    let report = tokio::time::timeout(WAIT, async {
        loop {
            match job.next().await.expect("the job ended without a report") {
                JobEvent::Output { text, .. } if text.contains("flood flood") => job.cancel(),
                JobEvent::State { host, state } => states.entry(host).or_default().push(state),
                JobEvent::Finished(report) => return report,
                _ => {}
            }
        }
    })
    .await
    .expect("the cancelled job did not finish");

    let done = result(&report, "r1.yaml");
    assert_eq!(done.state, HostState::Done);
    assert!(done.output.contains("\nhi\n"), "{}", done.output);
    let flooding = result(&report, "r2.yaml");
    assert_eq!(flooding.state, HostState::Cancelled);
    assert!(
        flooding
            .output
            .starts_with("r2# terminal length 0\nr2# flood\nflood flood"),
        "{}",
        &flooding.output[..80]
    );
    let waiting = result(&report, "r3.yaml");
    assert_eq!(waiting.state, HostState::Cancelled);
    assert_eq!(waiting.output, "");
    assert_eq!(
        states[&HostId::new("r3.yaml")],
        [HostState::Waiting, HostState::Cancelled]
    );
    assert_eq!(r3.connections(), 0, "a waiting host never connects");
    for server in [&r1, &r2] {
        closes(server).await;
    }
}

/// Waits until `server` has no connection open.
async fn closes(server: &TestSshServer) {
    tokio::time::timeout(WAIT, async {
        while server.connections_open() > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a job session stayed open");
}

/// Runs `plan` with `options` to the end; its report.
async fn run_with(manager: &SessionManager, plan: JobPlan, options: JobOptions) -> JobReport {
    let mut job = job::start(manager, plan, options).unwrap();
    tokio::time::timeout(WAIT * 3, async {
        loop {
            if let JobEvent::Finished(report) =
                job.next().await.expect("the job ended without a report")
            {
                return report;
            }
        }
    })
    .await
    .expect("the job did not finish in time")
}

/// The EOS definition has a privilege command and password question: the
/// job enters privileged mode with the profile's enable secret, and fails
/// a host whose profile has none, or one the device refuses (asking again,
/// or at once back at the unprivileged prompt). A device already
/// privileged is not asked.
#[tokio::test]
async fn privileged_mode_is_entered_with_the_enable_secret() {
    let r1 = TestSshServer::start_unprivileged_device("r1").await;
    let r2 = TestSshServer::start_unprivileged_device("r2").await;
    let r3 = TestSshServer::start_unprivileged_device("r3").await;
    let r4 = TestSshServer::start_device("r4").await;
    let r5 = TestSshServer::start_unprivileged_device("r5").await;
    let (_temp, workspace) = workspace(&[
        (
            "r1.yaml",
            device_on(&r1, "eos", "credential_profile: enable\n"),
        ),
        (
            "r2.yaml",
            device_on(&r2, "eos", "credential_profile: plain\n"),
        ),
        (
            "r3.yaml",
            device_on(&r3, "eos", "credential_profile: wrong\n"),
        ),
        (
            "r4.yaml",
            device_on(&r4, "eos", "credential_profile: plain\n"),
        ),
        (
            "r5.yaml",
            device_on(&r5, "eos", "credential_profile: denied\n"),
        ),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let with_secret = |name: &str, secret: &str| CredentialProfile {
        enable_secret: Some(secret.into()),
        ..profile(name, USERNAME)
    };
    let store = store(
        &workspace,
        &[
            with_secret("enable", support::ENABLE_SECRET),
            profile("plain", USERNAME),
            with_secret("wrong", "open sesame"),
            with_secret("denied", support::DENIED_SECRET),
        ],
    );
    let planned = plan(&workspace, &store, &folder(""), &snippet("Show", &[]));
    assert!(planned.hosts.iter().all(|host| host.platform == "eos"));
    assert!(!format!("{planned:?}").contains(support::ENABLE_SECRET));

    let report = run_with(&support::manager(), planned, JobOptions::default()).await;
    let r1 = result(&report, "r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert_eq!(
        r1.output,
        "r1> enable\nPassword:\nr1# terminal length 0\nr1# show hostname\nr1\nr1#\n"
    );
    let reason = failure(&report, "r2.yaml");
    assert!(
        reason.contains("\"enable\" asked for a password")
            && reason.contains("profile plain has no enable secret"),
        "{reason}"
    );
    let reason = failure(&report, "r3.yaml");
    assert!(
        reason.contains("refused the enable secret of the credential profile wrong"),
        "{reason}"
    );
    assert!(!result(&report, "r3.yaml").output.contains("open sesame"));
    let r4 = result(&report, "r4.yaml");
    assert_eq!(r4.state, HostState::Done, "{}", r4.output);
    assert!(r4.output.starts_with("r4# enable\nr4# terminal length 0\n"));
    let reason = failure(&report, "r5.yaml");
    assert!(
        reason.contains("refused the enable secret of the credential profile denied"),
        "{reason}"
    );
    assert!(result(&report, "r5.yaml")
        .output
        .contains(support::ACCESS_DENIED));
}

/// A privilege command that comes back to the very prompt it was typed at,
/// having said something (IOS with no enable password: "% No password
/// set" at `r1>`), did not reach privileged mode: the host fails with what
/// the device answered, and nothing more is sent. A device that prints
/// something and changes its prompt got there, and so did one already
/// privileged that says nothing and stays at `r3#`.
#[tokio::test]
async fn a_privilege_command_that_stays_at_the_prompt_saying_something_fails_the_host() {
    let r1 = TestSshServer::start_unprivileged_device("r1").await;
    r1.set_enable(support::EnableMode::NotSet);
    let r2 = TestSshServer::start_unprivileged_device("r2").await;
    r2.set_enable(support::EnableMode::Notice(
        "Entering privileged mode".into(),
    ));
    let r3 = TestSshServer::start_device("r3").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: enable\n".into()),
        ("r1.yaml", device_on(&r1, "eos", "")),
        ("r2.yaml", device_on(&r2, "eos", "")),
        ("r3.yaml", device_on(&r3, "eos", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let store = store(
        &workspace,
        &[CredentialProfile {
            enable_secret: Some(support::ENABLE_SECRET.into()),
            ..profile("enable", USERNAME)
        }],
    );
    let planned = plan(&workspace, &store, &folder(""), &snippet("Show", &[]));
    let report = run_with(&support::manager(), planned, JobOptions::default()).await;

    let reason = failure(&report, "r1.yaml");
    assert_eq!(
        reason,
        format!(
            "\"enable\" did not reach privileged mode: the device answered {:?} and stayed at r1>",
            support::NO_ENABLE_PASSWORD
        )
    );
    let r1 = result(&report, "r1.yaml");
    assert!(!r1.output.contains("show hostname"), "{}", r1.output);
    assert!(!r1.output.contains("terminal length"), "{}", r1.output);

    let r2 = result(&report, "r2.yaml");
    assert_eq!(r2.state, HostState::Done, "{}", r2.output);
    assert!(
        r2.output
            .starts_with("r2> enable\nEntering privileged mode\nr2# terminal length 0\n"),
        "{}",
        r2.output
    );
    let r3 = result(&report, "r3.yaml");
    assert_eq!(r3.state, HostState::Done, "{}", r3.output);
    assert!(r3.output.starts_with("r3# enable\nr3# terminal length 0\n"));
}

/// A "Password:" right after logging in is a login question, even where
/// it matches the privilege password pattern: the host fails saying so,
/// and neither the privilege command nor the enable secret is typed into
/// it.
#[tokio::test]
async fn a_password_question_right_after_login_fails_the_host_and_is_never_answered() {
    let r1 = TestSshServer::start_unprivileged_device("r1").await;
    r1.ask_password_at_login();
    let r2 = TestSshServer::start_unprivileged_device("r2").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: enable\n".into()),
        ("r1.yaml", device_on(&r1, "eos", "")),
        ("r2.yaml", device_on(&r2, "eos", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let store = store(
        &workspace,
        &[CredentialProfile {
            enable_secret: Some(support::ENABLE_SECRET.into()),
            ..profile("enable", USERNAME)
        }],
    );
    let planned = plan(&workspace, &store, &folder(""), &snippet("Show", &[]));
    let report = run_with(&support::manager(), planned, JobOptions::default()).await;

    let reason = failure(&report, "r1.yaml");
    assert_eq!(
        reason,
        format!(
            "the device asked {:?} after logging in, before its first prompt; a job never answers login questions",
            support::LOGIN_QUESTION.trim()
        )
    );
    assert_eq!(r1.typed_at_login(), Vec::<String>::new());
    let output = &result(&report, "r1.yaml").output;
    assert_eq!(output.trim(), support::LOGIN_QUESTION.trim(), "{output}");
    // The other host still enters privileged mode as ever.
    let r2 = result(&report, "r2.yaml");
    assert_eq!(r2.state, HostState::Done, "{}", r2.output);
}

/// The command timeout counts from the last output: a command that keeps
/// printing for longer than the timeout is waited for, to its prompt.
#[tokio::test]
async fn output_still_arriving_after_the_command_timeout_is_waited_for() {
    let r1 = TestSshServer::start_device("r1").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("r1.yaml", device(&r1, "")),
        ("_snippets.yaml", "Slow: drip 12 250\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Slow", &[]));
    // 12 lines a quarter of a second apart: 3 s in all, never 2 s quiet.
    let options = JobOptions {
        command_timeout: std::time::Duration::from_secs(2),
        ..JobOptions::default()
    };
    let report = run_with(&support::manager(), planned, options).await;
    let r1 = result(&report, "r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    let all: String = (1..=12).map(|n| format!("line {n}\n")).collect();
    assert_eq!(
        r1.output,
        format!("r1# terminal length 0\nr1# drip 12 250\n{all}r1#\n")
    );
}

/// Output that never ends is ended by the command limit, which the reason
/// names; the default limit is half an hour.
#[tokio::test]
async fn output_that_never_ends_fails_its_host_at_the_command_limit() {
    let r1 = TestSshServer::start_device("r1").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("r1.yaml", device(&r1, "")),
        // A line every 100 ms, for hours.
        ("_snippets.yaml", "Endless: drip 100000 100\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Endless", &[]));
    assert_eq!(
        JobOptions::default().command_limit,
        std::time::Duration::from_secs(30 * 60)
    );
    let options = JobOptions {
        command_timeout: std::time::Duration::from_secs(1),
        command_limit: std::time::Duration::from_secs(2),
        ..JobOptions::default()
    };
    let report = run_with(&support::manager(), planned, options).await;
    assert_eq!(
        failure(&report, "r1.yaml"),
        "no prompt after \"drip 100000 100\": it was still showing output after 2 s, the most one command may take"
    );
    assert!(
        result(&report, "r1.yaml").output.contains("\nline 5\n"),
        "{}",
        result(&report, "r1.yaml").output
    );
    closes(&r1).await;
}

/// A snippet line that makes the device ask for a password fails the host
/// at the question; the next line is never typed into it.
#[tokio::test]
async fn a_snippet_line_asking_for_a_password_fails_its_host() {
    let r1 = TestSshServer::start_device("r1").await;
    let (_temp, workspace) = workspace(&[
        (
            "r1.yaml",
            device_on(&r1, "eos", "credential_profile: lab\n"),
        ),
        (
            "_snippets.yaml",
            "Down: \"disable\\nenable\\nshow hostname\"\n".into(),
        ),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Down", &[]));
    let report = run_with(&support::manager(), planned, JobOptions::default()).await;
    let reason = failure(&report, "r1.yaml");
    assert!(
        reason.starts_with("\"enable\" asked for a password"),
        "{reason}"
    );
    assert!(!result(&report, "r1.yaml").output.contains("show hostname"));
}

/// Output the device pages is captured whole once the definition's
/// disable-paging command has turned paging off; without that command the
/// device stops at `--More--`, and the host fails after the command
/// timeout.
#[tokio::test]
async fn paged_output_is_captured_whole_after_paging_is_turned_off() {
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("r1.yaml", device(&r1, "")),
        ("r2.yaml", device_on(&r2, "pager", "")),
        (
            "_platforms.yaml",
            "pager:\n  prompt: '[\\w.\\-]+#'\n".into(),
        ),
        ("_snippets.yaml", "Long: lines 60\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Long", &[]));
    assert_eq!(planned.hosts.len(), 2);

    let options = JobOptions {
        command_timeout: std::time::Duration::from_secs(1),
        ..JobOptions::default()
    };
    let report = run_with(&support::manager(), planned, options).await;
    let r1 = result(&report, "r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    let all: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    assert_eq!(
        r1.output,
        format!("r1# terminal length 0\nr1# lines 60\n{all}r1#\n")
    );
    let reason = failure(&report, "r2.yaml");
    assert_eq!(
        reason,
        "no prompt after \"lines 60\": the device showed nothing more for 1 s"
    );
    let r2 = result(&report, "r2.yaml");
    assert!(r2.output.contains("line 24\n"), "{}", r2.output);
    assert!(!r2.output.contains("line 25\n"), "{}", r2.output);
    assert!(r2.output.contains(support::MORE.trim()), "{}", r2.output);
}

/// A command that never gives the prompt back fails its host after the
/// command timeout, closing its session; the other host is done.
#[tokio::test]
async fn a_command_that_never_returns_fails_only_its_host() {
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("r1.yaml", device(&r1, "custom:\n  command: hang\n")),
        (
            "r2.yaml",
            device(&r2, "custom:\n  command: show hostname\n"),
        ),
        (
            "_snippets.yaml",
            "Run: \"{{command}}\\necho after\"\n".into(),
        ),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Run", &[]));
    assert_eq!(
        JobOptions::default().command_timeout,
        std::time::Duration::from_secs(60)
    );

    let options = JobOptions {
        command_timeout: std::time::Duration::from_secs(1),
        ..JobOptions::default()
    };
    let report = run_with(&support::manager(), planned, options).await;
    assert_eq!(
        failure(&report, "r1.yaml"),
        "no prompt after \"hang\": the device showed nothing more for 1 s"
    );
    assert!(!result(&report, "r1.yaml").output.contains("after"));
    let r2 = result(&report, "r2.yaml");
    assert_eq!(r2.state, HostState::Done, "{}", r2.output);
    assert!(r2.output.contains("\nafter\n"), "{}", r2.output);
    closes(&r1).await;
}

/// Hosts that all lead to one server: the server never has more
/// connections open at once than the job's parallel limit, 10 unless the
/// run says otherwise.
#[tokio::test]
async fn no_more_hosts_than_the_parallel_limit_connect_at_once() {
    for (parallel, hosts) in [(Some(2), 5), (None, 12)] {
        let server = TestSshServer::start_device("r").await;
        let mut files = vec![
            ("_folder.yaml", "credential_profile: lab\n".to_string()),
            ("_snippets.yaml", "Slow: pause 300\n".to_string()),
        ];
        let names: Vec<String> = (1..=hosts).map(|n| format!("r{n:02}.yaml")).collect();
        for name in &names {
            files.push((name.as_str(), device(&server, "")));
        }
        let (_temp, workspace) = workspace(&files);
        let store = store(&workspace, &[profile("lab", USERNAME)]);
        let planned = plan(&workspace, &store, &folder(""), &snippet("Slow", &[]));
        assert_eq!(planned.hosts.len(), hosts);
        for refused in [0, job::MAX_PARALLEL + 1] {
            let options = JobOptions {
                parallel: refused,
                ..JobOptions::default()
            };
            assert!(matches!(
                job::start(&support::manager(), planned.clone(), options),
                Err(StartError::Parallel(n)) if n == refused
            ));
        }

        let options = match parallel {
            Some(parallel) => JobOptions {
                parallel,
                ..JobOptions::default()
            },
            None => JobOptions::default(),
        };
        let report = run_with(&support::manager(), planned, options).await;
        assert!(
            report
                .hosts
                .iter()
                .all(|host| host.state == HostState::Done),
            "{report:?}"
        );
        assert_eq!(server.connections(), hosts);
        assert_eq!(
            server.most_connections_open(),
            parallel.unwrap_or(job::DEFAULT_PARALLEL)
        );
        closes(&server).await;
    }
    assert_eq!(job::DEFAULT_PARALLEL, 10);
    assert_eq!(job::MAX_PARALLEL, 50);
}
