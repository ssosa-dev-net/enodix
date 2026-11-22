//! The multi-send group across windows (#87): the shell's side.
//!
//! The group is the app's (`Windows`, #83): typed keys fan out in
//! `session_input` whichever window each member is in. What this adds:
//!
//! - every window is told its layout (`layout-changed`) when the group
//!   changes, so each shows the members' markers and the group's size;
//! - the pasting window learns every member, wherever it is, from the
//!   app's record of its session (`group_members`): its name, its paste
//!   safety and its host, so it plans the paste and asks once;
//! - once confirmed, each member's lines go to the window showing it
//!   (`group_paste_send` → `group-paste-lines`, the pasting window's own
//!   included), which sends them through that member's own terminal (its
//!   bracketed paste, its line delay, stopping when its session ends or it
//!   leaves the group). Nothing goes through the fan-out.
//!
//! A group paste goes to the members as they are when it is sent (#123):
//! the members come with the group's revision, and the paste is called
//! off (nothing sent, the reason said) if the group changed since, or
//! the tab pasted in is no longer a member (`group_paste_check` lets the
//! window close its question as soon as it sees the change).
//!
//! What is decided is the core's: the members and their order and the
//! revision (`Windows::group_paste_members`), whether the paste may go and
//! where each member's lines go (`Windows::group_paste_targets`), whether
//! the group changed (`Windows::group_revision`), a member's safety
//! (`SessionRecords::paste_safety`) and host (`SessionRecord::host_in`).

use std::sync::atomic::Ordering;

use enodix_core::layout::{TabId, WindowId};
use enodix_core::paste::PasteSafety;
use enodix_core::session::{SessionId, SessionRecords, SessionRelays, SessionState};
use enodix_core::workspace::HostId;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::{calling_window, session_windows, Shell, Tabs};

/// A window is told its layout: the multi-send group changed elsewhere.
pub(super) const LAYOUT_CHANGED_EVENT: &str = "layout-changed";
/// A window is sent a member's lines of a confirmed group paste.
const GROUP_PASTE_LINES_EVENT: &str = "group-paste-lines";

/// Tells every window its layout if the multi-send group changed since
/// the windows were last told: a member's marker and the group's size are
/// shown in every window; and the main window the terminal used last,
/// which may have joined or left. Called after every change to the
/// windows.
pub(super) fn tell_windows(app: &AppHandle, tabs: &Tabs, records: &SessionRecords) {
    let views: Vec<_> = {
        let windows = tabs.lock();
        let revision = windows.group_revision();
        if tabs.told.swap(revision, Ordering::SeqCst) == revision {
            return;
        }
        windows
            .windows()
            .into_iter()
            .filter_map(|window| Some((window, windows.view(window)?)))
            .collect()
    };
    for (window, view) in views {
        let _ = app.emit_to(
            session_windows::label_for(window).as_str(),
            LAYOUT_CHANGED_EVENT,
            view,
        );
    }
    session_windows::tell_snippet_target(app, tabs, records);
}

/// A member of the multi-send group as a pasting window sees it,
/// whichever window shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupMember {
    window: WindowId,
    tab: TabId,
    session: u64,
    /// Its tab's name.
    name: String,
    /// What applies to its pastes: its host's, as the open workspace sets
    /// it now, else as it opened.
    safety: PasteSafety,
    /// The host its snippets are filled from, if in the open workspace.
    host: Option<HostId>,
    /// Whether its session is connected (only those take a paste).
    connected: bool,
}

/// Who a paste in a tab goes to, and the group's revision they were read
/// at (the paste must still find it when it is sent, #123).
#[derive(Serialize)]
pub(super) struct GroupMembers {
    revision: u64,
    members: Vec<GroupMember>,
}

/// Where a paste in the calling window's tab `tab` goes (#87): every
/// member of the group whose session is known, the one pasted in first,
/// in any window, with the group's revision. Refused (the paste is called
/// off, #123) when the tab is not a member.
#[tauri::command]
pub(super) fn group_members(
    window: tauri::WebviewWindow,
    shell: State<'_, Shell>,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
    tab: TabId,
) -> Result<GroupMembers, String> {
    let window = calling_window(&window)?;
    let planned = tabs
        .lock()
        .group_paste_members(window, tab)
        .map_err(|refused| refused.to_string())?;
    let workspace = shell.workspace();
    let members = planned
        .members
        .into_iter()
        .map(|(member, session)| {
            let record = records.get(session);
            let safety = records.paste_safety(session, workspace.as_ref());
            let host = record
                .as_ref()
                .zip(workspace.as_ref())
                .and_then(|(record, workspace)| record.host_in(workspace).cloned());
            let connected = relays
                .reported(session)
                .and_then(|reported| reported.state)
                .is_some_and(|state| state == SessionState::Connected);
            GroupMember {
                window: member.window,
                tab: member.tab,
                session: session.0,
                name: record.map(|record| record.name).unwrap_or_default(),
                safety,
                host,
                connected,
            }
        })
        .collect();
    Ok(GroupMembers {
        revision: planned.revision,
        members,
    })
}

/// A member's lines of a confirmed group paste, as the window showing it
/// is sent them.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GroupPasteLines {
    tab: TabId,
    session: u64,
    lines: Vec<String>,
    line_delay_ms: u32,
}

/// One member's part of a group paste, as the pasting window planned it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemberLines {
    session: u64,
    lines: Vec<String>,
    line_delay_ms: u32,
}

/// Sends a group paste the calling window's tab `tab` planned at the
/// group's `revision` and the user confirmed (#87, #123): each member's
/// lines to the window showing its session now (this one included), which
/// pastes them through that member's terminal. Refused whole, nothing
/// sent, if the tab is no longer a member or the group changed since.
#[tauri::command]
pub(super) fn group_paste_send(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    tab: TabId,
    revision: u64,
    members: Vec<MemberLines>,
) -> Result<(), String> {
    let window = calling_window(&window)?;
    let sessions: Vec<SessionId> = members
        .iter()
        .map(|member| SessionId(member.session))
        .collect();
    let targets = tabs
        .lock()
        .group_paste_targets(window, tab, revision, &sessions)
        .map_err(|refused| refused.to_string())?;
    for (target, member) in targets.into_iter().zip(members) {
        app.emit_to(
            session_windows::label_for(target.window).as_str(),
            GROUP_PASTE_LINES_EVENT,
            GroupPasteLines {
                tab: target.tab,
                session: member.session,
                lines: member.lines,
                line_delay_ms: member.line_delay_ms,
            },
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Whether a group paste the calling window's tab `tab` planned at the
/// group's `revision` could still be sent (#123); refused, with the reason
/// the paste is called off, as `group_paste_send` would. The window asks
/// when it sees the group change while the paste's question is open.
#[tauri::command]
pub(super) fn group_paste_check(
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    tab: TabId,
    revision: u64,
) -> Result<(), String> {
    let window = calling_window(&window)?;
    tabs.lock()
        .group_paste_targets(window, tab, revision, &[])
        .map(|_| ())
        .map_err(|refused| refused.to_string())
}

/// What applies to the pastes of the session `id`, as `group_members`
/// gives a member's: for the question a session window asks before its
/// production sessions join the group (#87).
#[tauri::command]
pub(super) fn session_safety(
    shell: State<'_, Shell>,
    records: State<'_, SessionRecords>,
    id: u64,
) -> PasteSafety {
    records.paste_safety(SessionId(id), shell.workspace().as_ref())
}
