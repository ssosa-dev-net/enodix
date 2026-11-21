//! The plans on screen: each pre-flight's plan, kept under an id of its own
//! until its job starts or it is forgotten, so that starting runs exactly
//! the plan the user looked at (and confirmed) in that tab, never another
//! tab's or window's.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::{JobPlan, JobTarget};
use crate::workspace::HostId;

/// The most plans kept at once; making one more forgets the oldest (its
/// Start is then refused, asking to plan again).
pub const MAX_KEPT_PLANS: usize = 32;

/// The id a kept plan is known by, given when it is kept; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanId(pub u64);

/// A plan, with what it was made for: the workspace (by id, `None` if it
/// has none) and the target, both kept with the run.
#[derive(Debug, Clone)]
pub struct KeptPlan {
    pub workspace: Option<String>,
    pub target: JobTarget,
    pub plan: JobPlan,
}

/// Why a kept plan may not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanRefused {
    /// No plan is kept under that id: its job started already, its tab
    /// closed, another workspace was opened, or too many were planned
    /// since.
    #[error("this pre-flight is no longer current (the job started already, another workspace was opened, or many jobs were planned since): plan the job again")]
    Gone,
    /// The plan has production hosts the start did not say were confirmed.
    #[error("{}", not_confirmed(.0))]
    ProductionNotConfirmed(Vec<String>),
}

/// "the production host r1 was not confirmed for this job: …".
fn not_confirmed(names: &[String]) -> String {
    match names {
        [one] => format!(
            "the production host {one} was not confirmed for this job: confirm it in its pre-flight"
        ),
        many => format!(
            "the production hosts {} were not confirmed for this job: confirm them in its pre-flight",
            many.join(", ")
        ),
    }
}

/// The plans on screen, by id, oldest first, at most [`MAX_KEPT_PLANS`].
#[derive(Debug, Default)]
pub struct KeptPlans {
    last: u64,
    plans: VecDeque<(PlanId, KeptPlan)>,
}

impl KeptPlans {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps `plan` under a new id (forgetting the oldest beyond
    /// [`MAX_KEPT_PLANS`]).
    pub fn keep(&mut self, plan: KeptPlan) -> PlanId {
        self.last += 1;
        let id = PlanId(self.last);
        self.plans.push_back((id, plan));
        while self.plans.len() > MAX_KEPT_PLANS {
            self.plans.pop_front();
        }
        id
    }

    /// The plan kept under `id`, if it is.
    pub fn get(&self, id: PlanId) -> Option<&KeptPlan> {
        self.plans
            .iter()
            .find(|(kept, _)| *kept == id)
            .map(|(_, plan)| plan)
    }

    /// The plan kept under `id`, if it may start: every production host
    /// it runs must be among `confirmed` (the production hosts its
    /// pre-flight's user confirmed). Refused if it is not kept, or names
    /// the production hosts not confirmed. Keeps it: [`Self::forget`] it
    /// once its job has started.
    pub fn ready(&self, id: PlanId, confirmed: &[HostId]) -> Result<&KeptPlan, PlanRefused> {
        let kept = self.get(id).ok_or(PlanRefused::Gone)?;
        let unconfirmed: Vec<String> = kept
            .plan
            .production
            .iter()
            .filter(|host| !confirmed.contains(host))
            .map(|host| {
                kept.plan
                    .hosts
                    .iter()
                    .find(|planned| planned.id == *host)
                    .map_or_else(|| host.as_str().to_string(), |planned| planned.name.clone())
            })
            .collect();
        if unconfirmed.is_empty() {
            Ok(kept)
        } else {
            Err(PlanRefused::ProductionNotConfirmed(unconfirmed))
        }
    }

    /// Forgets the plan kept under `id` (its job started, or its tab
    /// closed); whether it was kept.
    pub fn forget(&mut self, id: PlanId) -> bool {
        let before = self.plans.len();
        self.plans.retain(|(kept, _)| *kept != id);
        self.plans.len() != before
    }

    /// Forgets every plan (another workspace was opened).
    pub fn clear(&mut self) {
        self.plans.clear();
    }

    /// How many plans are kept.
    pub fn len(&self) -> usize {
        self.plans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
}
