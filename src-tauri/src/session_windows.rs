//! Session windows (#84): a session moved out of the main window into a
//! window of its own, and back.
//!
//! The giving window releases the session (`session_release`), waits for
//! the end of its share of the output, and serialises its terminal; then
//! `window_open_session` (from the main window) or `session_dock` (from a
//! session window) moves the tab from one window's layout to the other's
//! in the core (`Windows::move_session`). The receiving page writes the
//! snapshot, then attaches (`session_attach`). A new window's page takes
//! its session with `session_window_take`; the main window is told with
//! `session-arrived`.
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
    SessionRecords, SessionRelays, Transport,
};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use super::{
    calling_window, focused_main, product_name, window_theme, window_view, Channels,
    LoginBackupOutcome, StateMessage, Tabs, UserPreferences, MAIN_WINDOW,
};

const SESSION_WINDOW_PREFIX: &str = "session-";
/// The main window is given a session: docked back, or from a window that
/// died.
const SESSION_ARRIVED_EVENT: &str = "session-arrived";
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

/// Moves the main window's tab `tab`, whose session `id` it has released,
/// to a new session window, which shows `snapshot` (the terminal's screen
/// and scrollback) first. `width` and `height` are the window's inner size
/// (logical pixels) at which its terminal has the columns and rows it had;
/// it is made to fit the screen. Returns the main window's layout.
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
) -> Result<WindowView, String> {
    if calling_window(&window)? != WindowId::MAIN {
        return Err("only the main window opens a session window".into());
    }
    let session = SessionId(id);
    let target = {
        let mut windows = tabs.lock();
        if tab_showing(&windows, WindowId::MAIN, session) != Some(tab) {
            return Err("the tab does not show that session".into());
        }
        windows.open_session_window()
    };
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
        windows
            .move_session(WindowId::MAIN, tab, target)
            .map_err(|error| {
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
    // Kept with the move, under one lock: a page that finds its window
    // showing the session finds its snapshot too.
    windows.keep_snapshot(target, moved, snapshot);
    window_view(&windows, WindowId::MAIN)
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

/// What a session window's page starts from.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Taken {
    session: u64,
    tab: TabId,
    /// `None`: the page was reloaded, and what it showed is lost.
    snapshot: Option<String>,
    record: Option<SessionRecord>,
    layout: WindowView,
}

/// The calling session window's session, once it was moved there (the
/// page may load first: it waits a little), as `Windows::take_session`
/// gives it. Without a snapshot (the page reloaded), whoever held the
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
            if let Some(taken) = windows.take_session(window) {
                if taken.snapshot.is_none() {
                    relays.let_go(taken.session);
                }
                return Ok(Taken {
                    session: taken.session.0,
                    tab: taken.tab,
                    snapshot: taken.snapshot,
                    record: records.get(taken.session),
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
    let state = relays.state(session).map(StateMessage::from);
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
        state,
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

/// Docks the calling session window's tab `tab` back into the main window
/// (its session `id` released, `snapshot` its terminal), which takes the
/// focus; the session window closes.
#[tauri::command]
pub(super) async fn session_dock(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    tab: TabId,
    id: u64,
    snapshot: String,
) -> Result<(), String> {
    let from = calling_window(&window)?;
    if from == WindowId::MAIN {
        return Err("the main window docks nothing".into());
    }
    let session = SessionId(id);
    let main = focused_main(&app)?;
    let arrival = {
        let mut windows = tabs.lock();
        if tab_showing(&windows, from, session) != Some(tab) {
            return Err("the tab does not show that session".into());
        }
        let back = windows
            .move_session(from, tab, WindowId::MAIN)
            .map_err(|error| error.to_string())?;
        // Its only tab has gone: so does the window, before it is destroyed
        // (which then finds nothing to hand over).
        let _ = windows.close_window(from);
        Arrival {
            session: id,
            tab: back,
            snapshot: Some(snapshot),
            record: records.get(session),
            layout: window_view(&windows, WindowId::MAIN)?,
        }
    };
    main.emit_to(MAIN_WINDOW, SESSION_ARRIVED_EVENT, arrival)
        .map_err(|error| error.to_string())?;
    window.destroy().map_err(|error| error.to_string())
}

/// Closes the calling session window and its session: its tab was closed.
#[tauri::command]
pub(super) async fn session_window_close(
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
