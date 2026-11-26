//! A snippet's steps on an open session (#23): what they will do, for the
//! confirmation, and running and stopping them. The run is the core's
//! (`enodix_core::steps`); its events go to the window over a `Channel`.

use std::collections::BTreeMap;

use enodix_core::paste::confirms_steps;
use enodix_core::session::{SessionId, SessionManager, SessionRecords};
use enodix_core::steps::{Permission, RunEvent, StepRuns, StepsPlan};
use enodix_core::template::Variables;
use enodix_core::workspace::{Environment, ExpandError};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::{read, session_windows, Shell, Tabs, UserPreferences};

/// The steps of a snippet as the confirmation shows them.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum StepsPreview {
    /// What they will do, one line each, and whether the user is asked.
    #[serde(rename_all = "camelCase")]
    Ready {
        preview: Vec<String>,
        /// How many steps there are.
        count: usize,
        /// Whether the user confirms them first (the paste rule: always on
        /// production).
        confirm: bool,
        environment: Option<Environment>,
        /// What identifies these steps as shown: `steps_start` takes it
        /// back, and refuses steps that differ.
        digest: String,
    },
    /// Ask the user for these variables first.
    Missing { names: Vec<String> },
}

/// The snippet `name`'s steps for the session `id`, filled from its host
/// (while the open workspace is its own) and `answers`, and the safety of
/// the session's host.
fn plan(
    shell: &Shell,
    records: &SessionRecords,
    id: u64,
    name: &str,
    answers: BTreeMap<String, String>,
) -> Result<
    (
        Result<StepsPlan, ExpandError>,
        enodix_core::paste::PasteSafety,
    ),
    String,
> {
    let answers: Variables = answers.into_iter().collect();
    read(shell, |workspace| {
        let host = session_windows::snippet_host(records, workspace, id);
        let safety = records.paste_safety(SessionId(id), Some(workspace));
        (
            workspace.plan_snippet_steps(name, host.as_ref(), &answers),
            safety,
        )
    })
}

/// What the steps of the snippet `name` will do on the session `id`, for
/// the user to check before they run; or the variables to ask for first.
#[tauri::command]
pub(super) fn steps_preview(
    shell: State<'_, Shell>,
    records: State<'_, SessionRecords>,
    preferred: State<'_, UserPreferences>,
    id: u64,
    name: String,
    answers: BTreeMap<String, String>,
) -> Result<StepsPreview, String> {
    let (planned, safety) = plan(&shell, &records, id, &name, answers)?;
    match planned {
        Ok(plan) => Ok(StepsPreview::Ready {
            preview: plan.preview(),
            count: plan.len(),
            confirm: confirms_steps(&safety, preferred.lock().confirm_multi_line_paste),
            environment: safety.environment,
            digest: plan.digest(),
        }),
        Err(ExpandError::Missing(missing)) => Ok(StepsPreview::Missing {
            names: missing.names,
        }),
        Err(error) => Err(error.to_string()),
    }
}

/// Runs the steps of the snippet `name` on the session `id`; their events
/// go to `on_event`, the last saying how the run ended. `previewed`: the
/// digest of the steps the user was shown; `confirmed`: the user said yes
/// to them. The core refuses steps that differ from those shown, unconfirmed
/// ones where they are confirmed, a session of the multi-send group, and a
/// session that runs steps already.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(super) async fn steps_start(
    shell: State<'_, Shell>,
    records: State<'_, SessionRecords>,
    preferred: State<'_, UserPreferences>,
    manager: State<'_, SessionManager>,
    tabs: State<'_, Tabs>,
    runs: State<'_, StepRuns>,
    id: u64,
    name: String,
    answers: BTreeMap<String, String>,
    previewed: String,
    confirmed: bool,
    on_event: Channel<RunEvent>,
) -> Result<(), String> {
    let (planned, safety) = plan(&shell, &records, id, &name, answers)?;
    let plan = planned.map_err(|error| error.to_string())?;
    let permission = Permission {
        safety,
        confirm_multi_line: preferred.lock().confirm_multi_line_paste,
        previewed,
        confirmed,
        in_group: tabs.lock().member_showing(SessionId(id)).is_some(),
    };
    runs.start(&manager, SessionId(id), plan, &permission, move |event| {
        let _ = on_event.send(event);
    })
    .map_err(|error| error.to_string())
}

/// Stops the steps running on the session `id`; false if none were.
#[tauri::command]
pub(super) fn steps_stop(runs: State<'_, StepRuns>, id: u64) -> bool {
    runs.stop(SessionId(id))
}
