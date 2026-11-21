//! The record of live jobs (#77): every job started in this app run, with
//! what it has reported so far. The app keeps one record for all its
//! windows, so a window opened late (a detached job window, a reloaded
//! page) shows what already happened, then follows the events: each is
//! numbered as it is recorded, and a window that read a job's record
//! applies only the events numbered after its `events`.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{HostState, JobEvent, JobPlan, JobTarget, SentFile};
use crate::configs::SaveOutcome;
use crate::workspace::HostId;

/// A job as it starts: what the record keeps besides its progress.
#[derive(Debug, Clone)]
pub struct NewLiveJob {
    /// The id the app gave it (for cancelling).
    pub id: u64,
    /// The workspace it runs in, as the interface names it (its root).
    pub workspace: String,
    /// Its kept run.
    pub run: String,
    /// What it runs on, for the user.
    pub label: String,
    pub target: JobTarget,
    pub plan: JobPlan,
}

/// A job started in this app run, and what it has reported so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveJob {
    pub id: u64,
    pub workspace: String,
    pub run: String,
    pub label: String,
    pub target: JobTarget,
    pub plan: JobPlan,
    /// Each host's latest state (hosts that reported none yet are absent).
    pub states: BTreeMap<HostId, HostState>,
    /// Each host's output so far (its report's once finished).
    pub outputs: BTreeMap<HostId, String>,
    /// What a config backup kept, per host.
    pub backups: BTreeMap<HostId, SaveOutcome>,
    /// A script's exit status, once it ended with one.
    pub exits: BTreeMap<HostId, i32>,
    /// A file job's latest progress per host, as `[done, total]`.
    pub progress: BTreeMap<HostId, (u64, Option<u64>)>,
    /// A file job's bytes sent per host, once its file went through.
    pub bytes: BTreeMap<HostId, u64>,
    /// A file job's file, once it ended (with its hashes).
    pub file: Option<SentFile>,
    /// Whether its last event (the report) has come.
    pub finished: bool,
    /// How many events it has recorded: the number of the latest.
    pub events: u64,
}

impl LiveJob {
    fn new(job: NewLiveJob) -> Self {
        Self {
            id: job.id,
            workspace: job.workspace,
            run: job.run,
            label: job.label,
            target: job.target,
            plan: job.plan,
            states: BTreeMap::new(),
            outputs: BTreeMap::new(),
            backups: BTreeMap::new(),
            exits: BTreeMap::new(),
            progress: BTreeMap::new(),
            bytes: BTreeMap::new(),
            file: None,
            finished: false,
            events: 0,
        }
    }

    /// Takes `event` in; returns its number.
    fn record(&mut self, event: &JobEvent) -> u64 {
        match event {
            JobEvent::State { host, state } => {
                self.states.insert(host.clone(), state.clone());
            }
            JobEvent::Output { host, text } => {
                self.outputs.entry(host.clone()).or_default().push_str(text);
            }
            JobEvent::Backup { host, outcome } => {
                self.backups.insert(host.clone(), outcome.clone());
            }
            JobEvent::Progress { host, done, total } => {
                self.progress.insert(host.clone(), (*done, *total));
            }
            JobEvent::Finished(report) => {
                for result in &report.hosts {
                    self.states.insert(result.id.clone(), result.state.clone());
                    self.outputs
                        .insert(result.id.clone(), result.output.clone());
                    if let Some(backup) = &result.backup {
                        self.backups.insert(result.id.clone(), backup.clone());
                    }
                    if let Some(status) = result.exit_status {
                        self.exits.insert(result.id.clone(), status);
                    }
                    if let Some(bytes) = result.bytes {
                        self.bytes.insert(result.id.clone(), bytes);
                    }
                }
                self.file.clone_from(&report.file);
                self.finished = true;
            }
        }
        self.events += 1;
        self.events
    }
}

/// Every job started in this app run that has not been forgotten, in the
/// order they started.
#[derive(Debug, Default)]
pub struct LiveJobs {
    jobs: Vec<LiveJob>,
}

impl LiveJobs {
    /// Adds a job that has just started; returns its record.
    pub fn start(&mut self, job: NewLiveJob) -> &LiveJob {
        self.jobs.push(LiveJob::new(job));
        self.jobs.last().expect("just added")
    }

    /// Records `event` of the job `id`; returns the event's number, or
    /// `None` if no such job is kept.
    pub fn record(&mut self, id: u64, event: &JobEvent) -> Option<u64> {
        let job = self.jobs.iter_mut().find(|job| job.id == id)?;
        Some(job.record(event))
    }

    pub fn jobs(&self) -> &[LiveJob] {
        &self.jobs
    }

    pub fn job(&self, id: u64) -> Option<&LiveJob> {
        self.jobs.iter().find(|job| job.id == id)
    }

    /// Forgets the finished jobs, except the one keeping the run `keep`
    /// (their runs stay on disk).
    pub fn forget_finished(&mut self, keep: Option<&str>) {
        self.jobs
            .retain(|job| !job.finished || Some(job.run.as_str()) == keep);
    }
}
