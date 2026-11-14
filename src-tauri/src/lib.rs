//! The Tauri shell: a thin adapter that exposes `netterm-core` to the
//! interface. No behaviour lives here; it only translates between the
//! interface's messages and the core's workspace and session interfaces.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use netterm_core::app_state::AppState;
use netterm_core::credentials::{
    CredentialStore, HostLogin, KeychainStore, MemoryStore, ProfileDraft, ProfileError,
    ProfileSummary, Profiles,
};
use netterm_core::session::{
    LoginPrompt, SessionEvent, SessionId, SessionManager, SessionState, SessionTarget, SshAuth,
    SshTarget, TerminalSize,
};
use netterm_core::workspace::{
    EditError, EffectiveSettings, FilterView, Folder, FolderDeletion, Host, HostId, Problem, Role,
    SavedFilter, SearchResults, Settings, WatchOptions, Workspace, WorkspaceWatcher,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// Overrides the per-user application data folder (the smoke test uses it to
/// start from a known state).
const DATA_DIR_VARIABLE: &str = "NETTERM_DATA_DIR";

/// Set to `memory` to keep credential profiles in memory instead of the OS
/// keychain (the smoke test uses it, so that it never touches the keychain).
const CREDENTIAL_STORE_VARIABLE: &str = "NETTERM_CREDENTIAL_STORE";

/// The event that brings the interface the workspace after changes made
/// outside the app.
const WORKSPACE_CHANGED_EVENT: &str = "workspace-changed";

/// Where app-level state lives, the open workspace, and where credential
/// profiles are kept.
struct Shell {
    data_folder: PathBuf,
    credentials: Box<dyn CredentialStore>,
    workspace: Mutex<Option<Workspace>>,
    /// Reports changes to the open workspace's files; replaced with it.
    watcher: Mutex<Option<WorkspaceWatcher>>,
}

impl Shell {
    fn workspace(&self) -> MutexGuard<'_, Option<Workspace>> {
        self.workspace
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes `workspace` the open one and watches its folder, so that
    /// changes from outside (a git pull, an editor) reach the interface.
    fn open(&self, app: &AppHandle, workspace: Workspace) -> WorkspaceView {
        let root = workspace.root().to_path_buf();
        *self.workspace() = Some(workspace);
        let app = app.clone();
        let watched = root.clone();
        let watcher = WorkspaceWatcher::start(&root, WatchOptions::default(), move |_| {
            app.state::<Shell>().reload_from_disk(&app, &watched);
        });
        let watcher = watcher
            .inspect_err(|error| eprintln!("changes from outside will not show: {error}"))
            .ok();
        // The previous watcher stops here.
        *self.watcher.lock().unwrap_or_else(PoisonError::into_inner) = watcher;

        let mut guard = self.workspace();
        let workspace = guard.as_mut().expect("the workspace was just opened");
        // What changed between loading and watching would otherwise only
        // show with the next change.
        if let Err(error) = workspace.refresh() {
            eprintln!("cannot reload the workspace: {error}");
        }
        WorkspaceView::from(&*workspace)
    }

    /// Reloads the workspace at `root`, if it is still the open one, and
    /// sends it to the interface if anything differs. The app's own edits
    /// are already loaded, so they send nothing.
    fn reload_from_disk(&self, app: &AppHandle, root: &std::path::Path) {
        let mut guard = self.workspace();
        let Some(workspace) = guard.as_mut().filter(|w| w.root() == root) else {
            return;
        };
        match workspace.refresh() {
            Ok(true) => {
                let view = WorkspaceView::from(&*workspace);
                if let Err(error) = app.emit(WORKSPACE_CHANGED_EVENT, view) {
                    eprintln!("cannot send the reloaded workspace: {error}");
                }
            }
            Ok(false) => {}
            // Gone or unreadable for now (mid-checkout, say): keep showing
            // what was loaded; the next change tries again.
            Err(error) => eprintln!("cannot reload the workspace: {error}"),
        }
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
    /// A host of the open workspace: with the credentials prompted for, or
    /// without, to log in with its credential profile.
    Host {
        id: HostId,
        credentials: Option<Prompted>,
    },
}

/// A username and password the user typed.
#[derive(Deserialize)]
struct Prompted {
    username: String,
    password: String,
}

/// An open workspace as the interface receives it.
#[derive(Serialize, Clone)]
struct WorkspaceView {
    /// `Workspace::revision`: higher is newer; the interface ignores a
    /// view older than the one it shows.
    revision: u64,
    root: String,
    tree: Folder,
    /// The roles a host can be given.
    roles: Vec<Role>,
    /// The role each host that has one shows, by host id.
    host_roles: BTreeMap<HostId, Role>,
    /// The saved filters, in file order.
    filters: Vec<SavedFilter>,
    problems: Vec<Problem>,
}

impl From<&Workspace> for WorkspaceView {
    fn from(workspace: &Workspace) -> Self {
        Self {
            revision: workspace.revision(),
            root: workspace.root().display().to_string(),
            tree: workspace.tree().clone(),
            roles: workspace.roles().to_vec(),
            host_roles: workspace.host_roles(),
            filters: workspace.filters().to_vec(),
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
    #[serde(rename_all = "camelCase")]
    Closed {
        reason: String,
        /// Logging in with a password might get past the failure (the SSH
        /// agent did not work).
        offers_password_login: bool,
    },
}

impl From<SessionState> for StateMessage {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Connecting => Self::Connecting,
            SessionState::Connected => Self::Connected,
            SessionState::Closed(reason) => Self::Closed {
                offers_password_login: reason.offers_password_login(),
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
fn workspace_open(
    app: AppHandle,
    shell: State<'_, Shell>,
    path: PathBuf,
) -> Result<WorkspaceView, String> {
    let workspace = Workspace::open(&path).map_err(|error| error.to_string())?;
    let view = shell.open(&app, workspace);
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
fn workspace_restore(
    app: AppHandle,
    shell: State<'_, Shell>,
) -> Result<Option<WorkspaceView>, String> {
    let Some(path) = AppState::load(&shell.data_folder).last_workspace else {
        return Ok(None);
    };
    let workspace = Workspace::open(&path).map_err(|error| error.to_string())?;
    Ok(Some(shell.open(&app, workspace)))
}

/// The workspace after an edit, and what the edit returned (a new path or
/// host id).
#[derive(Serialize)]
struct Edited<T> {
    workspace: WorkspaceView,
    result: T,
}

/// What deleting a folder did.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum FolderDeleted {
    Deleted {
        workspace: Box<WorkspaceView>,
    },
    /// Nothing was deleted: the folder holds hosts and the user must confirm.
    NeedsConfirmation {
        hosts: usize,
    },
}

/// Applies an edit to the open workspace.
fn edit<T>(
    shell: &Shell,
    change: impl FnOnce(&mut Workspace) -> Result<T, EditError>,
) -> Result<Edited<T>, String> {
    let mut guard = shell.workspace();
    let workspace = guard.as_mut().ok_or("no workspace is open")?;
    let result = change(workspace).map_err(|error| error.to_string())?;
    Ok(Edited {
        workspace: WorkspaceView::from(&*workspace),
        result,
    })
}

#[tauri::command]
fn folder_create(
    shell: State<'_, Shell>,
    parent: String,
    name: String,
) -> Result<Edited<String>, String> {
    edit(&shell, |workspace| workspace.create_folder(&parent, &name))
}

#[tauri::command]
fn folder_rename(
    shell: State<'_, Shell>,
    path: String,
    name: String,
) -> Result<Edited<String>, String> {
    edit(&shell, |workspace| workspace.rename_folder(&path, &name))
}

#[tauri::command]
fn folder_move(
    shell: State<'_, Shell>,
    path: String,
    parent: String,
) -> Result<Edited<String>, String> {
    edit(&shell, |workspace| workspace.move_folder(&path, &parent))
}

#[tauri::command]
fn folder_delete(
    shell: State<'_, Shell>,
    path: String,
    deletion: FolderDeletion,
) -> Result<FolderDeleted, String> {
    match edit(&shell, |workspace| {
        match workspace.delete_folder(&path, deletion) {
            Err(EditError::FolderHasHosts { hosts, .. }) => Ok(Some(hosts)),
            other => other.map(|()| None),
        }
    })? {
        Edited {
            result: Some(hosts),
            ..
        } => Ok(FolderDeleted::NeedsConfirmation { hosts }),
        Edited {
            workspace,
            result: None,
        } => Ok(FolderDeleted::Deleted {
            workspace: Box::new(workspace),
        }),
    }
}

#[tauri::command]
fn folder_set_description(
    shell: State<'_, Shell>,
    path: String,
    description: Option<String>,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| {
        workspace.set_folder_description(&path, description.as_deref())
    })
}

/// Saves the settings a folder sets for everything below it.
#[tauri::command]
fn folder_save_settings(
    shell: State<'_, Shell>,
    path: String,
    settings: Settings,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| {
        workspace.write_folder_settings(&path, &settings)
    })
}

#[tauri::command]
fn folder_set_tags(
    shell: State<'_, Shell>,
    path: String,
    tags: Vec<String>,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.set_folder_tags(&path, &tags))
}

/// A tree item, as the interface names it.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Item {
    Host { id: HostId },
    Folder { path: String },
}

/// The settings that apply to a host or folder, and where each comes from.
#[tauri::command]
fn settings_resolve(shell: State<'_, Shell>, item: Item) -> Result<EffectiveSettings, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    match item {
        Item::Host { id } => workspace
            .resolve_host(&id)
            .map(|host| host.settings)
            .ok_or_else(|| format!("{id} is not in the open workspace")),
        Item::Folder { path } => workspace
            .resolve_folder(&path)
            .ok_or_else(|| format!("{path} is not in the open workspace")),
    }
}

/// The credential profiles of the open workspace, as the interface shows
/// them: without secrets.
#[derive(Serialize)]
struct ProfilesView {
    profiles: Vec<ProfileSummary>,
    /// Profile names the workspace uses that are not set up here.
    missing: Vec<String>,
}

/// Runs `action` on the open workspace's credential profiles and returns
/// them afterwards.
fn with_profiles(
    shell: &Shell,
    action: impl FnOnce(&Profiles<'_>) -> Result<(), ProfileError>,
) -> Result<ProfilesView, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let profiles = Profiles::new(shell.credentials.as_ref(), workspace);
    let view = || {
        action(&profiles)?;
        Ok(ProfilesView {
            profiles: profiles.list()?,
            missing: profiles.missing()?,
        })
    };
    view().map_err(|error: ProfileError| error.to_string())
}

#[tauri::command]
fn profiles_list(shell: State<'_, Shell>) -> Result<ProfilesView, String> {
    with_profiles(&shell, |_| Ok(()))
}

/// Creates a profile (no `previous`) or changes the one called `previous`.
#[tauri::command]
fn profile_save(
    shell: State<'_, Shell>,
    previous: Option<String>,
    draft: ProfileDraft,
) -> Result<ProfilesView, String> {
    with_profiles(&shell, |profiles| {
        profiles.save(previous.as_deref(), &draft).map(drop)
    })
}

#[tauri::command]
fn profile_delete(shell: State<'_, Shell>, name: String) -> Result<ProfilesView, String> {
    with_profiles(&shell, |profiles| profiles.delete(&name))
}

/// How a host logs in, as the interface needs to know it.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum LoginView {
    /// With its profile, without a prompt.
    Profile { profile: String, username: String },
    /// The host names a profile that is not set up on this computer.
    NotSetUp { profile: String },
    /// Ask for a username and password.
    Prompt,
}

#[tauri::command]
fn host_login(shell: State<'_, Shell>, id: HostId) -> Result<LoginView, String> {
    Ok(match profile_login(&shell, &id)? {
        HostLogin::Profile { profile, target } => LoginView::Profile {
            profile,
            username: target.username,
        },
        HostLogin::NotSetUp { profile } => LoginView::NotSetUp { profile },
        HostLogin::Prompt => LoginView::Prompt,
    })
}

fn profile_login(shell: &Shell, id: &HostId) -> Result<HostLogin, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    Profiles::new(shell.credentials.as_ref(), workspace)
        .login(id)
        .map_err(|error| error.to_string())
}

/// The hosts of the open workspace matching `query` (the Ctrl+K palette).
#[tauri::command]
fn workspace_search(
    shell: State<'_, Shell>,
    query: String,
    limit: usize,
) -> Result<SearchResults, String> {
    read(&shell, |workspace| workspace.search(&query, limit))
}

/// The part of the tree a saved filter shows.
#[tauri::command]
fn filter_apply(shell: State<'_, Shell>, filter: SavedFilter) -> Result<FilterView, String> {
    read(&shell, |workspace| workspace.apply_filter(&filter))
}

/// Asks the open workspace something.
fn read<T>(shell: &Shell, query: impl FnOnce(&Workspace) -> T) -> Result<T, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    Ok(query(workspace))
}

#[tauri::command]
fn filter_save(shell: State<'_, Shell>, filter: SavedFilter) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.save_filter(&filter))
}

#[tauri::command]
fn filter_delete(shell: State<'_, Shell>, name: String) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.delete_filter(&name))
}

#[tauri::command]
fn host_create(
    shell: State<'_, Shell>,
    folder: String,
    name: String,
    address: String,
) -> Result<Edited<HostId>, String> {
    edit(&shell, |workspace| {
        workspace.create_host(&folder, &name, &address)
    })
}

#[tauri::command]
fn host_rename(
    shell: State<'_, Shell>,
    id: HostId,
    name: String,
) -> Result<Edited<HostId>, String> {
    edit(&shell, |workspace| workspace.rename_host(&id, &name))
}

#[tauri::command]
fn host_move(
    shell: State<'_, Shell>,
    id: HostId,
    folder: String,
) -> Result<Edited<HostId>, String> {
    edit(&shell, |workspace| workspace.move_host(&id, &folder))
}

#[tauri::command]
fn host_delete(shell: State<'_, Shell>, id: HostId) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.delete_host(&id))
}

/// Saves a host's fields, as edited in the side panel.
#[tauri::command]
fn host_save(shell: State<'_, Shell>, host: Host) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.write_host(&host))
}

/// Opens a session. Output arrives on `on_output` as raw bytes, already
/// batched by the core, state changes on `on_state`, and questions the
/// server asks while logging in on `on_prompt` (answered with
/// `session_answer`); terminal data never goes through the event system.
/// The interface acknowledges output with `session_acknowledge`.
#[tauri::command]
async fn session_open(
    manager: State<'_, SessionManager>,
    shell: State<'_, Shell>,
    target: ConnectTarget,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
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
            auth: SshAuth::Password(password),
        },
        ConnectTarget::Host {
            id,
            credentials: Some(Prompted { username, password }),
        } => shell
            .workspace()
            .as_ref()
            .and_then(|workspace| workspace.resolve_host(&id))
            .ok_or_else(|| format!("{id} is not in the open workspace"))?
            .ssh_target(username, password),
        ConnectTarget::Host {
            id,
            credentials: None,
        } => {
            let guard = shell.workspace();
            let workspace = guard.as_ref().ok_or("no workspace is open")?;
            Profiles::new(shell.credentials.as_ref(), workspace)
                .profile_target(&id)
                .map_err(|error| error.to_string())?
        }
    });
    let (id, mut events) = manager.open(target, size.into());
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            let delivered = match event {
                SessionEvent::Output(bytes) => on_output.send(InvokeResponseBody::Raw(bytes)),
                SessionEvent::State(state) => on_state.send(state.into()),
                SessionEvent::Prompt(prompt) => on_prompt.send(prompt),
            };
            // The webview is gone; dropping the events closes the session.
            if delivered.is_err() {
                break;
            }
        }
    });
    Ok(id.0)
}

/// Answers the questions the session's server asked last.
#[tauri::command]
fn session_answer(
    manager: State<'_, SessionManager>,
    id: u64,
    answers: Vec<String>,
) -> Result<(), String> {
    manager
        .answer(SessionId(id), answers)
        .map_err(|error| error.to_string())
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
            let credentials: Box<dyn CredentialStore> =
                match std::env::var(CREDENTIAL_STORE_VARIABLE).as_deref() {
                    Ok("memory") => Box::new(MemoryStore::new()),
                    // Entries are named after the app's identifier, which
                    // stays put if the product is renamed.
                    _ => Box::new(KeychainStore::new(app.config().identifier.clone())),
                };
            app.manage(Shell {
                data_folder,
                credentials,
                workspace: Mutex::new(None),
                watcher: Mutex::new(None),
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
            folder_create,
            folder_rename,
            folder_move,
            folder_delete,
            folder_set_description,
            folder_save_settings,
            folder_set_tags,
            settings_resolve,
            workspace_search,
            filter_apply,
            filter_save,
            filter_delete,
            host_create,
            host_rename,
            host_move,
            host_delete,
            host_save,
            profiles_list,
            profile_save,
            profile_delete,
            host_login,
            session_open,
            session_write,
            session_answer,
            session_acknowledge,
            session_resize,
            session_close
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
