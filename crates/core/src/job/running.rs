//! The jobs running in this process, so that a cancel can reach any of them
//! and closing the app can end them all.
//!
//! A started job is followed with [`RunningJobs::follow`]: its events go
//! through the returned [`FollowedRun`] to its kept run, and it counts as
//! running until that is dropped (its last event recorded). Closing the app
//! calls [`RunningJobs::end_all`]: every running job is cancelled as Cancel
//! does (a script's processes end, so do the tunnels it holds), and its kept
//! run says why ([`APP_CLOSED`]), within a bounded wait. What a crash or a
//! kill leaves is covered elsewhere: a script's processes end with this
//! process (`crate::process`), and a run whose end was never written is
//! listed as interrupted.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::results::{ResultsError, RunRecorder};
use super::{JobCanceller, JobEvent};

/// Why a run was cancelled when the app was closed while it ran: kept as
/// its [`super::results::StoredRun::cancel_reason`].
pub const APP_CLOSED: &str = "the app was closed while the job ran";

/// How long closing the app waits for its jobs (and a Python setup) to
/// end before it goes anyway: long enough for sessions to close and a
/// partial upload to be removed.
pub const APP_CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The jobs running in this process, by the id [`RunningJobs::follow`]
/// gave them. Clones share them.
#[derive(Clone, Default)]
pub struct RunningJobs(Arc<Shared>);

#[derive(Default)]
struct Shared {
    next: AtomicU64,
    jobs: Mutex<BTreeMap<u64, Running>>,
    /// Told whenever a job stops running.
    ended: Condvar,
}

struct Running {
    canceller: JobCanceller,
    reason: Reason,
}

/// Why a job is being cancelled, when not by the user.
type Reason = Arc<Mutex<Option<String>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RunningJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Follows a started job (its `canceller`) whose run `recorder` keeps:
    /// record its events through the returned [`FollowedRun`]; the job
    /// counts as running until that is dropped.
    pub fn follow(&self, canceller: JobCanceller, recorder: RunRecorder) -> FollowedRun {
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let reason = Reason::default();
        lock(&self.0.jobs).insert(
            id,
            Running {
                canceller,
                reason: reason.clone(),
            },
        );
        FollowedRun {
            id,
            jobs: self.clone(),
            reason,
            recorder,
        }
    }

    /// How many jobs run.
    pub fn count(&self) -> usize {
        lock(&self.0.jobs).len()
    }

    /// Cancels the running job `id` as Cancel does; `false` if it has ended.
    pub fn cancel(&self, id: u64) -> bool {
        match lock(&self.0.jobs).get(&id) {
            Some(job) => {
                job.canceller.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancels every running job as Cancel does, its kept run keeping
    /// `reason`, and waits up to `within` for all of them to have recorded
    /// their end. Returns how many still run (their runs stay without an
    /// end, so they are listed as interrupted).
    pub fn end_all(&self, reason: &str, within: Duration) -> usize {
        let until = Instant::now() + within;
        let mut jobs = lock(&self.0.jobs);
        for job in jobs.values() {
            *lock(&job.reason) = Some(reason.to_string());
            job.canceller.cancel();
        }
        while !jobs.is_empty() {
            let Some(left) = until.checked_duration_since(Instant::now()) else {
                break;
            };
            jobs = self
                .0
                .ended
                .wait_timeout(jobs, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        jobs.len()
    }
}

/// A followed job's kept run: record the job's events here. Dropped, the
/// job no longer counts as running.
pub struct FollowedRun {
    id: u64,
    jobs: RunningJobs,
    reason: Reason,
    recorder: RunRecorder,
}

impl FollowedRun {
    /// The id the job is cancelled by ([`RunningJobs::cancel`]).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The kept run's id.
    pub fn run(&self) -> &str {
        self.recorder.id()
    }

    /// Keeps what `event` says ([`RunRecorder::record`]); the end also
    /// keeps why the job was cancelled, if [`RunningJobs::end_all`] did.
    pub fn record(&mut self, event: &JobEvent) -> Result<(), ResultsError> {
        if matches!(event, JobEvent::Finished(_)) {
            if let Some(reason) = lock(&self.reason).as_deref() {
                self.recorder.cancelled_because(reason);
            }
        }
        self.recorder.record(event)
    }
}

impl Drop for FollowedRun {
    fn drop(&mut self) {
        lock(&self.jobs.0.jobs).remove(&self.id);
        self.jobs.0.ended.notify_all();
    }
}
