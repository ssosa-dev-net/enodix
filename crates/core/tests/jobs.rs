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

/// A host file for a device-mode server on the loopback address.
fn device(server: &TestSshServer, extra: &str) -> String {
    format!(
        "address: 127.0.0.1\nport: {}\nplatform: frr\n{extra}",
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
                "{name}# echo host {name} vlan {vlan}\nhost {name} vlan {vlan}\n{name}# colors\ndown up\n{name}#\n"
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

/// Cancelling closes the job's sessions: a host still waiting for its
/// prompt ends cancelled, keeping the output it had.
#[tokio::test]
async fn a_cancelled_job_keeps_the_output_collected() {
    let r1 = TestSshServer::start_device("r1").await;
    let (_temp, workspace) = workspace(&[
        ("r1.yaml", device(&r1, "credential_profile: lab\n")),
        ("_snippets.yaml", "Flood: flood\n".into()),
    ]);
    let store = store(&workspace, &[profile("lab", USERNAME)]);
    let planned = plan(&workspace, &store, &folder(""), &snippet("Flood", &[]));

    let mut job = job::start(&support::manager(), planned, JobOptions::default()).unwrap();
    let report = tokio::time::timeout(WAIT, async {
        loop {
            match job.next().await.expect("the job ended without a report") {
                JobEvent::Output { text, .. } if text.contains("flood flood") => job.cancel(),
                JobEvent::Finished(report) => return report,
                _ => {}
            }
        }
    })
    .await
    .expect("the cancelled job did not finish");
    let host = result(&report, "r1.yaml");
    assert_eq!(host.state, HostState::Cancelled);
    assert!(
        host.output.starts_with("r1# flood\nflood flood"),
        "{}",
        &host.output[..80]
    );
}
