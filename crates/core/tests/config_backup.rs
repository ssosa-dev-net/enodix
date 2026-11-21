//! Config backup: a job that fetches each device's configuration with its
//! platform definition's show-config command and keeps a history per host,
//! in a temporary data folder outside the workspace, keyed by the
//! workspace id and the host id.

mod support;

use std::fs;

use netterm_core::configs::{ConfigError, ConfigStore, LastBackup, SaveOutcome, VERSION_EXTENSION};
use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::diff::DiffLine;
use netterm_core::job::results::RunStore;
use netterm_core::job::{
    self, HostResult, HostState, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget,
    SkipReason, StartError,
};
use netterm_core::workspace::{HostId, Workspace};
use support::workspaces::files;
use support::{TestSshServer, CONFIG_TIME, PASSWORD, USERNAME, WAIT};
use tempfile::TempDir;

/// A workspace made of `files` (relative path, text), with an id.
fn workspace(temp: &TempDir, files: &[(&str, String)]) -> Workspace {
    let root = temp.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let mut workspace = Workspace::open(&root).unwrap();
    workspace.ensure_id().unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    workspace
}

/// A host for the device-mode server `server`, driven by `platform`.
fn host(server: &TestSshServer, platform: &str) -> String {
    format!(
        "address: 127.0.0.1\nport: {}\nplatform: {platform}\n",
        server.port()
    )
}

fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: USERNAME.into(),
                auth: ProfileAuth::Password {
                    password: PASSWORD.into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    store
}

fn site() -> JobTarget {
    JobTarget::Folder {
        path: "Site".into(),
    }
}

fn plan(workspace: &Workspace, store: &MemoryStore, target: &JobTarget) -> JobPlan {
    job::plan(
        workspace,
        &Profiles::new(store, workspace),
        target,
        &JobStep::ConfigBackup,
    )
    .unwrap()
}

fn id(workspace: &Workspace) -> &str {
    workspace.id().expect("the test workspace has an id")
}

fn options(configs: &ConfigStore) -> JobOptions {
    JobOptions {
        configs: Some(configs.clone()),
        ..JobOptions::default()
    }
}

/// Runs `plan` to the end, keeping configurations in `configs`; the
/// report, after checking that every `Backup` event matches it.
async fn back_up(plan: JobPlan, configs: &ConfigStore) -> JobReport {
    let mut job = job::start(&support::manager(), plan, options(configs)).unwrap();
    let mut outcomes = Vec::new();
    let report = tokio::time::timeout(WAIT * 3, async {
        loop {
            match job.next().await.expect("the job ended without a report") {
                JobEvent::Backup { host, outcome } => outcomes.push((host, outcome)),
                JobEvent::Finished(report) => return report,
                _ => {}
            }
        }
    })
    .await
    .expect("the job did not finish");
    let reported: Vec<_> = report
        .hosts
        .iter()
        .filter_map(|h| h.backup.clone().map(|b| (h.id.clone(), b)))
        .collect();
    assert_eq!(outcomes, reported);
    report
}

fn result<'a>(report: &'a JobReport, id: &str) -> &'a HostResult {
    report
        .hosts
        .iter()
        .find(|host| host.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} did not run"))
}

/// The one host's outcome, after checking it is done.
fn outcome(report: &JobReport, id: &str) -> SaveOutcome {
    let host = result(report, id);
    assert_eq!(host.state, HostState::Done, "{}", host.output);
    host.backup.clone().expect("the backup kept nothing")
}

fn r1() -> HostId {
    HostId::new("Site/r1.yaml")
}

/// A site with one device driven by EOS (whose ignored-lines pattern
/// matches the device's `! Time:` line).
fn one_device(temp: &TempDir, server: &TestSshServer) -> (Workspace, MemoryStore) {
    let workspace = workspace(
        temp,
        &[
            ("Site/_folder.yaml", "credential_profile: lab\n".into()),
            ("Site/r1.yaml", host(server, "eos")),
        ],
    );
    let store = store(&workspace);
    (workspace, store)
}

#[tokio::test]
async fn a_backup_keeps_only_the_configuration_outside_the_workspace() {
    let server = TestSshServer::start_device("r1").await;
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let before = files(workspace.root());
    let configs = ConfigStore::new(temp.path().join("data").join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;
    let SaveOutcome::Saved { version } = outcome(&report, "Site/r1.yaml") else {
        panic!("the first backup did not save");
    };

    // The output shows the command and the prompt; the version holds only
    // what the command showed.
    let output = &result(&report, "Site/r1.yaml").output;
    assert!(output.contains("r1# show running-config\n"), "{output}");
    let kept = configs.read(id(&workspace), &r1(), &version).unwrap();
    assert_eq!(kept, server.running_config(1));
    assert!(!kept.contains("r1#"), "{kept}");

    // Named by local time, under the workspace id and the host's tree
    // path; the workspace untouched.
    assert_eq!(files(workspace.root()), before);
    let stamp = chrono::Local::now().format("%Y-%m-%d_").to_string();
    assert!(version.starts_with(&stamp), "{version}");
    assert_eq!(version.len(), "YYYY-MM-DD_HH-MM-SS".len(), "{version}");
    assert_eq!(
        files(&configs.root().join(id(&workspace))),
        [
            "Site/".to_string(),
            "Site/r1/".to_string(),
            format!("Site/r1/{version}.{VERSION_EXTENSION}"),
        ]
    );
    let versions = configs.versions(id(&workspace), &r1()).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].id, version);
    assert_eq!(versions[0].lines, kept.lines().count());
    assert_eq!(versions[0].bytes, kept.len() as u64);
}

/// A device's login banner (#134), even one that looks like the start of
/// its configuration, is neither in the backup nor in the job's output.
#[tokio::test]
async fn a_login_banner_is_not_in_the_backup() {
    let server = TestSshServer::start_device("r1").await;
    server
        .quirks()
        .auth_banner("AUTHORIZED ACCESS ONLY\nr1# show running-config\nhostname evil\n");
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("data").join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;

    let SaveOutcome::Saved { version } = outcome(&report, "Site/r1.yaml") else {
        panic!("the backup did not save");
    };
    let kept = configs.read(id(&workspace), &r1(), &version).unwrap();
    assert_eq!(kept, server.running_config(1));
    let output = &result(&report, "Site/r1.yaml").output;
    assert!(!output.contains("AUTHORIZED"), "{output}");
    assert!(!output.contains("evil"), "{output}");
}

#[tokio::test]
async fn an_unchanged_device_saves_nothing_even_when_ignored_lines_differ() {
    let server = TestSshServer::start_device("r1").await;
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let first = back_up(plan(&workspace, &store, &site()), &configs).await;
    let saved = outcome(&first, "Site/r1.yaml");
    assert!(matches!(saved, SaveOutcome::Saved { .. }), "{saved:?}");

    // The device's timestamp line differs, and EOS ignores it.
    let second = back_up(plan(&workspace, &store, &site()), &configs).await;
    assert_eq!(
        outcome(&second, "Site/r1.yaml"),
        SaveOutcome::Unchanged {
            version: saved.version().to_string()
        }
    );
    let versions = configs.versions(id(&workspace), &r1()).unwrap();
    assert_eq!(versions.len(), 1);
    // The kept file keeps its ignored lines.
    let kept = configs
        .read(id(&workspace), &r1(), saved.version())
        .unwrap();
    assert!(kept.contains(&format!("{CONFIG_TIME}1\n")), "{kept}");
}

#[tokio::test]
async fn a_changed_configuration_is_a_new_version_and_the_history_diffs_any_two() {
    let server = TestSshServer::start_device("r1").await;
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let first = outcome(
        &back_up(plan(&workspace, &store, &site()), &configs).await,
        "Site/r1.yaml",
    );
    server.configure("interface Loopback9");
    let second = outcome(
        &back_up(plan(&workspace, &store, &site()), &configs).await,
        "Site/r1.yaml",
    );
    let SaveOutcome::Saved { version: second } = second else {
        panic!("a changed configuration was not saved: {second:?}");
    };
    assert_ne!(second, first.version());

    // Newest first.
    let versions: Vec<String> = configs
        .versions(id(&workspace), &r1())
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(versions, [second.clone(), first.version().to_string()]);
    assert_eq!(
        configs.read(id(&workspace), &r1(), &second).unwrap(),
        server.running_config(2)
    );

    let diff = configs
        .diff(id(&workspace), &r1(), first.version(), &second)
        .unwrap();
    let changed: Vec<&DiffLine> = diff.iter().filter(|line| line.differs()).collect();
    assert_eq!(
        changed,
        [
            &DiffLine::Removed {
                old: 2,
                text: format!("{CONFIG_TIME}1"),
            },
            &DiffLine::Added {
                new: 2,
                text: format!("{CONFIG_TIME}2"),
            },
            &DiffLine::Added {
                new: 4,
                text: "interface Loopback9".into(),
            },
        ]
    );
    // And the other way round.
    let back = configs
        .diff(id(&workspace), &r1(), &second, first.version())
        .unwrap();
    assert!(back.contains(&DiffLine::Removed {
        old: 4,
        text: "interface Loopback9".into(),
    }));
}

#[tokio::test]
async fn the_first_lines_of_a_two_line_prompt_are_not_kept() {
    // As SR Linux: a blank line and `--{ running }--[  ]--` above every
    // prompt, which the definition's ignored lines match.
    let server = TestSshServer::start_device("r1").await;
    server.set_prompt_banner("--{ running }--[  ]--");
    let temp = TempDir::new().unwrap();
    let workspace = workspace(
        &temp,
        &[
            (
                "_platforms.yaml",
                "banner:\n  prompt: '[\\w.-]+#'\n  show_config: show running-config\n  ignored_lines: '^(--\\{.*\\}--|! Time:)'\n"
                    .into(),
            ),
            ("Site/_folder.yaml", "credential_profile: lab\n".into()),
            ("Site/r1.yaml", host(&server, "banner")),
        ],
    );
    let store = store(&workspace);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;
    let saved = outcome(&report, "Site/r1.yaml");
    let kept = configs
        .read(id(&workspace), &r1(), saved.version())
        .unwrap();
    assert_eq!(kept, server.running_config(1));
}

/// An enable that comes back to the unprivileged prompt saying "% No
/// password set" fails the backup, which keeps nothing: the device's error
/// for `show running-config` never becomes a version.
#[tokio::test]
async fn a_backup_whose_privileged_mode_is_not_reached_keeps_nothing() {
    let server = TestSshServer::start_unprivileged_device("r1").await;
    server.set_enable(support::EnableMode::NotSet);
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;
    let host = result(&report, "Site/r1.yaml");
    let HostState::Failed { reason } = &host.state else {
        panic!("the backup did not fail: {:?}\n{}", host.state, host.output);
    };
    assert!(
        reason.contains("did not reach privileged mode")
            && reason.contains(support::NO_ENABLE_PASSWORD),
        "{reason}"
    );
    assert_eq!(host.backup, None);
    assert!(
        !host.output.contains("show running-config"),
        "{}",
        host.output
    );
    assert!(configs.versions(id(&workspace), &r1()).unwrap().is_empty());
}

/// A definition with no privilege command on a login that is not
/// privileged: what `show running-config` shows is only the device's
/// error (a `^` marker and a `%` line), which is not kept.
#[tokio::test]
async fn a_backup_that_shows_only_an_error_keeps_nothing() {
    let server = TestSshServer::start_unprivileged_device("r1").await;
    let temp = TempDir::new().unwrap();
    let workspace = workspace(
        &temp,
        &[
            (
                "_platforms.yaml",
                "plain:\n  prompt: '[\\w.-]+[>#]'\n  show_config: show running-config\n".into(),
            ),
            ("Site/_folder.yaml", "credential_profile: lab\n".into()),
            ("Site/r1.yaml", host(&server, "plain")),
        ],
    );
    let store = store(&workspace);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;
    let host = result(&report, "Site/r1.yaml");
    let HostState::Failed { reason } = &host.state else {
        panic!("the backup did not fail: {:?}\n{}", host.state, host.output);
    };
    assert_eq!(
        reason,
        &format!(
            "\"show running-config\" showed only an error, not a configuration: {:?}",
            support::INVALID_INPUT
        )
    );
    assert!(configs.versions(id(&workspace), &r1()).unwrap().is_empty());
}

/// A configuration that takes longer than the command timeout to show,
/// but never pauses for that long, is waited for and kept whole.
#[tokio::test]
async fn a_configuration_shown_slowly_is_kept_whole() {
    let server = TestSshServer::start_device("r1").await;
    for n in 1..=4 {
        server.configure(&format!("interface Loopback{n}"));
    }
    // Eight lines, 400 ms apart: 3.2 s in all, never 2 s quiet.
    server.set_config_pace(std::time::Duration::from_millis(400));
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let options = JobOptions {
        command_timeout: std::time::Duration::from_secs(2),
        ..options(&configs)
    };
    let mut job = job::start(
        &support::manager(),
        plan(&workspace, &store, &site()),
        options,
    )
    .unwrap();
    let report = tokio::time::timeout(WAIT * 3, async {
        loop {
            if let JobEvent::Finished(report) = job.next().await.expect("no report") {
                return report;
            }
        }
    })
    .await
    .expect("the job did not finish");
    let saved = outcome(&report, "Site/r1.yaml");
    assert_eq!(
        configs
            .read(id(&workspace), &r1(), saved.version())
            .unwrap(),
        server.running_config(1)
    );
}

/// A configured line that ends like a prompt (a banner's text, `core-7#`),
/// and the device pausing right after it for longer than a prompt must stay
/// quiet: the configuration is kept whole, not cut there (#136).
#[tokio::test]
async fn a_configuration_line_like_a_prompt_and_a_pause_after_it_are_kept_whole() {
    let server = TestSshServer::start_device("r1").await;
    for line in ["banner motd ^C", "core-7#", "^C", "interface Loopback1"] {
        server.configure(line);
    }
    server.set_config_stall("core-7#", std::time::Duration::from_millis(1500));
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;
    let saved = outcome(&report, "Site/r1.yaml");
    assert_eq!(
        configs
            .read(id(&workspace), &r1(), saved.version())
            .unwrap(),
        server.running_config(1)
    );
}

/// A long configuration that pauses half way, after a line ending like a
/// prompt (a description), is kept whole (#136).
#[tokio::test]
async fn a_pause_in_the_middle_of_a_long_configuration_keeps_it_whole() {
    let server = TestSshServer::start_device("r1").await;
    for n in 1..=400 {
        server.configure(&format!("interface Ethernet{n}"));
        if n == 200 {
            server.configure(" description uplink to core-2#");
        }
    }
    server.set_config_stall(
        " description uplink to core-2#",
        std::time::Duration::from_millis(1500),
    );
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));

    let report = back_up(plan(&workspace, &store, &site()), &configs).await;
    let saved = outcome(&report, "Site/r1.yaml");
    let kept = configs
        .read(id(&workspace), &r1(), saved.version())
        .unwrap();
    assert_eq!(kept, server.running_config(1));
    assert!(kept.ends_with("interface Ethernet400\nend\n"), "{kept}");
}

/// The show-config command never changes the prompt: a configuration that
/// seems to stop at another prompt-like line is not kept, and the reason
/// says why (#136).
#[tokio::test]
async fn a_configuration_ending_at_another_prompt_is_not_kept() {
    let server = TestSshServer::start_device("r1").await;
    server.configure("core-7#");
    // Longer than the command timeout: the device seems to have stopped.
    server.set_config_stall("core-7#", std::time::Duration::from_secs(5));
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));
    let options = JobOptions {
        command_timeout: std::time::Duration::from_secs(2),
        ..options(&configs)
    };
    let mut job = job::start(
        &support::manager(),
        plan(&workspace, &store, &site()),
        options,
    )
    .unwrap();
    let report = tokio::time::timeout(WAIT * 3, async {
        loop {
            if let JobEvent::Finished(report) = job.next().await.expect("no report") {
                return report;
            }
        }
    })
    .await
    .expect("the job did not finish");
    let host = result(&report, "Site/r1.yaml");
    let HostState::Failed { reason } = &host.state else {
        panic!("the backup did not fail: {:?}\n{}", host.state, host.output);
    };
    assert_eq!(
        reason,
        "no prompt after \"show running-config\": the device showed nothing more for 2 s, and its last line \"core-7#\" is not the prompt \"show running-config\" was typed at (r1#); nothing was backed up"
    );
    assert_eq!(host.backup, None);
    assert!(configs.versions(id(&workspace), &r1()).unwrap().is_empty());
}

#[tokio::test]
async fn hosts_whose_definition_has_no_show_config_command_are_skipped() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(
        &temp,
        &[
            (
                "_platforms.yaml",
                "quiet:\n  prompt: '[\\w.-]+#'\n  disable_paging: terminal length 0\n".into(),
            ),
            ("Site/_folder.yaml", "credential_profile: lab\n".into()),
            ("Site/r1.yaml", "address: 127.0.0.1\nplatform: eos\n".into()),
            (
                "Site/q1.yaml",
                "address: 127.0.0.1\nplatform: quiet\n".into(),
            ),
            ("Site/none.yaml", "address: 127.0.0.1\n".into()),
        ],
    );
    let store = store(&workspace);
    let planned = plan(&workspace, &store, &site());
    let runs: Vec<&str> = planned.hosts.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(runs, ["Site/r1.yaml"]);
    let skipped: Vec<(&str, &SkipReason)> = planned
        .skipped
        .iter()
        .map(|h| (h.id.as_str(), &h.reason))
        .collect();
    assert_eq!(
        skipped,
        [
            ("Site/none.yaml", &SkipReason::NoPlatform { platform: None }),
            (
                "Site/q1.yaml",
                &SkipReason::NoShowConfig {
                    platform: "quiet".into()
                }
            ),
        ]
    );
    let message = &planned.skipped[1].message;
    assert!(message.contains("no show-config command"), "{message}");
}

#[tokio::test]
async fn a_backup_cannot_start_without_a_store_or_a_workspace_id() {
    let temp = TempDir::new().unwrap();
    let files = [
        ("Site/_folder.yaml", "credential_profile: lab\n".to_string()),
        ("Site/r1.yaml", "address: 127.0.0.1\nplatform: eos\n".into()),
    ];
    let with_id = workspace(&temp, &files);
    let store_with_id = store(&with_id);
    let planned = plan(&with_id, &store_with_id, &site());
    assert!(matches!(
        job::start(&support::manager(), planned.clone(), JobOptions::default()),
        Err(StartError::NoConfigStore)
    ));

    // The same folder, opened without its id.
    fs::remove_file(with_id.root().join("_workspace.yaml")).unwrap();
    let without = Workspace::open(with_id.root()).unwrap();
    assert_eq!(without.id(), None);
    let planned = plan(&without, &store(&without), &site());
    let configs = ConfigStore::new(temp.path().join("configs"));
    assert!(matches!(
        job::check(&planned, &options(&configs)),
        Err(StartError::NoWorkspaceId)
    ));
}

#[tokio::test]
async fn a_kept_run_records_what_each_host_backed_up() {
    let server = TestSshServer::start_device("r1").await;
    let temp = TempDir::new().unwrap();
    let (workspace, store) = one_device(&temp, &server);
    let configs = ConfigStore::new(temp.path().join("configs"));
    let runs = RunStore::new(temp.path().join("jobs"));
    let planned = plan(&workspace, &store, &site());
    let mut recorder = runs.begin(id(&workspace), &site(), &planned).unwrap();
    let run = recorder.id().to_string();

    let mut job = job::start(&support::manager(), planned, options(&configs)).unwrap();
    tokio::time::timeout(WAIT * 3, async {
        loop {
            let event = job.next().await.expect("the job ended without a report");
            recorder.record(&event).unwrap();
            if let JobEvent::Finished(_) = event {
                return;
            }
        }
    })
    .await
    .expect("the job did not finish");

    let kept = runs.run(id(&workspace), &run).unwrap();
    assert_eq!(kept.step, JobStep::ConfigBackup);
    let host = kept.host(&r1()).unwrap();
    assert_eq!(host.state, HostState::Done);
    let version = configs.versions(id(&workspace), &r1()).unwrap()[0]
        .id
        .clone();
    assert_eq!(host.backup, Some(SaveOutcome::Saved { version }));
}

#[test]
fn the_store_compares_without_ignored_lines_and_keeps_every_version() {
    let temp = TempDir::new().unwrap();
    let configs = ConfigStore::new(temp.path());
    let host = HostId::new("r1.yaml");
    let config = |time: &str, extra: &str| format!("! Time: {time}\nhostname r1\n{extra}end\n");

    let first = configs
        .save("ws", &host, &config("10:00", ""), Some("^! Time:"))
        .unwrap();
    let SaveOutcome::Saved { version: first } = first else {
        panic!("the first version was not saved: {first:?}");
    };
    // Only an ignored line differs.
    assert_eq!(
        configs
            .save("ws", &host, &config("11:00", ""), Some("^! Time:"))
            .unwrap(),
        SaveOutcome::Unchanged {
            version: first.clone()
        }
    );
    // Without the pattern, the timestamp counts; in the same second, the
    // new version is `_2`.
    let second = configs
        .save("ws", &host, &config("11:00", ""), None)
        .unwrap();
    let SaveOutcome::Saved { version: second } = second else {
        panic!("a changed text was not saved: {second:?}");
    };
    assert_ne!(second, first);
    // A blank pattern ignores nothing; CRLF breaks compare as LF ones.
    assert_eq!(
        configs
            .save(
                "ws",
                &host,
                "! Time: 11:00\r\nhostname r1\r\nend\r\n",
                Some(" ")
            )
            .unwrap(),
        SaveOutcome::Unchanged {
            version: second.clone()
        }
    );
    let third = configs
        .save(
            "ws",
            &host,
            &config("12:00", "ntp server 10.0.0.1\n"),
            Some("^! Time:"),
        )
        .unwrap();
    let ids: Vec<String> = configs
        .versions("ws", &host)
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, [third.version(), &second, &first]);
    assert_eq!(
        configs.read("ws", &host, &first).unwrap(),
        config("10:00", "")
    );

    // Saved to a folder as `<host>_<version>.txt`, `_2` when taken.
    let to = TempDir::new().unwrap();
    let copy = configs.export("ws", &host, &first, to.path()).unwrap();
    assert_eq!(copy, to.path().join(format!("r1_{first}.txt")));
    assert_eq!(fs::read_to_string(&copy).unwrap(), config("10:00", ""));
    let again = configs.export("ws", &host, &first, to.path()).unwrap();
    assert_eq!(again, to.path().join(format!("r1_{first}_2.txt")));

    // Each host and each workspace has its own history.
    assert!(configs
        .versions("ws", &HostId::new("Other/r1.yaml"))
        .unwrap()
        .is_empty());
    assert!(configs.versions("other", &host).unwrap().is_empty());
}

#[test]
fn the_store_refuses_what_cannot_name_its_files() {
    let temp = TempDir::new().unwrap();
    let configs = ConfigStore::new(temp.path());
    let host = HostId::new("r1.yaml");
    assert!(matches!(
        configs.save("../up", &host, "x\n", None),
        Err(ConfigError::BadWorkspaceId(_))
    ));
    assert!(matches!(
        configs.save("ws", &HostId::new("../r1.yaml"), "x\n", None),
        Err(ConfigError::BadHostId(_))
    ));
    assert!(matches!(
        configs.save("ws", &host, "x\n", Some("(")),
        Err(ConfigError::Pattern(_))
    ));
    assert!(matches!(
        configs.read("ws", &host, "2026-01-01_00-00-00"),
        Err(ConfigError::NoSuchVersion { .. })
    ));
    let saved = configs.save("ws", &host, "x\n", None).unwrap();
    assert!(matches!(
        configs.read("ws", &host, "../r1"),
        Err(ConfigError::NoSuchVersion { .. })
    ));
    assert!(matches!(
        configs.export("ws", &host, saved.version(), &temp.path().join("missing")),
        Err(ConfigError::NotAFolder(_))
    ));
    // Nothing but the version was written.
    assert_eq!(
        files(temp.path()),
        [
            "ws/".to_string(),
            "ws/r1/".to_string(),
            format!("ws/r1/{}.txt", saved.version()),
        ]
    );
}

#[test]
fn the_store_lists_the_hosts_backed_up_with_what_their_last_backup_did() {
    let temp = TempDir::new().unwrap();
    let configs = ConfigStore::new(temp.path());
    let r1 = HostId::new("Site/r1.yaml");
    let r2 = HostId::new("r2.yaml");
    let r3 = HostId::new("r3.yaml");
    let never = HostId::new("never.yaml");

    // r1: two versions, the last backup changed something.
    let first = configs.save("ws", &r1, "hostname r1\n", None).unwrap();
    let changed = configs
        .save("ws", &r1, "hostname r1\nntp server 10.0.0.1\n", None)
        .unwrap();
    // r2: one version, then a backup that found it unchanged.
    let only = configs.save("ws", &r2, "hostname r2\n", None).unwrap();
    assert!(matches!(
        configs.save("ws", &r2, "hostname r2\n", None).unwrap(),
        SaveOutcome::Unchanged { .. }
    ));
    // r3: its first backup.
    let r3_first = configs.save("ws", &r3, "hostname r3\n", None).unwrap();
    // A host of another workspace is not listed.
    configs.save("other", &never, "x\n", None).unwrap();

    let listed = configs
        .backed_up("ws", &[never.clone(), r3.clone(), r1.clone(), r2.clone()])
        .unwrap();
    // In the order asked, hosts without versions left out.
    let hosts: Vec<&HostId> = listed.iter().map(|h| &h.host).collect();
    assert_eq!(hosts, [&r3, &r1, &r2]);

    assert_eq!(listed[0].versions, 1);
    assert_eq!(listed[0].newest, r3_first.version());
    assert_eq!(listed[0].last, LastBackup::First);

    assert_eq!(listed[1].versions, 2);
    assert_eq!(listed[1].newest, changed.version());
    assert_ne!(listed[1].newest, first.version());
    assert_eq!(listed[1].last, LastBackup::Changed);
    // When it was last backed up: its newest version's time.
    assert_eq!(listed[1].last_backup, changed.version()[..19]);

    assert_eq!(listed[2].versions, 1);
    assert_eq!(listed[2].newest, only.version());
    assert_eq!(listed[2].last, LastBackup::Unchanged);
    assert!(listed[2].last_backup.as_str() >= &only.version()[..19]);

    // A later change replaces the unchanged mark; the note is no version.
    let again = configs.save("ws", &r2, "hostname r2b\n", None).unwrap();
    let r2_now = &configs.backed_up("ws", std::slice::from_ref(&r2)).unwrap()[0];
    assert_eq!(r2_now.last, LastBackup::Changed);
    assert_eq!(r2_now.newest, again.version());
    assert_eq!(configs.versions("ws", &r2).unwrap().len(), 2);

    // A host id that cannot name a folder is refused, as everywhere.
    assert!(matches!(
        configs.backed_up("ws", &[HostId::new("../r1.yaml")]),
        Err(ConfigError::BadHostId(_))
    ));
}
