//! The Tauri shell: a thin adapter that exposes `netterm-core` to the
//! interface. No behaviour lives here; it only translates between the
//! interface's messages and the core's workspace and session interfaces.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use netterm_core::app_state::AppState;
use netterm_core::session::{
    SessionEvent, SessionId, SessionManager, SessionState, SessionTarget, SshTarget, TerminalSize,
};
use netterm_core::workspace::{Folder, HostId, Problem, Workspace};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// Overrides the per-user application data folder (the smoke test uses it to
/// start from a known state).
const DATA_DIR_VARIABLE: &str = "NETTERM_DATA_DIR";

/// Where app-level state lives, and the open workspace.
struct Shell {
    data_folder: PathBuf,
    workspace: Mutex<Option<Workspace>>,
}

impl Shell {
    fn workspace(&self) -> MutexGuard<'_, Option<Workspace>> {
        self.workspace
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// What to connect to, as the interface sends it.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ConnectTarget {
    /// The quick-connect form.
    Quick {
        host: String,
        port: u16,
        username: String,
        password: String,
    },
    /// A host of the open workspace, with the credentials prompted for.
    Host {
        id: HostId,
        username: String,
        password: String,
    },
}

/// An open workspace as the interface receives it.
#[derive(Serialize)]
struct WorkspaceView {
    root: String,
    tree: Folder,
    problems: Vec<Problem>,
}

impl From<&Workspace> for WorkspaceView {
    fn from(workspace: &Workspace) -> Self {
        Self {
            root: workspace.root().display().to_string(),
            tree: workspace.tree().clone(),
            problems: workspace.problems().to_vec(),
        }
    }
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

/// Opens the workspace folder at `path` and remembers it for the next launch.
#[tauri::command]
fn workspace_open(shell: State<'_, Shell>, path: PathBuf) -> Result<WorkspaceView, String> {
    let workspace = Workspace::open(&path).map_err(|error| error.to_string())?;
    let view = WorkspaceView::from(&workspace);
    *shell.workspace() = Some(workspace);
    let state = AppState {
        last_workspace: Some(path),
    };
    state
        .save(&shell.data_folder)
        .map_err(|error| format!("the workspace opened, but could not be remembered: {error}"))?;
    Ok(view)
}

/// Reopens the workspace that was open last, if there was one.
#[tauri::command]
fn workspace_restore(shell: State<'_, Shell>) -> Result<Option<WorkspaceView>, String> {
    let Some(path) = AppState::load(&shell.data_folder).last_workspace else {
        return Ok(None);
    };
    let workspace = Workspace::open(&path).map_err(|error| error.to_string())?;
    let view = WorkspaceView::from(&workspace);
    *shell.workspace() = Some(workspace);
    Ok(Some(view))
}

/// Opens a session. Output arrives on `on_output` as raw bytes, already
/// batched by the core, and state changes on `on_state`; terminal data never
/// goes through the event system. The interface acknowledges output with
/// `session_acknowledge`.
#[tauri::command]
async fn session_open(
    manager: State<'_, SessionManager>,
    shell: State<'_, Shell>,
    target: ConnectTarget,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
) -> Result<u64, String> {
    let target = SessionTarget::Ssh(match target {
        ConnectTarget::Quick {
            host,
            port,
            username,
            password,
        } => SshTarget {
            host,
            port,
            username,
            password,
        },
        ConnectTarget::Host {
            id,
            username,
            password,
        } => shell
            .workspace()
            .as_ref()
            .and_then(|workspace| workspace.host(&id))
            .ok_or_else(|| format!("{id} is not in the open workspace"))?
            .ssh_target(username, password),
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

/// The terminal has consumed `bytes` bytes of the session's output. Without
/// these, the core stops reading from the device (flow control).
#[tauri::command]
fn session_acknowledge(
    manager: State<'_, SessionManager>,
    id: u64,
    bytes: usize,
) -> Result<(), String> {
    manager
        .acknowledge(SessionId(id), bytes)
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
        .plugin(tauri_plugin_dialog::init())
        .manage(SessionManager::new())
        .setup(|app| {
            let data_folder = match std::env::var_os(DATA_DIR_VARIABLE) {
                Some(folder) => PathBuf::from(folder),
                None => app.path().app_data_dir()?,
            };
            app.manage(Shell {
                data_folder,
                workspace: Mutex::new(None),
            });
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
            workspace_open,
            workspace_restore,
            session_open,
            session_write,
            session_acknowledge,
            session_resize,
            session_close
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
