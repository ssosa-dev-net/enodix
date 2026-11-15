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
//! answer) has its sessions let go of and handed to the main window
//! without their scrollback (`destroyed`).

use std::time::Duration;

use netterm_core::layout::{TabId, WindowId, WindowView, Windows};
use netterm_core::session::{
    HostKeyQuestion, LoginPrompt, NegotiatedAlgorithms, SessionId, SessionManager, SessionRecord,
    SessionRecords, SessionRelays, SessionState, Transport,
};
use netterm_core::workspace::{HostId, Workspace};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

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
/// How long a new session window's page waits for its session.
const TAKE_WAIT: Duration = Duration::from_secs(10);
/// Where a session window opens, from the main window's corner.
const OPEN_OFFSET: f64 = 40.0;
const MIN_WIDTH: f64 = 420.0;
const MIN_HEIGHT: f64 = 320.0;

/// The session window labelled `label`, as the core names it.
pub(super) fn window_of(label: &str) -> Option<WindowId> {
    let id: u64 = label.strip_prefix(SESSION_WINDOW_PREFIX)?.parse().ok()?;
    (id != WindowId::MAIN.0).then_some(WindowId(id))
}

fn label_of(window: WindowId) -> String {
    format!("{SESSION_WINDOW_PREFIX}{window}")
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
            netterm_core::session::ConsumerKey::from_raw(consumer),
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
/// moved or gone with it.
pub(super) fn announce(app: &AppHandle, tabs: &Tabs, records: &SessionRecords) {
    let _ = app.emit(SESSION_WINDOWS_EVENT, listed(tabs, records));
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
}

fn snippet_target_of(tabs: &Tabs, records: &SessionRecords) -> Option<SnippetTarget> {
    let used = tabs.lock().last_used()?;
    Some(SnippetTarget {
        window: used.window,
        tab: used.tab,
        session: used.session.0,
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
fn build(
    app: &AppHandle,
    main: &tauri::WebviewWindow,
    label: String,
    title: String,
    theme: Option<tauri::Theme>,
    (width, height): (f64, f64),
) -> Result<tauri::WebviewWindow, String> {
    let mut width = width.max(MIN_WIDTH);
    let mut height = height.max(MIN_HEIGHT);
    let scale = main.scale_factor().unwrap_or(1.0);
    let (mut x, mut y) = main
        .outer_position()
        .map(|at| {
            let at = at.to_logical::<f64>(scale);
            (at.x + OPEN_OFFSET, at.y + OPEN_OFFSET)
        })
        .unwrap_or((OPEN_OFFSET, OPEN_OFFSET));
    if let Ok(Some(monitor)) = main.current_monitor() {
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let (left, top) = (
            f64::from(area.position.x) / scale,
            f64::from(area.position.y) / scale,
        );
        let (across, down) = (
            f64::from(area.size.width) / scale,
            f64::from(area.size.height) / scale,
        );
        width = width.min(across);
        height = height.min(down);
        x = x.min(left + across - width).max(left);
        y = y.min(top + down - height).max(top);
    }
    WebviewWindowBuilder::new(app, label, WebviewUrl::default())
        .title(title)
        .inner_size(width, height)
        .position(x, y)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .theme(theme)
        .decorations(false)
        .shadow(true)
        .focused(true)
        .build()
        .map_err(|error| format!("the window could not be opened: {error}"))
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
    fn of(taken: netterm_core::layout::Taken, records: &SessionRecords) -> Self {
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

/// The session window labelled `label` was destroyed. If it still showed
/// sessions (it did not dock them back: its page could not answer), the
/// core hands them to the main window (`Windows::hand_to_main`), whoever
/// held them is let go of, and the main window is told. With the main
/// window gone too (the app is closing), they are discarded.
pub(super) fn destroyed(app: &AppHandle, label: &str) {
    let Some(window) = window_of(label) else {
        return;
    };
    let tabs = app.state::<Tabs>();
    let relays = app.state::<SessionRelays>();
    let records = app.state::<SessionRecords>();
    let Some(main) = app.get_webview_window(MAIN_WINDOW) else {
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
    };
    let (handed, layout) = {
        let mut windows = tabs.lock();
        let Ok(handed) = windows.hand_to_main(window) else {
            // Docked back or closed already.
            return;
        };
        (handed, windows.view(WindowId::MAIN))
    };
    announce(app, &tabs, &records);
    let Some(layout) = layout else {
        return;
    };
    for taken in handed {
        relays.let_go(taken.session);
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
