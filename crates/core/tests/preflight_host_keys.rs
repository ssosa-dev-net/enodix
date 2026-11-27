//! Pre-flight and host keys (#129): a planned host whose run would meet a
//! host key the app has not trusted yet (the device's, or a jump host's on
//! the way) is still planned, with a warning naming the key; the known
//! hosts are only looked up, nothing is connected, and the run decides.

mod support;

use std::time::Duration;

use enodix_core::job::{
    self, HostState, JobOptions, JobPlan, JobStep, JobTarget, PlannedHost, UntrustedKey,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::SessionManager;
use enodix_core::workspace::HostId;
use support::jobs::{folder, host_at, run, snippet, Lab};
use support::TestSshServer;

/// A host file for `address` (port 22) on `platform`, logging in with the
/// profile `lab`.
fn at(address: &str, platform: &str, extra: &str) -> String {
    format!("address: {address}\nplatform: {platform}\ncredential_profile: lab\n{extra}")
}

fn planned<'a>(plan: &'a JobPlan, id: &str) -> &'a PlannedHost {
    plan.hosts
        .iter()
        .find(|host| host.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is not planned: {plan:?}"))
}

/// The keys `id` is warned about, as (jump host, address, port).
fn untrusted(plan: &JobPlan, id: &str) -> Vec<(Option<String>, String, u16)> {
    planned(plan, id)
        .host_keys
        .as_ref()
        .map(|warning| {
            warning
                .keys
                .iter()
                .map(
                    |UntrustedKey {
                         jump_host,
                         address,
                         port,
                     }| { (jump_host.clone(), address.clone(), *port) },
                )
                .collect()
        })
        .unwrap_or_default()
}

fn message(plan: &JobPlan, id: &str) -> String {
    planned(plan, id)
        .host_keys
        .as_ref()
        .unwrap_or_else(|| panic!("{id} has no warning"))
        .message
        .clone()
}

/// Some key to trust, any server's.
async fn some_key() -> russh::keys::PublicKey {
    TestSshServer::start().await.host_key().clone()
}

/// A device whose key the app has not trusted is still planned, with a
/// warning naming the device's address and port and saying how to trust
/// it; the trusted one beside it is left alone. The plan itself (before
/// the lookup) has no warning, and the warning is in the serialised plan.
#[tokio::test]
async fn a_host_with_no_known_key_is_planned_with_a_warning() {
    let lab = Lab::new(&[
        ("trusted.yaml", at("10.9.0.1", "frr", "")),
        ("unknown.yaml", at("10.9.0.2", "frr", "port: 2222\n")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let known_hosts = KnownHosts::in_memory();
    known_hosts
        .remember("10.9.0.1", 22, &some_key().await)
        .unwrap();

    let mut plan = lab.plan(&folder(""), &snippet("Show"));
    assert!(plan.hosts.iter().all(|host| host.host_keys.is_none()));
    plan.check_host_keys(&known_hosts);

    assert_eq!(plan.hosts.len(), 2, "both planned: {plan:?}");
    assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    assert_eq!(planned(&plan, "trusted.yaml").host_keys, None);
    assert_eq!(
        untrusted(&plan, "unknown.yaml"),
        [(None, "10.9.0.2".to_string(), 2222)]
    );
    assert_eq!(
        message(&plan, "unknown.yaml"),
        "the host key of the device (10.9.0.2 port 2222) is not trusted yet, so the job will \
         refuse this host: connecting to it once in a terminal (Trust and remember) trusts the key"
    );
    assert_eq!(plan.untrusted_keys(), 1);
    assert!(plan.ready());

    let shown = serde_json::to_value(&plan).unwrap();
    let hosts = shown["hosts"].as_array().unwrap();
    assert!(hosts[0].get("hostKeys").is_none(), "{shown}");
    assert_eq!(
        hosts[1]["hostKeys"]["keys"],
        serde_json::json!([{ "address": "10.9.0.2", "port": 2222 }])
    );

    // Trusted since: checking again takes the warning away.
    known_hosts
        .remember("10.9.0.2", 2222, &some_key().await)
        .unwrap();
    plan.check_host_keys(&known_hosts);
    assert_eq!(plan.untrusted_keys(), 0);
}

/// A device behind a jump host whose key is not known names the jump
/// host's key (by the jump host's address and port), not the device's;
/// with both unknown, both are named in the order the run meets them.
#[tokio::test]
async fn a_host_behind_a_jump_host_whose_key_is_not_known_names_the_jump_host() {
    let lab = Lab::new(&[
        (
            "bastion.yaml",
            "address: 10.9.1.1\nport: 2200\ncredential_profile: lab\n".into(),
        ),
        ("Site/_folder.yaml", "jump_host: bastion.yaml\n".into()),
        ("Site/r1.yaml", at("10.9.1.11", "frr", "")),
        ("Site/r2.yaml", at("10.9.1.12", "frr", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let known_hosts = KnownHosts::in_memory();
    known_hosts
        .remember("10.9.1.11", 22, &some_key().await)
        .unwrap();

    let mut plan = lab.plan(&folder("Site"), &snippet("Show"));
    plan.check_host_keys(&known_hosts);

    assert_eq!(
        untrusted(&plan, "Site/r1.yaml"),
        [(
            Some("bastion.yaml".to_string()),
            "10.9.1.1".to_string(),
            2200
        )]
    );
    assert_eq!(
        message(&plan, "Site/r1.yaml"),
        "the host key of its jump host bastion.yaml (10.9.1.1 port 2200) is not trusted yet, so \
         the job will refuse this host: connecting to it once in a terminal (Trust and remember) \
         trusts the key"
    );
    assert_eq!(
        untrusted(&plan, "Site/r2.yaml"),
        [
            (
                Some("bastion.yaml".to_string()),
                "10.9.1.1".to_string(),
                2200
            ),
            (None, "10.9.1.12".to_string(), 22),
        ]
    );
    assert_eq!(
        message(&plan, "Site/r2.yaml"),
        "the host keys of its jump host bastion.yaml (10.9.1.1 port 2200) and of the device \
         (10.9.1.12 port 22) are not trusted yet, so the job will refuse this host: connecting to \
         it once in a terminal (Trust and remember) trusts them"
    );

    // The jump host trusted: r1 is clear, r2 still lacks its own.
    known_hosts
        .remember("10.9.1.1", 2200, &some_key().await)
        .unwrap();
    plan.check_host_keys(&known_hosts);
    assert_eq!(planned(&plan, "Site/r1.yaml").host_keys, None);
    assert_eq!(
        untrusted(&plan, "Site/r2.yaml"),
        [(None, "10.9.1.12".to_string(), 22)]
    );
}

/// Every kind of job is looked up the way it connects: a config backup
/// and a file job as a snippet; a script and filling in from devices only
/// for a host behind a jump host (its tunnel checks the jump host's key
/// and the device's), never for a host the script reaches by itself.
#[tokio::test]
async fn every_kind_of_job_is_warned_as_it_connects() {
    let lab = Lab::new(&[
        (
            "bastion.yaml",
            "address: 10.9.2.1
credential_profile: lab
"
            .into(),
        ),
        ("direct.yaml", at("10.9.2.10", "eos", "")),
        (
            "Behind/_folder.yaml",
            "jump_host: bastion.yaml
"
            .into(),
        ),
        ("Behind/r1.yaml", at("10.9.2.11", "eos", "")),
        (
            "_snippets.yaml",
            "Show: show hostname
"
            .into(),
        ),
        (
            "scripts/check.py",
            "print('hi')
"
            .into(),
        ),
    ]);
    let target = JobTarget::Hosts {
        ids: vec![HostId::new("direct.yaml"), HostId::new("Behind/r1.yaml")],
    };
    let local = lab.folder("files").join("image.bin");
    std::fs::write(&local, b"bits").unwrap();
    let known_hosts = KnownHosts::in_memory();
    // Nothing is trusted.
    let jump = (Some("bastion.yaml".to_string()), "10.9.2.1".to_string(), 22);
    let device = (None, "10.9.2.11".to_string(), 22);

    for step in [
        JobStep::ConfigBackup,
        JobStep::SendFile {
            file: local.display().to_string(),
            destination: "/tmp".into(),
            overwrite: false,
        },
        snippet("Show"),
    ] {
        let mut plan = lab.plan(&target, &step);
        plan.check_host_keys(&known_hosts);
        assert_eq!(plan.hosts.len(), 2, "{step:?}: {plan:?}");
        assert_eq!(
            untrusted(&plan, "direct.yaml"),
            [(None, "10.9.2.10".to_string(), 22)]
        );
        assert_eq!(
            untrusted(&plan, "Behind/r1.yaml"),
            [jump.clone(), device.clone()]
        );
        assert!(message(&plan, "direct.yaml").contains("so the job will refuse this host"));
        assert_eq!(plan.untrusted_keys(), 2, "{step:?}");
    }

    // A workspace's script reaches a direct host by itself: its library
    // checks that key, not the app.
    let mut plan = lab.plan(
        &target,
        &JobStep::Script {
            path: "scripts/check.py".into(),
        },
    );
    plan.check_host_keys(&known_hosts);
    assert_eq!(plan.hosts.len(), 2, "{plan:?}");
    assert_eq!(planned(&plan, "direct.yaml").host_keys, None);
    assert_eq!(
        untrusted(&plan, "Behind/r1.yaml"),
        [jump.clone(), device.clone()]
    );
    assert!(
        message(&plan, "Behind/r1.yaml").contains("so the script's tunnel will refuse this host"),
        "{}",
        message(&plan, "Behind/r1.yaml")
    );
    assert_eq!(plan.untrusted_keys(), 1);

    // Fill in from devices checks a direct host's key against the app's
    // and refuses an untrusted one, as for a host behind jump hosts.
    let mut plan = lab.plan(&target, &JobStep::Enrich);
    plan.check_host_keys(&known_hosts);
    assert_eq!(plan.hosts.len(), 2, "{plan:?}");
    assert_eq!(
        untrusted(&plan, "direct.yaml"),
        [(None, "10.9.2.10".to_string(), 22)]
    );
    assert_eq!(untrusted(&plan, "Behind/r1.yaml"), [jump, device]);
    for host in ["direct.yaml", "Behind/r1.yaml"] {
        assert!(
            message(&plan, host).contains("so Fill in from devices will refuse this host"),
            "{}",
            message(&plan, host)
        );
    }
    assert_eq!(plan.untrusted_keys(), 2);
}

/// A changed key cannot be told by pre-flight: the store holds a key for
/// the address, so the host is shown as any trusted one, with no warning;
/// the run then refuses it, naming the change.
#[tokio::test]
async fn a_changed_key_shows_no_warning_and_the_run_refuses_it() {
    let device = TestSshServer::start_device("changed").await;
    let lab = Lab::new(&[
        ("changed.yaml", host_at(device.port(), "frr", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let known_hosts = KnownHosts::in_memory();
    // Another server's key, as if the device had been replaced.
    known_hosts
        .remember("127.0.0.1", device.port(), &some_key().await)
        .unwrap();

    let mut plan = lab.plan(&folder(""), &snippet("Show"));
    plan.check_host_keys(&known_hosts);
    assert_eq!(planned(&plan, "changed.yaml").host_keys, None);

    let ran = run(
        &SessionManager::new(known_hosts),
        plan,
        JobOptions::default(),
        Duration::from_secs(20),
    )
    .await;
    let reason = ran.failure("changed.yaml");
    assert!(reason.contains("has changed"), "{reason}");
}

/// The warning is advice: a plan made before the key was trusted runs the
/// host once it is (the run looks the key up again), and a plan whose
/// warning still stands is refused by the run, as without it.
#[tokio::test]
async fn the_run_decides_whatever_the_plan_said() {
    let later = TestSshServer::start_device("later").await;
    let never = TestSshServer::start_device("never").await;
    let lab = Lab::new(&[
        ("later.yaml", host_at(later.port(), "frr", "")),
        ("never.yaml", host_at(never.port(), "frr", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let known_hosts = KnownHosts::in_memory();
    let mut plan = lab.plan(
        &JobTarget::Hosts {
            ids: vec![HostId::new("later.yaml"), HostId::new("never.yaml")],
        },
        &snippet("Show"),
    );
    plan.check_host_keys(&known_hosts);
    assert_eq!(plan.untrusted_keys(), 2);

    // Trusted in a terminal between the plan and the start.
    known_hosts
        .remember("127.0.0.1", later.port(), later.host_key())
        .unwrap();
    assert!(job::check(&plan, &JobOptions::default()).is_ok());
    let ran = run(
        &SessionManager::new(known_hosts),
        plan,
        JobOptions::default(),
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(ran.state("later.yaml"), &HostState::Done);
    assert!(ran.result("later.yaml").output.contains("\nlater\n"));
    let reason = ran.failure("never.yaml");
    assert!(reason.contains("not trusted yet"), "{reason}");
}

/// A device a `@cert-authority` line of the known hosts covers gets no
/// warning: its host certificate will vouch for it in the run (#26).
#[tokio::test]
async fn a_host_a_certificate_authority_covers_has_no_warning() {
    let lab = Lab::new(&[
        ("covered.yaml", at("10.9.5.1", "frr", "")),
        ("other.yaml", at("10.9.6.1", "frr", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let data = tempfile::TempDir::new().unwrap();
    std::fs::write(
        data.path().join(enodix_core::known_hosts::FILE_NAME),
        format!(
            "@cert-authority 10.9.5.* {}\n",
            some_key().await.to_openssh().unwrap()
        ),
    )
    .unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());

    let mut plan = lab.plan(&folder(""), &snippet("Show"));
    plan.check_host_keys(&known_hosts);

    assert_eq!(planned(&plan, "covered.yaml").host_keys, None);
    assert_eq!(
        untrusted(&plan, "other.yaml"),
        [(None, "10.9.6.1".to_string(), 22)]
    );
}

/// A known-hosts file in a data folder holding `text`.
fn known_hosts_file(text: &str) -> (tempfile::TempDir, KnownHosts) {
    let data = tempfile::TempDir::new().unwrap();
    std::fs::write(data.path().join(enodix_core::known_hosts::FILE_NAME), text).unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());
    (data, known_hosts)
}

/// A key trusted and revoked since fails the pre-flight with the revoked
/// wording, not with the "not trusted yet" advice.
#[tokio::test]
async fn a_remembered_key_that_is_revoked_fails_the_preflight_as_revoked() {
    let lab = Lab::new(&[
        ("r1.yaml", at("10.9.0.1", "frr", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let key = some_key().await;
    let text = key.to_openssh().unwrap();
    let (_data, known_hosts) = known_hosts_file(&format!("10.9.0.1 {text}\n@revoked * {text}\n"));

    let mut plan = lab.plan(&folder(""), &snippet("Show"));
    plan.check_host_keys(&known_hosts);

    assert_eq!(
        message(&plan, "r1.yaml"),
        "the host key of the device (10.9.0.1 port 22) is revoked in the known hosts, so the \
         job will refuse this host"
    );
    // Nothing a connection could fix.
    assert_eq!(untrusted(&plan, "r1.yaml"), []);
    assert_eq!(plan.untrusted_keys(), 1);
}

/// A revoked authority does not vouch for a host with a host certificate.
#[tokio::test]
async fn a_revoked_authority_does_not_vouch_in_the_preflight() {
    let lab = Lab::new(&[
        ("r1.yaml", at("10.9.0.1", "frr", "")),
        ("_snippets.yaml", "Show: show hostname\n".into()),
    ]);
    let authority = some_key().await.to_openssh().unwrap();

    let (_data, known_hosts) = known_hosts_file(&format!("@cert-authority * {authority}\n"));
    let mut plan = lab.plan(&folder(""), &snippet("Show"));
    plan.check_host_keys(&known_hosts);
    assert_eq!(plan.untrusted_keys(), 0, "vouched for: {plan:?}");

    let (_data, known_hosts) = known_hosts_file(&format!(
        "@cert-authority * {authority}\n@revoked * {authority}\n"
    ));
    plan.check_host_keys(&known_hosts);
    assert_eq!(plan.untrusted_keys(), 1);
    assert!(message(&plan, "r1.yaml").contains("not trusted yet"));
}
