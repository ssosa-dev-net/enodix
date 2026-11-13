//! The Tauri shell: a thin adapter that exposes `netterm-core` to the
//! interface. No behaviour lives here; it only translates between the
//! interface's messages and the core's session interface.

use netterm_core::session::{
    SessionEvent, SessionId, SessionManager, SessionState, SessionTarget, SshTarget, TerminalSize,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, State, WebviewUrl, WebviewWindowBuilder};

/// The quick-connect form as the interface sends it.
#[derive(Deserialize)]
struct QuickConnect {
    host: String,
    port: u16,
    username: String,
    password: String,
}

#[derive(Deserialize, Clone, Copy)]
struct Size {
    cols: u32,
    rows: u32,
}

impl From<Size> for TerminalSize {
    fn from(size: Size) -> Self {
        Self {
            cols: size.cols,
            rows: size.rows,
        }
    }
}

/// A session state as the interface receives it.
#[derive(Serialize, Clone)]
#[serde(tag = "state", rename_all = "camelCase")]
enum StateMessage {
    Connecting,
    Connected,
    Closed { reason: String },
}

impl From<SessionState> for StateMessage {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Connecting => Self::Connecting,
            SessionState::Connected => Self::Connected,
            SessionState::Closed(reason) => Self::Closed {
                reason: reason.to_string(),
            },
        }
    }
}

/// The user-visible app name. It is defined once, as `productName` in
/// `tauri.conf.json`; the window title and the interface both read it here.
fn product_name(app: &AppHandle) -> String {
    app.config()
        .product_name
        .clone()
        .unwrap_or_else(|| app.package_info().name.clone())
}

#[tauri::command]
fn app_name(app: AppHandle) -> String {
    product_name(&app)
}

/// Opens a session. Output arrives on `on_output` as raw bytes and state
/// changes on `on_state`; terminal data never goes through the event system.
#[tauri::command]
async fn session_open(
    manager: State<'_, SessionManager>,
    target: QuickConnect,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
) -> Result<u64, String> {
    let target = SessionTarget::Ssh(SshTarget {
        host: target.host,
        port: target.port,
        username: target.username,
        password: target.password,
    });
    let (id, mut events) = manager.open(target, size.into());
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            let delivered = match event {
                SessionEvent::Output(bytes) => on_output.send(InvokeResponseBody::Raw(bytes)),
                SessionEvent::State(state) => on_state.send(state.into()),
            };
            // The webview is gone; dropping the events closes the session.
            if delivered.is_err() {
                break;
            }
        }
    });
    Ok(id.0)
}

#[tauri::command]
fn session_write(manager: State<'_, SessionManager>, id: u64, data: Vec<u8>) -> Result<(), String> {
    manager
        .write(SessionId(id), data)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_resize(manager: State<'_, SessionManager>, id: u64, size: Size) -> Result<(), String> {
    manager
        .resize(SessionId(id), size.into())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn session_close(manager: State<'_, SessionManager>, id: u64) -> Result<(), String> {
    manager
        .close(SessionId(id))
        .map_err(|error| error.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .manage(SessionManager::new())
        .setup(|app| {
            let title = product_name(app.handle());
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title(title)
                .inner_size(1100.0, 720.0)
                .min_inner_size(480.0, 320.0)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_name,
            session_open,
            session_write,
            session_resize,
            session_close
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
