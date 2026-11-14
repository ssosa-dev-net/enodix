//! Job results: runs kept in a temporary data folder, outside the
//! workspace, keyed by the workspace id; listed, read, searched, compared,
//! exported, deleted and planned again.

mod support;

use std::fs;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::diff::DiffLine;
use netterm_core::job::results::{
    OutputRef, ResultsError, RunRecorder, RunStore, OUTPUT_FOLDER, RUN_FILE,
};
use netterm_core::job::{
    self, HostResult, HostState, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget,
    SkipReason,
};
use netterm_core::workspace::{HostId, Workspace};
use support::workspaces::files;
use support::{TestSshServer, PASSWORD, USERNAME, WAIT};
use tempfile::TempDir;

/// A workspace with an id, made of `files` (relative path, text).
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

/// A host on the loopback address, port `port`, driven by frr.
fn host(port: u16) -> String {
    format!("address: 127.0.0.1\nport: {port}\nplatform: frr\n")
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

fn snippet(name: &str) -> JobStep {
    JobStep::Snippet {
        name: name.into(),
        answers: Default::default(),
    }
}

fn folder(path: &str) -> JobTarget {
    JobTarget::Folder { path: path.into() }
}

fn plan(workspace: &Workspace, store: &MemoryStore, target: &JobTarget, step: &JobStep) -> JobPlan {
    job::plan(workspace, &Profiles::new(store, workspace), target, step).unwrap()
}

fn id(workspace: &Workspace) -> &str {
    workspace.id().expect("the test workspace has an id")
}

fn output(run: &str, host: &str) -> OutputRef {
    OutputRef {
        run: run.into(),
        host: HostId::new(host),
    }
}

/// Records, by hand, a run of `plan` in which each host shows `outputs`
/// (in plan order) and ends done.
fn record_finished(recorder: &mut RunRecorder, plan: &JobPlan, outputs: &[&str]) {
    let mut hosts = Vec::new();
    for (host, text) in plan.hosts.iter().zip(outputs) {
        for state in [HostState::Connecting, HostState::Running] {
            recorder
                .record(&JobEvent::State {
                    host: host.id.clone(),
                    state,
                })
                .unwrap();
        }
        recorder
            .record(&JobEvent::Output {
                host: host.id.clone(),
                text: text.to_string(),
            })
            .unwrap();
        hosts.push(HostResult {
            id: host.id.clone(),
            name: host.name.clone(),
            state: HostState::Done,
            output: text.to_string(),
        });
    }
    recorder
        .record(&JobEvent::Finished(JobReport { hosts }))
        .unwrap();
}

/// Two hosts in `Site`, whose `show` snippet is planned without
/// connecting (the ports are never used).
fn site(temp: &TempDir) -> (Workspace, MemoryStore, JobPlan) {
    let workspace = workspace(
        temp,
        &[
            ("Site/_folder.yaml", "credential_profile: lab\n".into()),
            ("Site/r1.yaml", host(1)),
            ("Site/Core/r2.yaml", host(2)),
            ("_snippets.yaml", "show: show version\n".into()),
        ],
    );
    let store = store(&workspace);
    let planned = plan(&workspace, &store, &folder("Site"), &snippet("show"));
    (workspace, store, planned)
}

#[tokio::test]
async fn a_run_is_kept_outside_the_workspace_with_one_text_file_per_host() {
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let temp = TempDir::new().unwrap();
    let workspace = workspace(
        &temp,
        &[
            ("Site/_folder.yaml", "credential_profile: lab\n".into()),
            ("Site/r1.yaml", host(r1.port())),
            ("Site/Core/r2.yaml", host(r2.port())),
            ("Site/off.yaml", "address: 127.0.0.1\n".into()),
            ("_snippets.yaml", "hello: echo hello {{name}}\n".into()),
        ],
    );
    let before = files(workspace.root());
    let store = store(&workspace);
    let planned = plan(&workspace, &store, &folder("Site"), &snippet("hello"));
    assert_eq!(planned.skipped.len(), 1);

    let runs = RunStore::new(temp.path().join("data").join("jobs"));
    let mut recorder = runs
        .begin(id(&workspace), &folder("Site"), &planned)
        .unwrap();
    let run_id = recorder.id().to_string();
    let kept = recorder.run().clone();
    assert!(!kept.ended());
    assert!(kept.hosts.iter().all(|h| h.state == HostState::Waiting));

    let mut job = job::start(&support::manager(), planned, JobOptions::default()).unwrap();
    let report = tokio::time::timeout(WAIT * 3, async {
        loop {
            let event = job.next().await.expect("the job ended without a report");
            recorder.record(&event).unwrap();
            if let JobEvent::Finished(report) = event {
                return report;
            }
        }
    })
    .await
    .expect("the job did not finish");

    // The workspace is untouched; the run is under the store, by id.
    assert_eq!(files(workspace.root()), before);
    let folder_of_run = runs.root().join(id(&workspace)).join(&run_id);
    assert_eq!(
        files(&folder_of_run),
        [
            format!("{OUTPUT_FOLDER}/"),
            format!("{OUTPUT_FOLDER}/Site/"),
            format!("{OUTPUT_FOLDER}/Site/Core/"),
            format!("{OUTPUT_FOLDER}/Site/Core/r2.txt"),
            format!("{OUTPUT_FOLDER}/Site/r1.txt"),
            RUN_FILE.to_string(),
        ]
    );
    for result in &report.hosts {
        assert_eq!(result.state, HostState::Done, "{}", result.output);
        let kept = runs
            .output(id(&workspace), &output(&run_id, result.id.as_str()))
            .unwrap();
        assert_eq!(kept, result.output);
        assert!(kept.contains(&format!("hello {}\n", result.name)), "{kept}");
    }
    let run = runs.run(id(&workspace), &run_id).unwrap();
    assert!(run.ended());
    assert_eq!(run.id, run_id);
    assert_eq!(run.workspace, id(&workspace));
    assert_eq!(run.target, folder("Site"));
    assert_eq!(run.step, snippet("hello"));
    assert!(run.hosts.iter().all(|h| h.state == HostState::Done));
    assert_eq!(run.skipped.len(), 1);
    assert_eq!(run.skipped[0].id.as_str(), "Site/off.yaml");
    assert!(
        run.skipped[0].reason.contains("platform"),
        "{:?}",
        run.skipped
    );
}

#[test]
fn runs_are_listed_newest_first_and_survive_a_restart() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let root = temp.path().join("jobs");
    let ids: Vec<String> = {
        let runs = RunStore::new(&root);
        (0..3)
            .map(|n| {
                let mut recorder = runs
                    .begin(id(&workspace), &folder("Site"), &planned)
                    .unwrap();
                record_finished(&mut recorder, &planned, &[&format!("run {n}\n"), "x\n"]);
                recorder.id().to_string()
            })
            .collect()
    };
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3
    );

    // A new store on the same folder: as after restarting the app.
    let runs = RunStore::new(&root);
    let list = runs.list(id(&workspace)).unwrap();
    assert!(list.damaged.is_empty());
    let listed: Vec<&str> = list.runs.iter().map(|run| run.id.as_str()).collect();
    let newest_first: Vec<&str> = ids.iter().rev().map(String::as_str).collect();
    assert_eq!(listed, newest_first);
    assert!(list.runs.iter().all(|run| run.ended()));
    assert_eq!(
        runs.output(id(&workspace), &output(&ids[0], "Site/r1.yaml"))
            .unwrap(),
        "run 0\n"
    );

    // Another workspace's runs are kept apart.
    assert!(runs.list("another-workspace").unwrap().runs.is_empty());
}

#[test]
fn a_run_the_app_did_not_finish_is_listed_without_an_end() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let mut recorder = runs
        .begin(id(&workspace), &folder("Site"), &planned)
        .unwrap();
    let r1 = planned.hosts[0].id.clone();
    for state in [HostState::Connecting, HostState::Running] {
        recorder
            .record(&JobEvent::State {
                host: r1.clone(),
                state,
            })
            .unwrap();
    }
    recorder
        .record(&JobEvent::Output {
            host: r1.clone(),
            text: "half way\n".into(),
        })
        .unwrap();
    drop(recorder);

    let list = RunStore::new(runs.root()).list(id(&workspace)).unwrap();
    let run = &list.runs[0];
    assert!(!run.ended());
    assert_eq!(run.host(&r1).unwrap().state, HostState::Running);
    assert_eq!(run.hosts[1].state, HostState::Waiting);
    assert_eq!(
        runs.output(id(&workspace), &output(&run.id, r1.as_str()))
            .unwrap(),
        "half way\n"
    );
    assert_eq!(
        runs.output(id(&workspace), &output(&run.id, "Site/Core/r2.yaml"))
            .unwrap(),
        ""
    );
}

#[test]
fn search_shows_which_hosts_hold_a_text() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let mut recorder = runs
        .begin(id(&workspace), &folder("Site"), &planned)
        .unwrap();
    record_finished(
        &mut recorder,
        &planned,
        &[
            "r1# show version\nVersion 10.2\nUptime 3 days\nr1# ",
            "r2# show version\nVersion 9.1\nr2# ",
        ],
    );
    let run = recorder.id().to_string();
    let ws = id(&workspace);

    let hits = runs.search(ws, &run, "uptime").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].host.as_str(), "Site/r1.yaml");
    assert_eq!(hits[0].name, "r1");
    assert_eq!(hits[0].count, 1);
    assert_eq!(hits[0].lines[0].number, 3);
    assert_eq!(hits[0].lines[0].text, "Uptime 3 days");

    let hits = runs.search(ws, &run, "VERSION").unwrap();
    let hosts: Vec<&str> = hits.iter().map(|hit| hit.host.as_str()).collect();
    assert_eq!(hosts, ["Site/r1.yaml", "Site/Core/r2.yaml"]);
    assert!(hits.iter().all(|hit| hit.count == 2));

    assert!(runs.search(ws, &run, "nowhere").unwrap().is_empty());
    assert!(runs.search(ws, &run, "  ").unwrap().is_empty());
}

#[test]
fn two_hosts_or_one_host_in_two_runs_compare_line_by_line() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let ws = id(&workspace);
    let mut first = runs.begin(ws, &folder("Site"), &planned).unwrap();
    record_finished(&mut first, &planned, &["a\nb\nc\n", "a\nB\nc\n"]);
    let mut second = runs.begin(ws, &folder("Site"), &planned).unwrap();
    record_finished(&mut second, &planned, &["a\nb\nc\nd\n", "a\nB\nc\n"]);
    let (first, second) = (first.id().to_string(), second.id().to_string());

    let hosts = runs
        .compare(
            ws,
            &output(&first, "Site/r1.yaml"),
            &output(&first, "Site/Core/r2.yaml"),
        )
        .unwrap();
    let changed: Vec<&DiffLine> = hosts.iter().filter(|line| line.differs()).collect();
    assert_eq!(
        changed,
        [
            &DiffLine::Removed {
                old: 2,
                text: "b".into()
            },
            &DiffLine::Added {
                new: 2,
                text: "B".into()
            },
        ]
    );

    let runs_apart = runs
        .compare(
            ws,
            &output(&first, "Site/r1.yaml"),
            &output(&second, "Site/r1.yaml"),
        )
        .unwrap();
    assert_eq!(
        runs_apart.last(),
        Some(&DiffLine::Added {
            new: 4,
            text: "d".into()
        })
    );
    assert_eq!(runs_apart.iter().filter(|l| l.differs()).count(), 1);

    assert!(matches!(
        runs.compare(
            ws,
            &output(&first, "Site/nope.yaml"),
            &output(&first, "Site/r1.yaml")
        ),
        Err(ResultsError::NoSuchHost { .. })
    ));
}

#[test]
fn a_run_is_exported_to_a_chosen_folder_and_deleted_from_disk() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let ws = id(&workspace);
    let mut kept = runs.begin(ws, &folder("Site"), &planned).unwrap();
    record_finished(&mut kept, &planned, &["one\n", "two\n"]);
    let mut other = runs.begin(ws, &folder("Site"), &planned).unwrap();
    record_finished(&mut other, &planned, &["three\n", "four\n"]);
    let (kept, other) = (kept.id().to_string(), other.id().to_string());

    let chosen = temp.path().join("chosen");
    fs::create_dir(&chosen).unwrap();
    let copy = runs.export(ws, &kept, &chosen).unwrap();
    assert_eq!(copy, chosen.join(&kept));
    assert_eq!(
        files(&copy),
        [
            "output/".to_string(),
            "output/Site/".into(),
            "output/Site/Core/".into(),
            "output/Site/Core/r2.txt".into(),
            "output/Site/r1.txt".into(),
            RUN_FILE.into(),
        ]
    );
    assert_eq!(
        fs::read_to_string(copy.join("output/Site/r1.txt")).unwrap(),
        "one\n"
    );
    // Saving again does not write over the first copy.
    let again = runs.export(ws, &kept, &chosen).unwrap();
    assert_eq!(again, chosen.join(format!("{kept}_2")));
    assert!(matches!(
        runs.export(ws, &kept, &chosen.join("missing")),
        Err(ResultsError::NotAFolder(_))
    ));

    runs.delete(ws, &kept).unwrap();
    assert!(!runs.root().join(ws).join(&kept).exists());
    let listed: Vec<String> = runs
        .list(ws)
        .unwrap()
        .runs
        .into_iter()
        .map(|run| run.id)
        .collect();
    assert_eq!(listed, [other]);
    assert!(matches!(
        runs.run(ws, &kept),
        Err(ResultsError::NoSuchRun(_))
    ));
    assert!(matches!(
        runs.delete(ws, &kept),
        Err(ResultsError::NoSuchRun(_))
    ));
    // The export stays.
    assert!(copy.join(RUN_FILE).is_file());
}

#[test]
fn run_again_plans_the_same_step_and_target_against_the_workspace_as_it_is() {
    let temp = TempDir::new().unwrap();
    let (workspace, store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let ws = id(&workspace).to_string();
    let mut recorder = runs.begin(&ws, &folder("Site"), &planned).unwrap();
    record_finished(&mut recorder, &planned, &["", ""]);
    let by_folder = recorder.id().to_string();
    let hosts = JobTarget::Hosts {
        ids: vec![
            HostId::new("Site/r1.yaml"),
            HostId::new("Site/Core/r2.yaml"),
        ],
    };
    let listed = plan(&workspace, &store, &hosts, &snippet("show"));
    let mut recorder = runs.begin(&ws, &hosts, &listed).unwrap();
    record_finished(&mut recorder, &listed, &["", ""]);
    let by_hosts = recorder.id().to_string();

    // Since: r2 is gone, r3 is new in the folder.
    let root = workspace.root().to_path_buf();
    fs::remove_file(root.join("Site/Core/r2.yaml")).unwrap();
    fs::write(root.join("Site/r3.yaml"), host(3)).unwrap();
    let workspace = Workspace::open(&root).unwrap();
    let profiles = Profiles::new(&store, &workspace);

    let again =
        job::plan_again(&workspace, &profiles, &runs.run(&ws, &by_folder).unwrap()).unwrap();
    assert_eq!(again.step, snippet("show"));
    let ids: Vec<&str> = again.hosts.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["Site/r1.yaml", "Site/r3.yaml"]);
    assert!(again.skipped.is_empty());
    assert!(again.ready());

    let again = job::plan_again(&workspace, &profiles, &runs.run(&ws, &by_hosts).unwrap()).unwrap();
    let ids: Vec<&str> = again.hosts.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["Site/r1.yaml"]);
    assert_eq!(again.skipped.len(), 1);
    assert_eq!(again.skipped[0].reason, SkipReason::Gone);

    // A snippet removed since cannot be planned.
    fs::write(root.join("_snippets.yaml"), "other: x\n").unwrap();
    let workspace = Workspace::open(&root).unwrap();
    let profiles = Profiles::new(&store, &workspace);
    assert!(job::plan_again(&workspace, &profiles, &runs.run(&ws, &by_folder).unwrap()).is_err());
}

#[test]
fn damaged_runs_are_listed_apart_and_bad_names_are_refused() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let ws = id(&workspace);
    let mut good = runs.begin(ws, &folder("Site"), &planned).unwrap();
    record_finished(&mut good, &planned, &["", ""]);
    let damaged = runs.root().join(ws).join("2020-01-01_00-00-00");
    fs::create_dir_all(&damaged).unwrap();
    fs::write(damaged.join(RUN_FILE), "hosts: [not, a, run\n").unwrap();
    fs::create_dir_all(runs.root().join(ws).join("empty")).unwrap();

    let list = runs.list(ws).unwrap();
    assert_eq!(list.runs.len(), 1);
    let damaged: Vec<&str> = list.damaged.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(damaged, ["empty", "2020-01-01_00-00-00"]);
    // A damaged run can still be deleted.
    runs.delete(ws, "2020-01-01_00-00-00").unwrap();

    for bad in ["..", ".", "", "a/b", "a\\b"] {
        assert!(matches!(
            runs.list(bad),
            Err(ResultsError::BadWorkspaceId(_))
        ));
        assert!(runs.run(ws, bad).is_err(), "{bad:?}");
        assert!(runs.delete(ws, bad).is_err(), "{bad:?}");
    }
    assert!(runs.delete(ws, &format!("../{ws}")).is_err());
    assert!(runs.root().join(ws).is_dir());
}

#[test]
fn a_running_run_is_neither_deleted_nor_exported() {
    let temp = TempDir::new().unwrap();
    let (workspace, _store, planned) = site(&temp);
    let runs = RunStore::new(temp.path().join("jobs"));
    let ws = id(&workspace);
    let chosen = temp.path().join("chosen");
    fs::create_dir(&chosen).unwrap();

    let mut recorder = runs.begin(ws, &folder("Site"), &planned).unwrap();
    let running = recorder.id().to_string();
    // Another handle on the same store (as the shell's commands use).
    let other = runs.clone();
    assert_eq!(other.running(ws), std::slice::from_ref(&running));
    assert!(other.running("another-workspace").is_empty());
    assert!(matches!(
        other.delete(ws, &running),
        Err(ResultsError::Running(_))
    ));
    assert!(matches!(
        other.export(ws, &running, &chosen),
        Err(ResultsError::Running(_))
    ));
    assert!(runs.root().join(ws).join(&running).join(RUN_FILE).is_file());

    record_finished(&mut recorder, &planned, &["", ""]);
    assert!(other.running(ws).is_empty());
    other.export(ws, &running, &chosen).unwrap();

    // A recorder dropped before the end (the app closing) lets go too.
    let dropped = runs.begin(ws, &folder("Site"), &planned).unwrap();
    let interrupted = dropped.id().to_string();
    drop(dropped);
    assert!(runs.running(ws).is_empty());
    runs.delete(ws, &interrupted).unwrap();
    runs.delete(ws, &running).unwrap();
}
