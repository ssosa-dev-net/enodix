//! Session windows (#84, #85): sessions moved out of the main window into
//! windows of their own, between them, and back.
//!
//! The giving window releases the session (`session_release`), waits for
//! the end of its share of the output, and serialises its terminal; then
//! `window_open_session` (into a new session window or an open one) or
//! `session_dock` / `session_dock_all` (back to the main window) moves the
//! tab from one window's layout to the other's in the core
//! (`Windows::move_session`), which closes a session window left without
//! a tab. The receiving page writes the snapshot, then attaches
//! (`session_attach`). A new window's page takes its sessions with
//! `session_window_take`; an open one is told with `session-tab-arrived` (the
//! tab) and takes it with `session_window_take_tab`; the main window is
//! told with `session-arrived` (the session, snapshot and all). Every
//! window is told the open session windows with `session-windows`.
//!
//! What is decided is the core's (`Windows`: the move, the snapshot kept
//! for a page, a reloaded page's rule, a dead window's sessions); this only
//! carries it between the pages and the session relays.
//!
//! A session window destroyed without docking back (its page could not
//! answer), or one whose page died while it stayed open (#89, `page_died`),
//! has its sessions let go of and handed to the main window without their
//! scrollback (`hand_over`); the latter is then closed. One whose page
//! leaves the shell's questions unanswered (#120, `ask_pages`, the core's
//! `PageAnswers`) is said to the main window, which offers to take its
//! sessions the same way (`session_window_take_back`), and told again once
//! it answers. The engine ending as a whole ends the app (`engine_gone`).

use std::path::PathBuf;
use std::time::Duration;

use enodix_core::layout::{
    close_workspace_sessions, live_in_session_windows, place_session_window, take_over_dead_window,
    Answered, Frame, PageAnswers, PhysicalRect, TabId, WindowId, WindowView, Windows,
    PAGE_ASK_EVERY, SESSION_WINDOW_MIN_SIZE,
};
use enodix_core::session::{
    HostKeyQuestion, LoginPrompt, NegotiatedAlgorithms, SessionId, SessionManager, SessionRecord,
    SessionRecords, SessionRelays, SessionState, Transport,
};
use enodix_core::workspace::{HostId, Workspace};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl,
    WebviewWindowBuilder,
};

use super::page_watch::Page;
use super::{
    calling_window, focused_main, product_name, window_theme, window_view, Channels,
    LoginBackupOutcome, Shell, StateMessage, Tabs, UserPreferences, MAIN_WINDOW,
};

const SESSION_WINDOW_PREFIX: &str = "session-";
/// The main window is given a session: docked back, or from a window that
/// died.
const SESSION_ARRIVED_EVENT: &str = "session-arrived";
/// An open session window is given a tab (#85); only it listens.
const SESSION_TAB_ARRIVED_EVENT: &str = "session-tab-arrived";
/// Every window is told which session windows are open (#85).
const SESSION_WINDOWS_EVENT: &str = "session-windows";
/// The main window is told the terminal used last (#86), for its Snippets
/// view.
const SNIPPET_TARGET_EVENT: &str = "snippet-target";
/// A session window is sent a snippet for one of its tabs (#86).
const SNIPPET_SEND_EVENT: &str = "snippet-send";
/// The main window is asked to do something for a session window's
/// session (#86): open the details sheet, or log in with a password.
const MAIN_WINDOW_REQUEST_EVENT: &str = "main-window-request";
/// The main window is told a session window's page stopped answering
/// (#120), or answers again.
const NOT_ANSWERING_EVENT: &str = "session-window-not-answering";
const ANSWERING_EVENT: &str = "session-window-answering";
/// The shell's question to a session window's page (#120), which it
/// answers as soon as it runs (`session_window_answering`).
const ANSWER_CHECK_EVENT: &str = "answer-check";
/// How long a new session window's page waits for its session.
const TAKE_WAIT: Duration = Duration::from_secs(10);

/// The session window labelled `label`, as the core names it.
pub(super) fn window_of(label: &str) -> Option<WindowId> {
    let id: u64 = label.strip_prefix(SESSION_WINDOW_PREFIX)?.parse().ok()?;
    (id != WindowId::MAIN.0).then_some(WindowId(id))
}

fn label_of(window: WindowId) -> String {
    format!("{SESSION_WINDOW_PREFIX}{window}")
}

/// The label of `window`, the main one's included.
pub(super) fn label_for(window: WindowId) -> String {
    if window == WindowId::MAIN {
        MAIN_WINDOW.to_owned()
    } else {
        label_of(window)
    }
}

/// Whether `window` shows `session`, and in which tab.
fn tab_showing(windows: &Windows, window: WindowId, session: SessionId) -> Option<TabId> {
    windows
        .session_tabs(window)
        .into_iter()
        .find(|&(_, shown)| shown == session)
        .map(|(tab, _)| tab)
}

/// What the app keeps of the session `id` (#84): what any window showing
/// it shows. `None` once it is discarded.
#[tauri::command]
pub(super) fn session_record(records: State<'_, SessionRecords>, id: u64) -> Option<SessionRecord> {
    records.get(SessionId(id))
}

/// The calling window gives the session `id` up (it is moving it): returns
/// the end of its share of the output, which it waits to have received
/// before serialising its terminal. Only the window showing it can.
#[tauri::command]
pub(super) fn session_release(
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    id: u64,
    consumer: u64,
) -> Result<u64, String> {
    let window = calling_window(&window)?;
    let session = SessionId(id);
    tab_showing(&tabs.lock(), window, session).ok_or("the session is not shown here")?;
    relays
        .release(
            session,
            enodix_core::session::ConsumerKey::from_raw(consumer),
        )
        .map_err(|error| error.to_string())
}

/// Moves the calling window's tab `tab`, whose session `id` it has
/// released, to another window, which shows `snapshot` (the terminal's
/// screen and scrollback) first: into the open session window `into`
/// (#85), or, without one, into a new session window. `width` and
/// `height` are a new window's inner size (logical pixels) at which its
/// terminal has the columns and rows it had; it is made to fit the screen.
/// Returns the calling window's layout; `None` once it closed, its last
/// tab gone (#85).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(super) async fn window_open_session(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    preferred: State<'_, UserPreferences>,
    tab: TabId,
    id: u64,
    snapshot: String,
    width: f64,
    height: f64,
    into: Option<WindowId>,
) -> Result<Option<WindowView>, String> {
    let from = calling_window(&window)?;
    let session = SessionId(id);
    if tab_showing(&tabs.lock(), from, session) != Some(tab) {
        return Err("the tab does not show that session".into());
    }
    match into {
        Some(target) => {
            let receiving = (target != WindowId::MAIN)
                .then(|| app.get_webview_window(&label_of(target)))
                .flatten()
                .ok_or("that session window is gone")?;
            {
                let mut windows = tabs.lock();
                let moved = windows
                    .move_session(from, tab, target)
                    .map_err(|error| error.to_string())?;
                // Its page takes it, snapshot and all, once told.
                windows.keep_snapshot(target, moved, snapshot);
                receiving
                    .emit_to(
                        receiving.label(),
                        SESSION_TAB_ARRIVED_EVENT,
                        ArrivedTab { tab: moved },
                    )
                    .map_err(|error| error.to_string())?;
            }
            let _ = receiving.unminimize();
            let _ = receiving.set_focus();
        }
        None => {
            let target = tabs.lock().open_session_window();
            let name = records
                .get(session)
                .map(|record| record.name)
                .unwrap_or_default();
            let theme = window_theme(preferred.lock().theme);
            let built = build(
                &app,
                &window,
                label_of(target),
                format!("{name} - {}", product_name(&app)),
                theme,
                (width, height),
            );
            let mut windows = tabs.lock();
            let moved = built.and_then(|built| {
                windows.move_session(from, tab, target).map_err(|error| {
                    let _ = built.destroy();
                    error.to_string()
                })
            });
            let moved = match moved {
                Ok(moved) => moved,
                Err(error) => {
                    let _ = windows.close_window(target);
                    return Err(error);
                }
            };
            // Kept with the move, under one lock: a page that finds its
            // window showing the session finds its snapshot too.
            windows.keep_snapshot(target, moved, snapshot);
        }
    }
    let shown = leave(&tabs, &window, from);
    announce(&app, &tabs, &records);
    shown
}

/// What the window `from` shows after a tab left it: its layout, or
/// `None` once its last tab has gone, the core closed it, and so does the
/// window (#85).
fn leave(
    tabs: &Tabs,
    window: &tauri::WebviewWindow,
    from: WindowId,
) -> Result<Option<WindowView>, String> {
    let view = tabs.lock().view(from);
    if view.is_none() {
        window.destroy().map_err(|error| error.to_string())?;
    }
    Ok(view)
}

/// An open session window, as the Open in window entries name it (#85).
#[derive(Clone, Serialize)]
pub(super) struct ListedWindow {
    window: WindowId,
    /// The name of the session its active tab shows; empty while unknown.
    name: String,
}

fn listed(tabs: &Tabs, records: &SessionRecords) -> Vec<ListedWindow> {
    let windows = tabs.lock().session_windows();
    windows
        .into_iter()
        .map(|(window, session)| ListedWindow {
            window,
            name: session
                .and_then(|session| records.get(session))
                .map(|record| record.name)
                .unwrap_or_default(),
        })
        .collect()
}

/// The open session windows, each named by the session its active tab
/// shows (#85).
#[tauri::command]
pub(super) fn session_windows_list(
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
) -> Vec<ListedWindow> {
    listed(&tabs, &records)
}

/// Tells every window which session windows are open now (one opened or
/// closed, or its active tab changed), with `session-windows`; and the
/// main window the terminal used last (`snippet-target`), which may have
/// moved or gone with it. If the multi-send group changed (a member's
/// window closed), every window is told its layout (#87).
pub(super) fn announce(app: &AppHandle, tabs: &Tabs, records: &SessionRecords) {
    super::group::tell_windows(app, tabs, records);
    let _ = app.emit(SESSION_WINDOWS_EVENT, listed(tabs, records));
    tell_snippet_target(app, tabs, records);
}

/// Tells the main window the terminal used last (`snippet-target`).
pub(super) fn tell_snippet_target(app: &AppHandle, tabs: &Tabs, records: &SessionRecords) {
    let _ = app.emit_to(
        MAIN_WINDOW,
        SNIPPET_TARGET_EVENT,
        snippet_target_of(tabs, records),
    );
}

/// The terminal used last, which the main window's snippets go to (#86).
#[derive(Clone, Serialize)]
pub(super) struct SnippetTarget {
    window: WindowId,
    tab: TabId,
    session: u64,
    /// Its session's name, as its tab shows it.
    name: String,
    /// Whether it is in the multi-send group (#87): a snippet sent to it
    /// goes to every member.
    member: bool,
}

fn snippet_target_of(tabs: &Tabs, records: &SessionRecords) -> Option<SnippetTarget> {
    let (used, member) = {
        let windows = tabs.lock();
        let used = windows.last_used()?;
        (used, windows.in_group(used.window, used.tab))
    };
    Some(SnippetTarget {
        window: used.window,
        tab: used.tab,
        session: used.session.0,
        member,
        name: records
            .get(used.session)
            .map(|record| record.name)
            .unwrap_or_default(),
    })
}

/// The terminal used last, in any window (#86).
#[tauri::command]
pub(super) fn snippet_target(
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
) -> Option<SnippetTarget> {
    snippet_target_of(&tabs, &records)
}

/// The calling window's terminal in the tab `tab` took the keyboard: it
/// is the one used last (#86). The main window is told when that changes.
#[tauri::command]
pub(super) fn terminal_used(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    tab: TabId,
) -> Result<(), String> {
    let window = calling_window(&window)?;
    let changed = {
        let mut windows = tabs.lock();
        let before = windows.last_used();
        windows
            .use_tab(window, tab)
            .map_err(|error| error.to_string())?;
        windows.last_used() != before
    };
    if changed {
        let _ = app.emit_to(
            MAIN_WINDOW,
            SNIPPET_TARGET_EVENT,
            snippet_target_of(&tabs, &records),
        );
    }
    Ok(())
}

/// A snippet a session window sends to one of its tabs.
#[derive(Clone, Serialize)]
struct SnippetSend {
    tab: TabId,
    name: String,
}

/// Sends the snippet `name` from the main window's Snippets view to the
/// session window `target`'s tab `tab` showing `id` (the terminal used
/// last, #86): its page fills it in and pastes it there, asking over that
/// pane what it must, and the window comes forward. Refused unless that
/// tab shows that session and it is connected.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(super) fn snippet_send_to(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    target: WindowId,
    tab: TabId,
    id: u64,
    name: String,
) -> Result<(), String> {
    if calling_window(&window)? != WindowId::MAIN {
        return Err("snippets are sent from the main window".into());
    }
    let session = SessionId(id);
    if target == WindowId::MAIN || tab_showing(&tabs.lock(), target, session) != Some(tab) {
        return Err("that terminal is no longer there".into());
    }
    let connected = relays
        .reported(session)
        .is_some_and(|reported| matches!(reported.state, Some(SessionState::Connected)));
    if !connected {
        return Err("the session is no longer connected".into());
    }
    let receiving = app
        .get_webview_window(&label_of(target))
        .ok_or("that session window is gone")?;
    receiving
        .emit_to(
            receiving.label(),
            SNIPPET_SEND_EVENT,
            SnippetSend { tab, name },
        )
        .map_err(|error| error.to_string())?;
    let _ = receiving.unminimize();
    let _ = receiving.set_focus();
    Ok(())
}

/// The host the snippets of the session `id` are filled from: its host,
/// while the open workspace is the one it came from and has it (#86).
pub(super) fn snippet_host(
    records: &SessionRecords,
    workspace: &Workspace,
    id: u64,
) -> Option<HostId> {
    records
        .get(SessionId(id))
        .and_then(|record| record.host_in(workspace).cloned())
}

/// What a session window asks of the main window for one of its sessions.
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(super) enum MainRequest {
    /// Open the details sheet on its host.
    Details,
    /// Log in to its host with a password: its SSH agent did not work.
    PasswordLogin { reason: String },
    /// Connect to the host, asking its credentials (#88: from a session
    /// window's palette, `main_window_connect`).
    Connect,
    /// Open its host's Files tab (#103), which lives in the main window.
    Files,
}

/// A request, with the session's record, as the main window is told it.
#[derive(Clone, Serialize)]
struct MainRequested {
    #[serde(flatten)]
    request: MainRequest,
    record: Option<SessionRecord>,
    /// Its host: the open workspace is the one it came from and has it
    /// (`SessionRecord::host_in`).
    host: HostId,
}

/// The calling session window asks the main window to do `request` for
/// the session of its tab `tab` (#86): the main window is told and comes
/// forward. Refused (the main window cannot help) unless the session's
/// host is in the open workspace, so the asking window keeps its tab.
#[tauri::command]
pub(super) fn main_window_request(
    app: AppHandle,
    window: tauri::WebviewWindow,
    shell: State<'_, Shell>,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    tab: TabId,
    request: MainRequest,
) -> Result<(), String> {
    let from = calling_window(&window)?;
    if from == WindowId::MAIN {
        return Err("not a session window".into());
    }
    if matches!(request, MainRequest::Connect) {
        return Err("a host is connected with main_window_connect".into());
    }
    let session = tabs
        .lock()
        .session_of(from, tab)
        .ok_or("that tab shows no session")?;
    let host = shell
        .workspace()
        .as_ref()
        .and_then(|workspace| snippet_host(&records, workspace, session.0))
        .ok_or("its host is not in the open workspace")?;
    let record = records.get(session);
    let main = focused_main(&app)?;
    main.emit_to(
        MAIN_WINDOW,
        MAIN_WINDOW_REQUEST_EVENT,
        MainRequested {
            request,
            record,
            host,
        },
    )
    .map_err(|error| error.to_string())
}

/// Builds a session window near `main`, `size` (logical) or as much of it
/// as the screen allows, with the app's own frame (#80); it has the focus.
///
/// Placed by physical position (#90): a logical one does not name one
/// monitor when monitors have different scaling. The builder only takes a
/// logical position, so the window is built hidden, placed, and shown.
fn build(
    app: &AppHandle,
    main: &tauri::WebviewWindow,
    label: String,
    title: String,
    theme: Option<tauri::Theme>,
    (width, height): (f64, f64),
) -> Result<tauri::WebviewWindow, String> {
    let monitor = main.current_monitor().ok().flatten();
    let scale = monitor
        .as_ref()
        .map(tauri::Monitor::scale_factor)
        .or_else(|| main.scale_factor().ok())
        .unwrap_or(1.0);
    let work_area = monitor.as_ref().map(|monitor| {
        let area = monitor.work_area();
        PhysicalRect {
            x: area.position.x,
            y: area.position.y,
            width: area.size.width,
            height: area.size.height,
        }
    });
    let from = main.outer_position().ok().map(|at| (at.x, at.y));
    let place = |frame| place_session_window(from, scale, work_area, (width, height), frame);
    let first = place(Frame::default());
    let (min_width, min_height) = SESSION_WINDOW_MIN_SIZE;
    let built = WebviewWindowBuilder::new(app, label, WebviewUrl::default())
        .additional_browser_args(crate::WEBVIEW_ARGUMENTS)
        .use_https_scheme(crate::HTTPS_SCHEME)
        .general_autofill_enabled(false)
        .title(title)
        .inner_size(first.width, first.height)
        .min_inner_size(min_width, min_height)
        .theme(theme)
        .decorations(false)
        .shadow(true)
        .visible(false)
        .build()
        .inspect(watch_page)
        .map_err(|error| format!("the window could not be opened: {error}"))?;
    // Moved to its monitor first: on one of another scale its scale
    // changes, so its size is set again at that scale. (This counts on
    // the move taking the new scale before it returns: on Windows the
    // move sends WM_DPICHANGED, which tao handles there and then, hidden
    // or not; checked for #90 at 100% and 150%.) Its frame (the invisible
    // border of a window with a shadow) is known there; it is then placed
    // so that what is seen fits the work area.
    let shown = built
        .set_position(PhysicalPosition::new(first.x, first.y))
        .and_then(|()| built.set_size(LogicalSize::new(first.width, first.height)))
        .and_then(|()| Ok((built.outer_position()?, built.inner_position()?)))
        .and_then(|(outer, inner)| {
            let frame = Frame {
                left: u32::try_from(inner.x - outer.x).unwrap_or(0),
                top: u32::try_from(inner.y - outer.y).unwrap_or(0),
            };
            let fitted = place(frame);
            built.set_position(PhysicalPosition::new(fitted.x, fitted.y))
        })
        .and_then(|()| built.show());
    if let Err(error) = shown {
        let _ = built.destroy();
        return Err(format!("the window could not be placed: {error}"));
    }
    let _ = built.set_focus();
    Ok(built)
}

/// A session a session window's page takes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TakenTab {
    session: u64,
    tab: TabId,
    /// `None`: the page was reloaded, and what it showed is lost.
    snapshot: Option<String>,
    record: Option<SessionRecord>,
}

impl TakenTab {
    fn of(taken: enodix_core::layout::Taken, records: &SessionRecords) -> Self {
        Self {
            session: taken.session.0,
            tab: taken.tab,
            record: records.get(taken.session),
            snapshot: taken.snapshot,
        }
    }
}

/// What a session window's page starts from: its sessions and layout.
#[derive(Serialize)]
pub(super) struct Taken {
    sessions: Vec<TakenTab>,
    layout: WindowView,
}

/// The calling session window's sessions, once one was moved there (the
/// page may load first: it waits a little), as `Windows::take_sessions`
/// gives them. Without a snapshot (the page reloaded), whoever held a
/// session (the page before) is let go of.
#[tauri::command]
pub(super) async fn session_window_take(
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
) -> Result<Taken, String> {
    let window = calling_window(&window)?;
    let started = std::time::Instant::now();
    loop {
        {
            let mut windows = tabs.lock();
            let taken = windows.take_sessions(window);
            if !taken.is_empty() {
                for each in taken.iter().filter(|each| each.snapshot.is_none()) {
                    relays.let_go(each.session);
                }
                return Ok(Taken {
                    sessions: taken
                        .into_iter()
                        .map(|each| TakenTab::of(each, &records))
                        .collect(),
                    layout: window_view(&windows, window)?,
                });
            }
        }
        if started.elapsed() > TAKE_WAIT {
            return Err("this window was given no session".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A tab moved into a session window whose page is open (#85).
#[derive(Clone, Serialize)]
struct ArrivedTab {
    tab: TabId,
}

/// A tab that moved into the calling session window while its page was
/// open (told by `session-tab-arrived`), and the window's layout.
#[derive(Serialize)]
pub(super) struct TakenOne {
    session: TakenTab,
    layout: WindowView,
}

/// The session of the tab `tab` moved into the calling session window
/// while its page was open, with its snapshot (used up).
#[tauri::command]
pub(super) fn session_window_take_tab(
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    tab: TabId,
) -> Result<TakenOne, String> {
    let window = calling_window(&window)?;
    let mut windows = tabs.lock();
    let taken = windows
        .take_tab(window, tab)
        .ok_or("this window does not show that tab")?;
    Ok(TakenOne {
        session: TakenTab::of(taken, &records),
        layout: window_view(&windows, window)?,
    })
}

/// A session the calling window takes over.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Attached {
    /// The key that acknowledges its output.
    consumer: u64,
    /// The output delivered before this window's share, in bytes.
    offset: u64,
    encrypted: bool,
    transport: Transport,
    /// Where it stands, before the events that waited for this window;
    /// `None`: none was shown yet.
    state: Option<StateMessage>,
    /// What it reported to the windows before (#86), as their events said
    /// it: the agreed algorithms, why its log failed, its backup outcome.
    algorithms: Option<Vec<NegotiatedAlgorithms>>,
    log_failure: Option<String>,
    backup: Option<LoginBackupOutcome>,
}

/// The calling window is shown the session `id` from now on: it must show
/// it in a tab (it was moved there) and nobody else may hold it. The events
/// that waited come first.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(super) async fn session_attach(
    app: AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, SessionManager>,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    id: u64,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
    on_backup: Channel<LoginBackupOutcome>,
) -> Result<Attached, String> {
    let window = calling_window(&window)?;
    let session = SessionId(id);
    tab_showing(&tabs.lock(), window, session).ok_or("the session is not shown here")?;
    let encrypted = manager
        .encrypted(session)
        .map_err(|error| error.to_string())?;
    let transport = manager
        .transport(session)
        .map_err(|error| error.to_string())?;
    // Nobody holds it, so nothing is delivered between the two.
    let reported = relays.reported(session).unwrap_or_default();
    let attached = relays
        .attach(
            session,
            window,
            Channels {
                session,
                output: on_output,
                state: on_state,
                prompt: on_prompt,
                host_key: on_host_key,
                algorithms: on_algorithms,
                log_failure: on_log_failure,
                backup: on_backup,
                app,
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(Attached {
        consumer: attached.key.get(),
        offset: attached.offset,
        encrypted,
        transport,
        state: reported.state.map(StateMessage::from),
        algorithms: reported.algorithms,
        log_failure: reported.log_failure.map(|failure| failure.to_string()),
        backup: reported.backup,
    })
}

/// A session the main window is given, in its tab `tab`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Arrival {
    session: u64,
    tab: TabId,
    /// The terminal as the window giving it showed it; `None`: that window
    /// died, and its output so far is lost.
    snapshot: Option<String>,
    record: Option<SessionRecord>,
    /// The main window's layout, with the tab.
    layout: WindowView,
}

/// A session tab a session window gives back to the main window: its
/// session `id`, released, and `snapshot`, its terminal.
#[derive(Deserialize)]
pub(super) struct Docking {
    tab: TabId,
    id: u64,
    snapshot: String,
}

/// Docks the calling session window's tabs back into the main window, in
/// order; the main window takes the focus and is told of each
/// (`session-arrived`). A window left without a tab closes (#85): its
/// layout is returned until then.
fn dock(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    tabs: &Tabs,
    records: &SessionRecords,
    docking: Vec<Docking>,
) -> Result<Option<WindowView>, String> {
    let from = calling_window(window)?;
    if from == WindowId::MAIN {
        return Err("the main window docks nothing".into());
    }
    let main = focused_main(app)?;
    let arrivals = {
        let mut windows = tabs.lock();
        if docking
            .iter()
            .any(|each| tab_showing(&windows, from, SessionId(each.id)) != Some(each.tab))
        {
            return Err("the tab does not show that session".into());
        }
        let mut moved = Vec::new();
        for each in docking {
            let back = windows
                .move_session(from, each.tab, WindowId::MAIN)
                .map_err(|error| error.to_string())?;
            moved.push((each.id, back, each.snapshot));
        }
        let layout = window_view(&windows, WindowId::MAIN)?;
        moved
            .into_iter()
            .map(|(id, tab, snapshot)| Arrival {
                session: id,
                tab,
                snapshot: Some(snapshot),
                record: records.get(SessionId(id)),
                layout: layout.clone(),
            })
            .collect::<Vec<_>>()
    };
    for arrival in arrivals {
        main.emit_to(MAIN_WINDOW, SESSION_ARRIVED_EVENT, arrival)
            .map_err(|error| error.to_string())?;
    }
    let shown = leave(tabs, window, from);
    announce(app, tabs, records);
    shown
}

/// Docks the calling session window's tab `tab` back into the main window
/// (its session `id` released, `snapshot` its terminal); the window
/// closes if that was its last tab.
#[tauri::command]
pub(super) async fn session_dock(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    tab: TabId,
    id: u64,
    snapshot: String,
) -> Result<Option<WindowView>, String> {
    dock(
        &app,
        &window,
        &tabs,
        &records,
        vec![Docking { tab, id, snapshot }],
    )
}

/// Dock all back (#85): every given tab of the calling session window goes
/// back to the main window, in order, and the window closes once none is
/// left.
#[tauri::command]
pub(super) async fn session_dock_all(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    docking: Vec<Docking>,
) -> Result<Option<WindowView>, String> {
    dock(&app, &window, &tabs, &records, docking)
}

/// Closes the calling session window and its sessions: its last tab was
/// closed.
#[tauri::command]
pub(super) async fn session_window_close(
    app: AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, SessionManager>,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
) -> Result<(), String> {
    let closing = calling_window(&window)?;
    if closing == WindowId::MAIN {
        return Err("not a session window".into());
    }
    let sessions = {
        let mut windows = tabs.lock();
        let sessions = windows.session_tabs(closing);
        windows
            .close_window(closing)
            .map_err(|error| error.to_string())?;
        sessions
    };
    for (_, session) in sessions {
        relays.forget(session);
        manager.discard(session);
        records.forget(session);
    }
    announce(&app, &tabs, &records);
    window.destroy().map_err(|error| error.to_string())
}

/// How many sessions of the session windows are live, only those to hosts
/// of the workspace at `workspace` when given (#88, the core's
/// `live_in_session_windows`): closing the main window asks about every
/// window's, switching workspace about those of the workspace left. The
/// main window counts its own.
#[tauri::command]
pub(super) fn session_windows_live(
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
    workspace: Option<PathBuf>,
) -> usize {
    live_in_session_windows(&tabs.lock(), &relays, &records, workspace.as_deref())
}

/// The main window switched away from the workspace at `workspace`, and
/// the user chose to close its sessions (#88): in every session window,
/// each session to one of its hosts closes (the core's
/// `close_workspace_sessions`); the windows left without a tab are
/// destroyed, the others told their layout. Main only: it closes its own
/// tabs.
#[tauri::command]
pub(super) fn session_windows_close_workspace(
    app: AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, SessionManager>,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
    workspace: PathBuf,
) -> Result<(), String> {
    if calling_window(&window)? != WindowId::MAIN {
        return Err("only the main window switches workspace".into());
    }
    let (closed, views) = {
        let mut windows = tabs.lock();
        let done = close_workspace_sessions(&mut windows, &manager, &relays, &records, &workspace);
        let views: Vec<_> = done
            .changed
            .into_iter()
            .filter_map(|window| Some((window, windows.view(window)?)))
            .collect();
        (done.closed, views)
    };
    for (window, view) in views {
        let _ = app.emit_to(
            label_of(window).as_str(),
            super::group::LAYOUT_CHANGED_EVENT,
            view,
        );
    }
    for window in closed {
        if let Some(webview) = app.get_webview_window(&label_of(window)) {
            let _ = webview.destroy();
        }
    }
    announce(&app, &tabs, &records);
    Ok(())
}

/// The main window closes (#88), its question answered or none needed:
/// every window goes, and their sessions with them (`close_detached`, on
/// its `Destroyed`).
#[tauri::command]
pub(super) fn main_window_close(window: tauri::WebviewWindow) -> Result<(), String> {
    if calling_window(&window)? != WindowId::MAIN {
        return Err("not the main window".into());
    }
    window.destroy().map_err(|error| error.to_string())
}

/// From a session window's palette (#88): the host `host` needs its
/// credentials asked, which only the main window does (it has the
/// credentials prompt), so the main window is asked to connect it and comes
/// forward. The session opens there.
#[tauri::command]
pub(super) fn main_window_connect(
    app: AppHandle,
    window: tauri::WebviewWindow,
    shell: State<'_, Shell>,
    host: HostId,
) -> Result<(), String> {
    if calling_window(&window)? == WindowId::MAIN {
        return Err("not a session window".into());
    }
    let known = shell
        .workspace()
        .as_ref()
        .is_some_and(|workspace| workspace.resolve_host(&host).is_some());
    if !known {
        return Err("that host is not in the open workspace".into());
    }
    let main = focused_main(&app)?;
    main.emit_to(
        MAIN_WINDOW,
        MAIN_WINDOW_REQUEST_EVENT,
        MainRequested {
            request: MainRequest::Connect,
            record: None,
            host,
        },
    )
    .map_err(|error| error.to_string())
}

/// The session window labelled `label` was destroyed. If it still showed
/// sessions (it did not dock them back: its page could not answer), they
/// are handed to the main window (`hand_over`). With the main window gone
/// too (the app is closing), they are discarded.
pub(super) fn destroyed(app: &AppHandle, label: &str) {
    let Some(window) = window_of(label) else {
        return;
    };
    if app.get_webview_window(MAIN_WINDOW).is_none() {
        let tabs = app.state::<Tabs>();
        let relays = app.state::<SessionRelays>();
        let records = app.state::<SessionRecords>();
        let sessions = {
            let mut windows = tabs.lock();
            let sessions = windows.session_tabs(window);
            let _ = windows.close_window(window);
            sessions
        };
        for (_, session) in sessions {
            relays.forget(session);
            app.state::<SessionManager>().discard(session);
            records.forget(session);
        }
        return;
    }
    hand_over(app, window);
}

/// The session window `window`'s page died while its window stayed open
/// (#89: its renderer crashed or was killed; a channel into it still says
/// `Ok`, so nothing else notices). Its sessions go to the main window
/// (`hand_over`) and the window is closed: a window the shell took
/// sessions from is never left showing them, since its terminals could
/// still type to the devices.
fn page_died(app: &AppHandle, window: WindowId) {
    hand_over(app, window);
    if let Some(webview) = app.get_webview_window(&label_of(window)) {
        let _ = webview.destroy();
    }
}

/// A session window, as the main window is told it stopped answering or
/// answers again (#120): the names of the sessions it shows.
#[derive(Clone, Serialize)]
struct Answering {
    window: WindowId,
    names: Vec<String>,
}

fn answering(app: &AppHandle, window: WindowId) -> Option<Answering> {
    let records = app.state::<SessionRecords>();
    let sessions = app.state::<Tabs>().lock().session_tabs(window);
    if sessions.is_empty() {
        return None;
    }
    Some(Answering {
        window,
        names: sessions
            .into_iter()
            .map(|(_, session)| {
                records
                    .get(session)
                    .map(|record| record.name)
                    .unwrap_or_default()
            })
            .collect(),
    })
}

/// The questions asked of the session windows' pages (#120, the core's
/// `PageAnswers`).
#[derive(Default)]
pub(super) struct Answers(std::sync::Mutex<PageAnswers>);

impl Answers {
    fn lock(&self) -> std::sync::MutexGuard<'_, PageAnswers> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Asks every session window's page a question every `PAGE_ASK_EVERY`
/// (`answer-check`, which it answers with `session_window_answering`), for
/// as long as the app runs; a page that leaves one unanswered for
/// `PAGE_ANSWER_WAIT` is said to the main window (`page_not_answering`).
pub(super) fn ask_pages(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(PAGE_ASK_EVERY).await;
            let open: Vec<(WindowId, tauri::WebviewWindow)> = app
                .webview_windows()
                .into_iter()
                .filter_map(|(label, webview)| Some((window_of(&label)?, webview)))
                .collect();
            let silent = {
                let state = app.state::<Answers>();
                let mut answers = state.lock();
                answers.keep_only(&open.iter().map(|(id, _)| *id).collect::<Vec<_>>());
                let now = std::time::Instant::now();
                for (id, _) in &open {
                    answers.ask(*id, now);
                }
                answers.not_answering(now)
            };
            for (_, webview) in &open {
                let _ = webview.emit_to(webview.label(), ANSWER_CHECK_EVENT, ());
            }
            for window in silent {
                page_not_answering(&app, window);
            }
        }
    });
}

/// The session window `window`'s page left the shell's question
/// unanswered for `PAGE_ANSWER_WAIT` (#120): it hangs, or is very busy. It
/// may recover, so nothing is taken from it: the main window is told and
/// offers to take its sessions (`session_window_take_back`). Its sessions
/// meanwhile stay where they are, paused by flow control once their output
/// goes unacknowledged.
fn page_not_answering(app: &AppHandle, window: WindowId) {
    let Some(said) = answering(app, window) else {
        return;
    };
    if let Some(main) = app.get_webview_window(MAIN_WINDOW) {
        let _ = main.emit_to(MAIN_WINDOW, NOT_ANSWERING_EVENT, said);
    }
}

/// The calling session window's page answers the shell's question
/// (#120); if it had been taken for not answering, the main window is told
/// it answers again.
#[tauri::command]
pub(super) fn session_window_answering(
    app: AppHandle,
    window: tauri::WebviewWindow,
    answers: State<'_, Answers>,
) -> Result<(), String> {
    let from = calling_window(&window)?;
    if from == WindowId::MAIN {
        return Err("not a session window".into());
    }
    if answers.lock().answered(from) == Answered::AsBefore {
        return Ok(());
    }
    let said = answering(&app, from).ok_or("this window shows no session")?;
    let main = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("the main window is gone")?;
    main.emit_to(MAIN_WINDOW, ANSWERING_EVENT, said)
        .map_err(|error| error.to_string())
}

/// From the main window (#120): the session window `target`, whose page
/// does not answer, gives its sessions to the main window as a dead one
/// does (without their earlier output, `hand_over`) and is closed.
#[tauri::command]
pub(super) fn session_window_take_back(
    app: AppHandle,
    window: tauri::WebviewWindow,
    target: WindowId,
) -> Result<(), String> {
    if calling_window(&window)? != WindowId::MAIN {
        return Err("only the main window takes sessions back".into());
    }
    if target == WindowId::MAIN || app.get_webview_window(&label_of(target)).is_none() {
        return Err("that session window is gone".into());
    }
    page_died(&app, target);
    Ok(())
}

/// Takes the session window `window`'s sessions away from it (the core's
/// `take_over_dead_window`: moved to the main window, whoever held them let
/// go of) and tells the main window of each (`session-arrived`, with the
/// snapshot its page never took, or none: its output so far is lost).
/// Nothing if it is gone already (docked back, closed, or taken over).
fn hand_over(app: &AppHandle, window: WindowId) {
    let tabs = app.state::<Tabs>();
    let relays = app.state::<SessionRelays>();
    let records = app.state::<SessionRecords>();
    let (handed, layout) = {
        let mut windows = tabs.lock();
        let Ok(handed) = take_over_dead_window(&mut windows, &relays, window) else {
            return;
        };
        (handed, windows.view(WindowId::MAIN))
    };
    announce(app, &tabs, &records);
    let (Some(main), Some(layout)) = (app.get_webview_window(MAIN_WINDOW), layout) else {
        return;
    };
    for taken in handed {
        let _ = main.emit_to(
            MAIN_WINDOW,
            SESSION_ARRIVED_EVENT,
            Arrival {
                session: taken.session.0,
                tab: taken.tab,
                snapshot: taken.snapshot,
                record: records.get(taken.session),
                layout: layout.clone(),
            },
        );
    }
}

/// Watches the page of the session window `webview` (#89, #120): once its
/// renderer is gone, `page_died`; once the engine is gone, `engine_gone`.
/// Windows: WebView2's `ProcessFailed`. Linux: WebKitGTK's
/// `web-process-terminated` (as the page's own). Elsewhere nothing is
/// watched. A page that hangs or is only slow raises neither: the shell's
/// questions find one that hangs (`ask_pages`).
fn watch_page(webview: &tauri::WebviewWindow) {
    let Some(window) = window_of(webview.label()) else {
        return;
    };
    let app = webview.app_handle().clone();
    let died = move |page| {
        let app = app.clone();
        // Not from inside the webview's own event: the window is destroyed.
        tauri::async_runtime::spawn(async move {
            match page {
                Page::Gone => page_died(&app, window),
                Page::EngineGone => engine_gone(&app),
            }
        });
    };
    let _ = webview.with_webview(move |platform| super::page_watch::watch(platform, died));
}

/// Watches the main window's page for the engine's end (#120): its own
/// renderer's end is not acted on (its sessions have nowhere to go).
pub(super) fn watch_main(webview: &tauri::WebviewWindow) {
    let app = webview.app_handle().clone();
    let died = move |page| {
        if page == Page::EngineGone {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { engine_gone(&app) });
        }
    };
    let _ = webview.with_webview(move |platform| super::page_watch::watch(platform, died));
}

/// The webview engine's browser process is gone (#120), and every window's
/// page with it: no session can be shown anywhere again, and no page is
/// left to close a window (a window's close waits for its page, so the app
/// could only be killed). The app ends as when its main window closes
/// (`end_with_the_app`); each session's log has every line it completed.
fn engine_gone(app: &AppHandle) {
    eprintln!("the web engine ended: the app ends");
    app.exit(0);
}
