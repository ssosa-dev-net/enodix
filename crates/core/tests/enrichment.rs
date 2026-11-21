//! Host enrichment: vendor, model, serial number and OS version filled in
//! from what the devices report. Pre-flight, the preview and applying it
//! run from given facts, with no device; one job runs a stand-in
//! interpreter (`support::python::FakePython`) that hands back a prepared
//! facts file, as the shipped script would.

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::enrich::{
    self, napalm_driver, ChangeKind, CollectedFacts, CollectedHost, DeviceFacts, EnrichError,
    EnrichField, EnrichPreview, PreviewOutcome, ENRICH_TITLE, FACTS_VARIABLE, OS_VERSION_FIELD,
};
use netterm_core::job::results::{RunSkipped, RunStore};
use netterm_core::job::script::ScriptTargets;
use netterm_core::job::{
    self, HostState, Job, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget,
    ScriptOptions, SkipReason, StartError, SCRIPT_OUTPUT,
};
use netterm_core::platform::BUILT_IN_PLATFORMS;
use netterm_core::workspace::{EditError, HostId, SavedJob, Workspace};
use support::python::FakePython;
use tempfile::TempDir;

/// `Site` holds hosts of every kind (by name: f1 frr, n1 nxos, r1 eos with
/// nothing filled in, r2 arista_eos with typed values (production), r3
/// cisco_ios with what its device reports, t1 telnet, x1 no platform), all
/// with the folder's profile `lab`; `Other` a host with no profile, one
/// behind a jump host (tunnelled), one behind a jump host with no profile
/// and one behind a jump host the workspace lacks.
fn workspace(temp: &TempDir) -> Workspace {
    let root = temp.path().join("workspace");
    let files = [
        ("bastion.yaml", "address: 10.9.9.9\ncredential_profile: lab\n"),
        ("open-bastion.yaml", "address: 10.9.9.8\n"),
        ("Site/_folder.yaml", "credential_profile: lab\n"),
        ("Site/f1.yaml", "address: 10.0.0.6\nplatform: frr\n"),
        ("Site/n1.yaml", "address: 10.0.0.7\nplatform: nxos\n"),
        ("Site/r1.yaml", "address: 10.0.0.1\nplatform: eos\n"),
        (
            "Site/r2.yaml",
            "# the core switch\naddress: 10.0.0.2\nvendor: Arista\nmodel: DCS-old\nplatform: arista_eos\nenvironment: production\nowner: noc\ncustom:\n  os_version: 4.20.1F\n  rack_note: by the door\n",
        ),
        (
            "Site/r3.yaml",
            "address: 10.0.0.3\nvendor: Cisco\nmodel: CSR1000V\nserial_number: 9ABCDEF\nplatform: cisco_ios\ncustom:\n  os_version: 17.3.1a\n",
        ),
        (
            "Site/t1.yaml",
            "address: 10.0.0.4\nplatform: ios\nprotocol: telnet\n",
        ),
        ("Site/x1.yaml", "address: 10.0.0.5\n"),
        ("Other/np.yaml", "address: 10.1.0.1\nplatform: eos\n"),
        (
            "Other/j1.yaml",
            "address: 10.1.0.2\nplatform: eos\ncredential_profile: lab\njump_host: bastion.yaml\n",
        ),
        (
            "Other/j2.yaml",
            "address: 10.1.0.3\nplatform: eos\ncredential_profile: lab\njump_host: open-bastion.yaml\n",
        ),
        (
            "Other/j3.yaml",
            "address: 10.1.0.4\nplatform: eos\ncredential_profile: lab\njump_host: gone.yaml\n",
        ),
    ];
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

fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: "admin".into(),
                auth: ProfileAuth::Password {
                    password: "secret".into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    store
}

fn plan(workspace: &Workspace, target: &JobTarget) -> JobPlan {
    let store = store(workspace);
    job::plan(
        workspace,
        &Profiles::new(&store, workspace),
        target,
        &JobStep::Enrich,
    )
    .unwrap()
}

fn site() -> JobTarget {
    JobTarget::Folder {
        path: "Site".into(),
    }
}

fn id(text: &str) -> HostId {
    HostId::new(text)
}

fn facts(vendor: &str, model: &str, serial: &str, os: &str) -> DeviceFacts {
    let given = |text: &str| (!text.is_empty()).then(|| text.to_string());
    DeviceFacts {
        vendor: given(vendor),
        model: given(model),
        serial_number: given(serial),
        os_version: given(os),
    }
}

fn reported(host: &str, facts: DeviceFacts) -> CollectedHost {
    CollectedHost {
        id: id(host),
        facts: Some(facts),
        error: None,
    }
}

fn failed(host: &str, error: &str) -> CollectedHost {
    CollectedHost {
        id: id(host),
        facts: None,
        error: Some(error.into()),
    }
}

/// What the devices of `Site` report: r1 and r2 answer (r2 with other
/// values than typed), r3 what it has, n1 could not be reached.
fn site_facts() -> CollectedFacts {
    CollectedFacts {
        hosts: vec![
            failed("Site/n1.yaml", "the device could not be reached: timed out"),
            reported(
                "Site/r1.yaml",
                facts("Arista", "cEOSLab", "SN-R1", "4.36.0.1F"),
            ),
            reported(
                "Site/r2.yaml",
                facts("Arista", "cEOSLab", "SN-R2", "4.36.0.1F"),
            ),
            reported(
                "Site/r3.yaml",
                facts("Cisco", " CSR1000V ", "9ABCDEF", "17.3.1a"),
            ),
        ],
    }
}

fn skipped(plan: &JobPlan) -> Vec<RunSkipped> {
    plan.skipped
        .iter()
        .map(|host| RunSkipped {
            id: host.id.clone(),
            name: host.name.clone(),
            reason: host.message.clone(),
        })
        .collect()
}

fn outcome<'a>(preview: &'a EnrichPreview, host: &str) -> &'a PreviewOutcome {
    &preview
        .hosts
        .iter()
        .find(|h| h.id.as_str() == host)
        .unwrap_or_else(|| panic!("{host} in the preview"))
        .outcome
}

#[test]
fn the_napalm_driver_comes_from_the_platform_definition() {
    let driver = |name: &str| {
        let definition = BUILT_IN_PLATFORMS
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .definition();
        napalm_driver(&definition)
    };
    assert_eq!(driver("eos"), Some("eos"));
    assert_eq!(driver("ios"), Some("ios"));
    assert_eq!(driver("junos"), Some("junos"));
    // NX-OS answers to both; the SSH one needs no NX-API.
    assert_eq!(driver("nxos"), Some("nxos_ssh"));
    assert_eq!(driver("srlinux"), None);
    assert_eq!(driver("frr"), None);
}

#[test]
fn pre_flight_keeps_the_hosts_napalm_can_reach_with_their_own_profile() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &site());
    let ran: Vec<(&str, &str)> = plan
        .hosts
        .iter()
        .map(|host| (host.id.as_str(), host.platform.as_str()))
        .collect();
    assert_eq!(
        ran,
        [
            ("Site/n1.yaml", "nxos"),
            ("Site/r1.yaml", "eos"),
            ("Site/r2.yaml", "eos"),
            ("Site/r3.yaml", "ios"),
        ]
    );
    assert_eq!(plan.production, [id("Site/r2.yaml")]);
    let reasons: Vec<(&str, &SkipReason)> = plan
        .skipped
        .iter()
        .map(|host| (host.id.as_str(), &host.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            (
                "Site/f1.yaml",
                &SkipReason::NoNapalmDriver {
                    platform: "frr".into()
                }
            ),
            (
                "Site/t1.yaml",
                &SkipReason::NotSsh {
                    protocol: netterm_core::workspace::Protocol::Telnet
                }
            ),
            ("Site/x1.yaml", &SkipReason::NoPlatform { platform: None }),
        ]
    );
    assert!(plan.skipped[0].message.contains("no NAPALM driver"));
    assert!(plan.ready());

    let other = self::plan(
        &workspace,
        &JobTarget::Folder {
            path: "Other".into(),
        },
    );
    // Behind a jump host with a stored profile: the script's tunnel
    // reaches it.
    let ran: Vec<&str> = other.hosts.iter().map(|host| host.id.as_str()).collect();
    assert_eq!(ran, ["Other/j1.yaml"]);
    let reasons: Vec<(&str, &SkipReason)> = other
        .skipped
        .iter()
        .map(|host| (host.id.as_str(), &host.reason))
        .collect();
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert_eq!(
        reasons[0],
        (
            "Other/j2.yaml",
            &SkipReason::BehindJumpHost {
                jump_host: "open-bastion.yaml".into()
            }
        )
    );
    assert!(other.skipped[0].message.contains("cannot log in to it"));
    assert_eq!(reasons[1].0, "Other/j3.yaml");
    assert!(
        matches!(reasons[1].1, SkipReason::JumpChain { error } if error.contains("gone.yaml")),
        "{reasons:?}"
    );
    assert_eq!(reasons[2], ("Other/np.yaml", &SkipReason::NoProfile));
}

#[test]
fn the_preview_shows_what_is_new_and_what_would_replace_a_typed_value() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &site());
    let before = fs::read_to_string(workspace.root().join("Site/r2.yaml")).unwrap();
    let preview = enrich::preview(&workspace, &site_facts(), &skipped(&plan));

    let order: Vec<&str> = preview.hosts.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        order,
        [
            "Site/n1.yaml",
            "Site/r1.yaml",
            "Site/r2.yaml",
            "Site/r3.yaml",
            "Site/f1.yaml",
            "Site/t1.yaml",
            "Site/x1.yaml",
        ]
    );

    let r1 = &preview.hosts[1];
    assert_eq!(r1.outcome, PreviewOutcome::Changes);
    assert!(!r1.replaces());
    let new: Vec<(EnrichField, ChangeKind, Option<&str>, &str)> = r1
        .changes
        .iter()
        .map(|c| (c.field, c.kind, c.current.as_deref(), c.value.as_str()))
        .collect();
    assert_eq!(
        new,
        [
            (EnrichField::Vendor, ChangeKind::New, None, "Arista"),
            (EnrichField::Model, ChangeKind::New, None, "cEOSLab"),
            (EnrichField::SerialNumber, ChangeKind::New, None, "SN-R1"),
            (EnrichField::OsVersion, ChangeKind::New, None, "4.36.0.1F"),
        ]
    );

    // The vendor is the same and left out; the typed model and OS version
    // would be replaced; the serial number is new.
    let r2 = &preview.hosts[2];
    assert_eq!(r2.outcome, PreviewOutcome::Changes);
    assert!(r2.replaces());
    let changes: Vec<(EnrichField, ChangeKind, Option<&str>, &str)> = r2
        .changes
        .iter()
        .map(|c| (c.field, c.kind, c.current.as_deref(), c.value.as_str()))
        .collect();
    assert_eq!(
        changes,
        [
            (
                EnrichField::Model,
                ChangeKind::Replace,
                Some("DCS-old"),
                "cEOSLab"
            ),
            (EnrichField::SerialNumber, ChangeKind::New, None, "SN-R2"),
            (
                EnrichField::OsVersion,
                ChangeKind::Replace,
                Some("4.20.1F"),
                "4.36.0.1F"
            ),
        ]
    );

    // Reported values are trimmed before comparing.
    assert_eq!(preview.hosts[3].outcome, PreviewOutcome::Unchanged);
    assert!(preview.hosts[3].changes.is_empty());
    assert_eq!(
        preview.hosts[0].outcome,
        PreviewOutcome::Failed {
            reason: "the device could not be reached: timed out".into()
        }
    );
    match outcome(&preview, "Site/f1.yaml") {
        PreviewOutcome::Skipped { reason } => assert!(reason.contains("NAPALM"), "{reason}"),
        other => panic!("{other:?}"),
    }

    // Nothing written.
    assert_eq!(
        fs::read_to_string(workspace.root().join("Site/r2.yaml")).unwrap(),
        before
    );
}

#[test]
fn applying_fills_empty_fields_and_replaces_typed_values_only_on_ticked_hosts() {
    let temp = TempDir::new().unwrap();
    let mut workspace = workspace(&temp);
    let plan = plan(&workspace, &site());
    let preview = enrich::preview(&workspace, &site_facts(), &skipped(&plan));
    let r3_before = fs::read_to_string(workspace.root().join("Site/r3.yaml")).unwrap();

    // r2 not ticked: its serial number is filled, its typed model and OS
    // version kept.
    let report = enrich::apply(&mut workspace, &preview, &[]);
    let names = |list: &[enrich::ReportedHost]| -> Vec<String> {
        list.iter().map(|h| h.id.to_string()).collect()
    };
    assert_eq!(names(&report.filled), ["Site/r1.yaml", "Site/r2.yaml"]);
    assert_eq!(names(&report.unchanged), ["Site/r3.yaml"]);
    assert_eq!(
        names(&report.skipped),
        ["Site/f1.yaml", "Site/t1.yaml", "Site/x1.yaml"]
    );
    assert_eq!(names(&report.failed), ["Site/n1.yaml"]);
    assert!(report.failed[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("timed out"));
    assert_eq!(report.reload_error, None);
    assert_eq!(report.filled[1].fields, [EnrichField::SerialNumber]);
    assert_eq!(
        report.filled[1].kept,
        [EnrichField::Model, EnrichField::OsVersion]
    );

    let r1 = workspace.host(&id("Site/r1.yaml")).unwrap();
    assert_eq!(r1.vendor.as_deref(), Some("Arista"));
    assert_eq!(r1.model.as_deref(), Some("cEOSLab"));
    assert_eq!(r1.serial_number.as_deref(), Some("SN-R1"));
    assert_eq!(
        r1.custom
            .iter()
            .find(|f| f.key == OS_VERSION_FIELD)
            .map(|f| f.value.as_str()),
        Some("4.36.0.1F")
    );
    let r2 = workspace.host(&id("Site/r2.yaml")).unwrap();
    assert_eq!(r2.model.as_deref(), Some("DCS-old"));
    assert_eq!(r2.serial_number.as_deref(), Some("SN-R2"));
    // r3 reported what it has: its file (comments and all) is untouched.
    assert_eq!(
        fs::read_to_string(workspace.root().join("Site/r3.yaml")).unwrap(),
        r3_before
    );

    // Ticked now: the typed values are replaced; other keys and custom
    // fields stay, in their order.
    let preview = enrich::preview(&workspace, &site_facts(), &[]);
    assert_eq!(
        outcome(&preview, "Site/r1.yaml"),
        &PreviewOutcome::Unchanged
    );
    let report = enrich::apply(&mut workspace, &preview, &[id("Site/r2.yaml")]);
    assert_eq!(names(&report.filled), ["Site/r2.yaml"]);
    assert_eq!(
        report.filled[0].fields,
        [EnrichField::Model, EnrichField::OsVersion]
    );
    assert!(report.filled[0].kept.is_empty());
    assert_eq!(names(&report.unchanged), ["Site/r1.yaml", "Site/r3.yaml"]);
    let text = fs::read_to_string(workspace.root().join("Site/r2.yaml")).unwrap();
    assert_eq!(
        text,
        "address: 10.0.0.2\nvendor: Arista\nmodel: cEOSLab\nserial_number: SN-R2\nplatform: arista_eos\nenvironment: production\nowner: noc\ncustom:\n  os_version: 4.36.0.1F\n  rack_note: by the door\n"
    );
}

#[test]
fn a_value_typed_after_the_preview_is_kept_unless_the_host_is_ticked() {
    let temp = TempDir::new().unwrap();
    let mut workspace = workspace(&temp);
    let preview = enrich::preview(&workspace, &site_facts(), &[]);
    // Someone types r1's model meanwhile, and clears r3's (which the
    // preview showed as unchanged).
    let mut r1 = workspace.host(&id("Site/r1.yaml")).unwrap().clone();
    r1.model = Some("typed by hand".into());
    workspace.write_host(&r1).unwrap();
    let mut r3 = workspace.host(&id("Site/r3.yaml")).unwrap().clone();
    r3.model = None;
    workspace.write_host(&r3).unwrap();

    let report = enrich::apply(&mut workspace, &preview, &[]);
    // Only what the preview showed is written.
    assert_eq!(
        workspace.host(&id("Site/r3.yaml")).unwrap().model,
        None,
        "a field the preview did not show is not filled"
    );
    let r1 = &report.filled[0];
    assert_eq!(r1.id.as_str(), "Site/r1.yaml");
    assert_eq!(r1.kept, [EnrichField::Model]);
    assert_eq!(
        workspace
            .host(&id("Site/r1.yaml"))
            .unwrap()
            .model
            .as_deref(),
        Some("typed by hand")
    );

    // A host deleted since the preview fails, naming why.
    workspace.delete_host(&id("Site/r2.yaml")).unwrap();
    let report = enrich::apply(&mut workspace, &preview, &[id("Site/r2.yaml")]);
    let gone = report
        .failed
        .iter()
        .find(|h| h.id.as_str() == "Site/r2.yaml")
        .unwrap();
    assert_eq!(
        gone.reason.as_deref(),
        Some("it is no longer in the workspace")
    );
}

#[test]
fn the_facts_file_is_read_for_the_jobs_hosts_only() {
    let hosts = [id("a.yaml"), id("b.yaml"), id("c.yaml")];
    let read = CollectedFacts::parse(
        r#"{"format": 1, "hosts": [
            {"id": "b.yaml", "error": "login refused"},
            {"id": "a.yaml", "facts": {"vendor": "Arista", "model": "", "extra": 1}},
            {"id": "zzz.yaml", "facts": {"vendor": "not a target"}}
        ]}"#,
        &hosts,
        "nothing reported",
    );
    assert_eq!(
        read.hosts,
        [
            reported("a.yaml", facts("Arista", "", "", "")),
            failed("b.yaml", "login refused"),
            failed("c.yaml", "nothing reported"),
        ]
    );
    // Blank values count as not reported.
    assert_eq!(
        read.hosts[0]
            .facts
            .as_ref()
            .unwrap()
            .get(EnrichField::Model),
        None
    );

    let wrong = CollectedFacts::parse(r#"{"format": 2, "hosts": []}"#, &hosts, "-");
    assert!(wrong
        .hosts
        .iter()
        .all(|h| h.error.as_deref().unwrap().contains("format 2")));
    let damaged = CollectedFacts::parse("not json", &hosts, "-");
    assert!(damaged.hosts[0]
        .error
        .as_deref()
        .unwrap()
        .contains("damaged"));
    let temp = TempDir::new().unwrap();
    let missing = CollectedFacts::read(&temp.path().join("none.json"), &hosts, "no file");
    assert!(missing
        .hosts
        .iter()
        .all(|h| h.error.as_deref() == Some("no file")));
}

/// Every event until the end, with the report.
async fn finish(job: &mut Job) -> (Vec<JobEvent>, JobReport) {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), job.next())
            .await
            .expect("the job ends")
            .expect("the job reports its end");
        if let JobEvent::Finished(report) = event {
            return (events, report);
        }
        events.push(event);
    }
}

/// A stand-in for the shipped script: prints its arguments and targets,
/// then copies `NETTERM_TEST_FACTS` to the facts file.
fn stand_in(folder: &Path) -> FakePython {
    FakePython::running(
        folder,
        "enrich",
        "echo args=%1 %2 %3\r\ntype \"%NETTERM_JOB_TARGETS%\"\r\necho.\r\necho facts=%NETTERM_FACTS_FILE%\r\ncopy /y \"%NETTERM_TEST_FACTS%\" \"%NETTERM_FACTS_FILE%\" >nul",
        "echo \"args=$1 $2 $3\"\ncat \"$NETTERM_JOB_TARGETS\"\necho\necho \"facts=$NETTERM_FACTS_FILE\"\ncp \"$NETTERM_TEST_FACTS\" \"$NETTERM_FACTS_FILE\"",
    )
}

fn options(interpreter: &Path, facts: &Path) -> JobOptions {
    JobOptions {
        script: Some(ScriptOptions {
            interpreter: interpreter.to_path_buf(),
            cli: None,
            environment: vec![("NETTERM_TEST_FACTS".into(), facts.into())],
        }),
        ..JobOptions::default()
    }
}

#[tokio::test]
async fn the_job_runs_the_shipped_script_and_keeps_its_facts_with_the_run() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &site());
    assert_eq!(
        job::check(&plan, &JobOptions::default()),
        Err(StartError::NoPython)
    );
    let prepared = temp.path().join("prepared.json");
    fs::write(
        &prepared,
        r#"{"format": 1, "hosts": [
            {"id": "Site/r1.yaml", "facts": {"vendor": "Arista", "model": "cEOSLab", "serial_number": "SN-R1", "os_version": "4.36.0.1F"}},
            {"id": "Site/n1.yaml", "error": "the login was refused"}
        ]}"#,
    )
    .unwrap();
    let python = stand_in(temp.path());
    let runs = RunStore::new(temp.path().join("data/jobs"));
    let ws = workspace.id().unwrap().to_string();
    let mut recorder = runs.begin(&ws, &site(), &plan).unwrap();
    let manager = support::manager();
    let mut job = job::start(&manager, plan, options(&python.path, &prepared)).unwrap();
    let (events, report) = finish(&mut job).await;
    for event in &events {
        recorder.record(event).unwrap();
    }
    recorder
        .record(&JobEvent::Finished(report.clone()))
        .unwrap();

    let [result] = report.hosts.as_slice() else {
        panic!("one output: {report:?}")
    };
    assert_eq!(result.id.as_str(), SCRIPT_OUTPUT);
    assert_eq!(result.name, ENRICH_TITLE);
    assert_eq!(result.state, HostState::Done);
    let output = &result.output;
    let line = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name}= in {output}"))
            .trim()
            .to_string()
    };
    assert_eq!(line("args"), "-u -m netterm.enrich");
    assert!(
        !Path::new(&line("facts")).exists(),
        "the facts file is removed"
    );
    let json: String = output
        .lines()
        .skip_while(|line| !line.starts_with('{'))
        .take_while(|line| !line.starts_with("facts="))
        .collect();
    let targets: ScriptTargets = serde_json::from_str(&json).unwrap();
    let drivers: Vec<(&str, &str)> = targets
        .napalm_drivers
        .iter()
        .map(|(id, driver)| (id.as_str(), driver.as_str()))
        .collect();
    assert_eq!(
        drivers,
        [
            ("Site/n1.yaml", "nxos_ssh"),
            ("Site/r1.yaml", "eos"),
            ("Site/r2.yaml", "eos"),
            ("Site/r3.yaml", "ios"),
        ]
    );

    let facts = result.facts.as_ref().unwrap();
    assert_eq!(facts.hosts.len(), 4);
    assert_eq!(
        facts.hosts[2].error.as_deref(),
        Some("the script reported nothing for it (see its output)")
    );

    // Kept with the run: the preview comes from there.
    let run = runs.run(&ws, recorder.id()).unwrap();
    assert_eq!(run.step, JobStep::Enrich);
    assert_eq!(run.facts.as_ref(), Some(facts));
    assert_eq!(run.targets.len(), 4);
    let preview = enrich::preview_run(&workspace, &run).unwrap();
    assert_eq!(outcome(&preview, "Site/r1.yaml"), &PreviewOutcome::Changes);
    assert_eq!(
        outcome(&preview, "Site/n1.yaml"),
        &PreviewOutcome::Failed {
            reason: "the login was refused".into()
        }
    );
    assert!(matches!(
        outcome(&preview, "Site/x1.yaml"),
        PreviewOutcome::Skipped { .. }
    ));
    assert_eq!(FACTS_VARIABLE, "NETTERM_FACTS_FILE");
}

#[tokio::test]
async fn a_script_that_reports_nothing_fails_every_host_and_other_runs_have_no_preview() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &site());
    let python = FakePython::running(
        temp.path(),
        "broken",
        "echo No module named napalm 1>&2\r\nexit /b 1",
        "echo No module named napalm >&2\nexit 1",
    );
    let runs = RunStore::new(temp.path().join("data/jobs"));
    let ws = workspace.id().unwrap().to_string();
    let mut recorder = runs.begin(&ws, &site(), &plan).unwrap();
    let id_of_run = recorder.id().to_string();
    // Not finished yet: no facts.
    assert_eq!(
        enrich::preview_run(&workspace, &runs.run(&ws, &id_of_run).unwrap()),
        Err(EnrichError::NoFacts(id_of_run.clone()))
    );
    let manager = support::manager();
    let mut job = job::start(
        &manager,
        plan,
        options(&python.path, &temp.path().join("none.json")),
    )
    .unwrap();
    let (_, report) = finish(&mut job).await;
    recorder
        .record(&JobEvent::Finished(report.clone()))
        .unwrap();
    let result = &report.hosts[0];
    assert!(matches!(result.state, HostState::Failed { .. }));
    assert!(result.output.contains("No module named napalm"));
    let preview = enrich::preview_run(&workspace, &runs.run(&ws, &id_of_run).unwrap()).unwrap();
    let failed: Vec<&str> = preview
        .hosts
        .iter()
        .filter(|h| matches!(h.outcome, PreviewOutcome::Failed { .. }))
        .map(|h| h.id.as_str())
        .collect();
    assert_eq!(failed.len(), 4);

    // A script run is not one to preview.
    let script = JobStep::Script {
        path: "x.py".into(),
    };
    let mut run = runs.run(&ws, &id_of_run).unwrap();
    run.step = script;
    assert_eq!(
        enrich::preview_run(&workspace, &run),
        Err(EnrichError::NotEnrichment(id_of_run))
    );
}

#[test]
fn filling_in_cannot_be_saved_as_a_job() {
    let temp = TempDir::new().unwrap();
    let mut workspace = workspace(&temp);
    let refused = workspace.save_job(
        None,
        &SavedJob {
            name: "enrich".into(),
            step: JobStep::Enrich,
            target: site(),
            ignored_answers: Vec::new(),
        },
    );
    assert!(matches!(refused, Err(EditError::Invalid(_))), "{refused:?}");
}
