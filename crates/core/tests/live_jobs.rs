//! The record of live jobs (#77): every job started in this app run, with
//! what it has reported so far, so that a window opened late (or reloaded)
//! shows what already happened and then follows the numbered events.

mod support;

use std::collections::BTreeMap;

use netterm_core::configs::SaveOutcome;
use netterm_core::credentials::{MemoryStore, Profiles};
use netterm_core::job::{
    self, HostResult, HostState, JobEvent, JobPlan, JobReport, JobStep, JobTarget, LiveJobs,
    NewLiveJob,
};
use netterm_core::workspace::{HostId, Workspace};
use support::workspaces::fixture;

fn target() -> JobTarget {
    JobTarget::Folder {
        path: "Madrid".into(),
    }
}

/// A plan of the `jobs` fixture (its hosts may all be skipped: the record
/// only keeps it).
fn plan() -> JobPlan {
    let workspace = Workspace::open(fixture("jobs")).unwrap();
    let store = MemoryStore::new();
    job::plan(
        &workspace,
        &Profiles::new(&store, &workspace),
        &target(),
        &JobStep::Snippet {
            name: "Show version".into(),
            answers: BTreeMap::new(),
        },
    )
    .unwrap()
}

fn started(jobs: &mut LiveJobs, id: u64, run: &str) {
    jobs.start(NewLiveJob {
        id,
        workspace: "C:/work/lab".into(),
        run: run.into(),
        label: "Madrid".into(),
        target: target(),
        plan: plan(),
    });
}

fn host(id: &str) -> HostId {
    HostId::new(id)
}

fn state(id: &str, state: HostState) -> JobEvent {
    JobEvent::State {
        host: host(id),
        state,
    }
}

fn output(id: &str, text: &str) -> JobEvent {
    JobEvent::Output {
        host: host(id),
        text: text.into(),
    }
}

fn result(id: &str, state: HostState, output: &str) -> HostResult {
    HostResult {
        id: host(id),
        name: id.into(),
        state,
        output: output.into(),
        backup: None,
        exit_status: None,
        facts: None,
        bytes: None,
    }
}

#[test]
fn a_started_job_has_reported_nothing_yet() {
    let mut jobs = LiveJobs::default();
    started(&mut jobs, 1, "2026-11-15_10-00-00");
    let job = &jobs.jobs()[0];
    assert_eq!(job.id, 1);
    assert_eq!(job.run, "2026-11-15_10-00-00");
    assert_eq!(job.workspace, "C:/work/lab");
    assert_eq!(job.label, "Madrid");
    assert_eq!(job.target, target());
    assert_eq!(job.plan, plan());
    assert!(job.states.is_empty() && job.outputs.is_empty());
    assert!(!job.finished);
    assert_eq!(job.events, 0);
}

#[test]
fn events_are_kept_and_numbered_from_one() {
    let mut jobs = LiveJobs::default();
    started(&mut jobs, 1, "run-a");
    assert_eq!(
        jobs.record(1, &state("Madrid/r1.yaml", HostState::Running)),
        Some(1)
    );
    assert_eq!(jobs.record(1, &output("Madrid/r1.yaml", "r1# ")), Some(2));
    assert_eq!(
        jobs.record(1, &output("Madrid/r1.yaml", "show version\n")),
        Some(3)
    );
    assert_eq!(
        jobs.record(
            1,
            &JobEvent::Backup {
                host: host("Madrid/r1.yaml"),
                outcome: SaveOutcome::Saved {
                    version: "2026-11-15_10-00-01".into(),
                },
            },
        ),
        Some(4)
    );
    let job = &jobs.jobs()[0];
    assert_eq!(job.events, 4);
    assert_eq!(job.states[&host("Madrid/r1.yaml")], HostState::Running);
    assert_eq!(job.outputs[&host("Madrid/r1.yaml")], "r1# show version\n");
    assert_eq!(
        job.backups[&host("Madrid/r1.yaml")],
        SaveOutcome::Saved {
            version: "2026-11-15_10-00-01".into()
        }
    );
    assert!(!job.finished);
}

#[test]
fn an_event_of_an_unknown_job_is_not_kept() {
    let mut jobs = LiveJobs::default();
    started(&mut jobs, 1, "run-a");
    assert_eq!(jobs.record(2, &output("Madrid/r1.yaml", "x")), None);
    assert_eq!(jobs.jobs()[0].events, 0);
}

#[test]
fn finishing_takes_the_report_as_it_is() {
    let mut jobs = LiveJobs::default();
    started(&mut jobs, 1, "run-a");
    jobs.record(1, &output("Madrid/r1.yaml", "partial"));
    let mut script = result(
        "script",
        HostState::Failed {
            reason: "exit 3".into(),
        },
        "",
    );
    script.exit_status = Some(3);
    let mut backed_up = result("Madrid/r2.yaml", HostState::Done, "config");
    backed_up.backup = Some(SaveOutcome::Unchanged {
        version: "v1".into(),
    });
    jobs.record(
        1,
        &JobEvent::Finished(JobReport {
            hosts: vec![
                result("Madrid/r1.yaml", HostState::Cancelled, "partial and more"),
                backed_up,
                script,
            ],
            file: None,
        }),
    );
    let job = &jobs.jobs()[0];
    assert!(job.finished);
    assert_eq!(job.events, 2);
    assert_eq!(job.states[&host("Madrid/r1.yaml")], HostState::Cancelled);
    assert_eq!(job.outputs[&host("Madrid/r1.yaml")], "partial and more");
    assert_eq!(
        job.backups[&host("Madrid/r2.yaml")],
        SaveOutcome::Unchanged {
            version: "v1".into()
        }
    );
    assert_eq!(job.exits[&host("script")], 3);
}

#[test]
fn jobs_are_listed_in_start_order() {
    let mut jobs = LiveJobs::default();
    started(&mut jobs, 1, "run-a");
    started(&mut jobs, 2, "run-b");
    let runs: Vec<_> = jobs.jobs().iter().map(|job| job.run.as_str()).collect();
    assert_eq!(runs, ["run-a", "run-b"]);
    assert_eq!(jobs.job(2).map(|job| job.id), Some(2));
    assert!(jobs.job(3).is_none());
}

#[test]
fn forgetting_finished_jobs_keeps_running_ones_and_the_one_asked_for() {
    let mut jobs = LiveJobs::default();
    for (id, run) in [(1, "run-a"), (2, "run-b"), (3, "run-c")] {
        started(&mut jobs, id, run);
    }
    for id in [1, 2] {
        jobs.record(
            id,
            &JobEvent::Finished(JobReport {
                hosts: Vec::new(),
                file: None,
            }),
        );
    }
    jobs.forget_finished(Some("run-b"));
    let runs: Vec<_> = jobs.jobs().iter().map(|job| job.run.as_str()).collect();
    assert_eq!(runs, ["run-b", "run-c"]);
    jobs.forget_finished(None);
    let runs: Vec<_> = jobs.jobs().iter().map(|job| job.run.as_str()).collect();
    assert_eq!(runs, ["run-c"]);
}

#[test]
fn the_record_serialises_as_the_interface_reads_it() {
    let mut jobs = LiveJobs::default();
    started(&mut jobs, 7, "run-a");
    jobs.record(7, &output("Madrid/r1.yaml", "hello\n"));
    let value = serde_json::to_value(&jobs.jobs()[0]).unwrap();
    assert_eq!(value["id"], 7);
    assert_eq!(value["run"], "run-a");
    assert_eq!(value["outputs"]["Madrid/r1.yaml"], "hello\n");
    assert_eq!(value["events"], 1);
    assert_eq!(value["finished"], false);
    assert_eq!(value["plan"]["step"]["kind"], "snippet");
    assert!(value["plan"]["missingVariables"].is_array());
}
