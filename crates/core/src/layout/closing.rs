//! What closing a window or leaving a workspace does to the sessions of
//! the session windows (#88).
//!
//! The main window's page knows its own sessions and counts and closes
//! them itself; the sessions in session windows are counted and closed
//! here, from the app's windows ([`Windows`]), what each session last
//! reported ([`SessionRelays::live`]) and the workspace it came from
//! ([`SessionRecord::of_workspace`](crate::session::SessionRecord::of_workspace)).

use std::path::Path;

use super::{WindowId, Windows};
use crate::session::{SessionId, SessionManager, SessionRecords, SessionRelays};

/// How many sessions of the session windows are live (opening or
/// connected): every one with `workspace` `None` (closing the main
/// window), else only those to hosts of the workspace at that root
/// (switching away from it).
pub fn live_in_session_windows(
    windows: &Windows,
    relays: &SessionRelays,
    records: &SessionRecords,
    workspace: Option<&Path>,
) -> usize {
    windows
        .session_window_sessions()
        .into_iter()
        .filter(|&(_, session)| workspace.is_none_or(|root| of_workspace(records, session, root)))
        .filter(|&(_, session)| relays.live(session))
        .count()
}

/// What closing a workspace's sessions in the session windows changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceClosed {
    /// The session windows left without a tab, closed, in order.
    pub closed: Vec<WindowId>,
    /// The session windows still open that lost a tab, in order: each is
    /// to be shown its layout.
    pub changed: Vec<WindowId>,
}

/// Closes, in every session window, each session to a host of the
/// workspace at `workspace` (switched away from, the user closing its
/// sessions): its tab goes ([`Windows::close_sessions`]), its relay is
/// dropped, the session discarded and its record forgotten. Live or not:
/// a closed tab of that workspace goes too.
pub fn close_workspace_sessions(
    windows: &mut Windows,
    manager: &SessionManager,
    relays: &SessionRelays,
    records: &SessionRecords,
    workspace: &Path,
) -> WorkspaceClosed {
    let closing: Vec<_> = windows
        .session_window_sessions()
        .into_iter()
        .filter(|&(_, session)| of_workspace(records, session, workspace))
        .collect();
    let sessions: Vec<SessionId> = closing.iter().map(|&(_, session)| session).collect();
    let closed = windows.close_sessions(&sessions);
    let mut changed: Vec<WindowId> = Vec::new();
    for (member, _) in &closing {
        if !closed.contains(&member.window) && !changed.contains(&member.window) {
            changed.push(member.window);
        }
    }
    for session in sessions {
        relays.forget(session);
        manager.discard(session);
        records.forget(session);
    }
    WorkspaceClosed { closed, changed }
}

fn of_workspace(records: &SessionRecords, session: SessionId, root: &Path) -> bool {
    records
        .get(session)
        .is_some_and(|record| record.of_workspace(root))
}
