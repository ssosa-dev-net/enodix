//! The Tauri shell: a thin adapter that exposes `netterm-core` to the
//! interface. No behaviour lives here; it only translates between the
//! interface's messages and the core's workspace and session interfaces.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use netterm_core::app_state::{resolved_path, AppState, KnownWorkspace};
use netterm_core::credentials::{
    prepare_workspace, CredentialStore, HostLogin, KeychainStore, MemoryStore, ProfileDraft,
    ProfileError, ProfileSummary, Profiles,
};
use netterm_core::highlight::{HighlightRule, Highlighter, ListedRule, Span};
use netterm_core::import::csv::{
    suggest_mapping, ColumnTarget, CsvImport, CsvTable, ImportPreview, Separator,
};
use netterm_core::import::ImportReport;
use netterm_core::known_hosts::KnownHosts;
use netterm_core::layout::{Layout, LayoutError, PaneId, Split, TabId};
use netterm_core::logging::{LogFolder, SessionLog};
use netterm_core::paste::{plan_paste, PastePlan, PasteSafety};
use netterm_core::preferences::{FontZoom, Preferences, Theme};
use netterm_core::session::{
    AlgorithmKind, AlgorithmLists, HostKeyDecision, HostKeyQuestion, LoginPrompt,
    NegotiatedAlgorithms, SerialParameters, SerialPortInfo, SerialTarget, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAuth, SshTarget,
    TerminalSize, Transport, DEFAULT_CONNECT_TIMEOUT,
};
use netterm_core::template::Variables;
use netterm_core::workspace::{
    EditError, EffectiveSettings, ExpandError, FilterView, Folder, FolderDeletion, Host, HostId,
    HostWebLink, Problem, Protocol, ResolvedHost, Role, SavedFilter, SearchResults, Settings,
    Snippet, WatchOptions, Workspace, WorkspaceWatcher, WORKSPACE_FILE,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

/// Overrides the per-user application data folder (the smoke test uses it to
/// start from a known state).
const DATA_DIR_VARIABLE: &str = "NETTERM_DATA_DIR";

/// Set to `memory` to keep credential profiles in memory instead of the OS
/// keychain (the smoke test uses it, so that it never touches the keychain).
const CREDENTIAL_STORE_VARIABLE: &str = "NETTERM_CREDENTIAL_STORE";

/// Set to a file path to have web links written there, one URL per line,
/// instead of opened in the browser (the smoke test uses it, so that it
/// never starts a browser).
const WEB_LINKS_TO_VARIABLE: &str = "NETTERM_WEB_LINKS_TO";

/// The event that brings the interface the workspace after changes made
/// outside the app.
const WORKSPACE_CHANGED_EVENT: &str = "workspace-changed";

/// Where app-level state lives, the open workspace, and where credential
/// profiles are kept.
struct Shell {
    data_folder: PathBuf,
    /// Held while `state.yaml` is read, changed and written back.
    app_state: Mutex<()>,
    /// Where session logs go: per user, outside every workspace.
    logs: LogFolder,
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
    fn open(&self, app: &AppHandle, mut workspace: Workspace) -> WorkspaceView {
        // Its profiles are kept under its own identifier (written to the
        // folder if it has none yet). Without one they stay under its path;
        // the interface shows why with the files that could not be loaded.
        let unprepared = prepare_workspace(self.credentials.as_ref(), &mut workspace).err();
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
        let mut view = WorkspaceView::from(&*workspace);
        if let Some(error) = unprepared {
            view.problems.push(Problem {
                file: WORKSPACE_FILE.into(),
                message: error.to_string(),
            });
        }
        view
    }

    /// Changes the app state kept in the data folder.
    fn update_state<T>(&self, change: impl FnOnce(&mut AppState) -> T) -> std::io::Result<T> {
        let _held = self
            .app_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut state = AppState::load(&self.data_folder);
        let result = change(&mut state);
        state.save(&self.data_folder)?;
        Ok(result)
    }

    /// The known workspaces, marking the open one.
    fn known_workspaces(&self) -> Vec<KnownWorkspace> {
        let open = self.workspace().as_ref().map(|w| w.root().to_path_buf());
        let _held = self
            .app_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        AppState::load(&self.data_folder).known_workspaces(open.as_deref())
    }

    /// Makes `workspace` the open one and adds it to the known workspaces
    /// (at the top), to reopen on the next launch.
    fn open_and_remember(
        &self,
        app: &AppHandle,
        workspace: Workspace,
    ) -> Result<WorkspaceView, String> {
        let path = workspace.root().to_path_buf();
        let view = self.open(app, workspace);
        self.update_state(|state| state.remember(&path))
            .map_err(|error| {
                format!("the workspace opened, but could not be remembered: {error}")
            })?;
        Ok(view)
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
    /// A host of the open workspace, by telnet (whatever its protocol
    /// setting): the device asks for the login in the terminal.
    Telnet { id: HostId },
    /// A host of the open workspace, on its serial console (whatever its
    /// protocol setting), with its serial settings.
    Serial { id: HostId },
    /// The quick-connect form's serial console: any port, no host.
    QuickSerial {
        port: String,
        parameters: SerialParameters,
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
    /// Every host's environment and paste line delay, by host id.
    host_safety: BTreeMap<HostId, PasteSafety>,
    /// The saved filters, in file order.
    filters: Vec<SavedFilter>,
    /// The highlight rules, the workspace's own first.
    highlight_rules: Vec<ListedRule>,
    /// The snippets, in file order.
    snippets: Vec<Snippet>,
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
            host_safety: workspace.host_paste_safety(),
            filters: workspace.filters().to_vec(),
            highlight_rules: workspace.highlight_rules().to_vec(),
            snippets: workspace.snippets().to_vec(),
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
        /// The algorithm negotiation failed: what the device offered.
        negotiation: Option<NegotiationMessage>,
    },
}

/// A failed algorithm negotiation as the interface shows it.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct NegotiationMessage {
    /// The jump host that failed; `None` for the device.
    jump_host: Option<String>,
    /// The first kind of algorithm nothing was in common for.
    kind: AlgorithmKind,
    /// What the device (or jump host) offered.
    offered: AlgorithmLists,
    /// Whether the session had the legacy-crypto switch on.
    legacy_crypto: bool,
    /// Turning the switch on would let the session agree with it.
    legacy_crypto_would_help: bool,
    /// The kind nothing would be in common for even with the switch on.
    unsupported: Option<AlgorithmKind>,
}

impl From<SessionState> for StateMessage {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Connecting => Self::Connecting,
            SessionState::Connected => Self::Connected,
            SessionState::Closed(reason) => Self::Closed {
                offers_password_login: reason.offers_password_login(),
                negotiation: reason.negotiation_failure().map(|(jump_host, failure)| {
                    NegotiationMessage {
                        jump_host: jump_host.map(str::to_owned),
                        kind: failure.kind,
                        offered: failure.offered.clone(),
                        legacy_crypto: failure.legacy_crypto,
                        legacy_crypto_would_help: failure.legacy_crypto_would_help(),
                        unsupported: failure.unsupported_even_with_legacy_crypto(),
                    }
                }),
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

/// Opens the workspace folder at `path` (an existing folder, which becomes
/// a known workspace) and remembers it for the next launch.
#[tauri::command]
fn workspace_open(
    app: AppHandle,
    shell: State<'_, Shell>,
    path: PathBuf,
) -> Result<WorkspaceView, String> {
    // One root for the interface however the path was typed.
    let workspace = Workspace::open(resolved_path(&path)).map_err(|error| error.to_string())?;
    shell.open_and_remember(&app, workspace)
}

/// Creates a new, empty workspace in the folder at `path` and opens it.
#[tauri::command]
fn workspace_create(
    app: AppHandle,
    shell: State<'_, Shell>,
    path: PathBuf,
) -> Result<WorkspaceView, String> {
    let created = Workspace::create(&path).map_err(|error| error.to_string())?;
    // One root for the interface however the path was typed (it can only
    // be resolved now that the folder is there).
    let workspace =
        Workspace::open(resolved_path(created.root())).map_err(|error| error.to_string())?;
    shell.open_and_remember(&app, workspace)
}

/// The known workspaces, most recently opened first (the switcher's list).
#[tauri::command]
fn workspaces_list(shell: State<'_, Shell>) -> Vec<KnownWorkspace> {
    shell.known_workspaces()
}

/// Takes the workspace at `path` off the known workspaces. Its folder is
/// left as it is. The open workspace cannot be taken off.
#[tauri::command]
fn workspace_forget(shell: State<'_, Shell>, path: PathBuf) -> Result<Vec<KnownWorkspace>, String> {
    let open = shell.workspace().as_ref().map(|w| w.root().to_path_buf());
    shell
        .update_state(|state| state.forget(&path, open.as_deref()))
        .map_err(|error| format!("the list of workspaces could not be saved: {error}"))?
        .map_err(|error| error.to_string())?;
    Ok(shell.known_workspaces())
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
    /// Over telnet: connect at once; the device asks in the terminal.
    Telnet,
    /// On its serial console: connect at once, as for telnet.
    Serial,
}

/// How the host `id` logs in over `protocol`, or over its own protocol
/// setting if none is given.
#[tauri::command]
fn host_login(
    shell: State<'_, Shell>,
    id: HostId,
    protocol: Option<Protocol>,
) -> Result<LoginView, String> {
    Ok(match profile_login(&shell, &id, protocol)? {
        HostLogin::Profile { profile, target } => LoginView::Profile {
            profile,
            username: target.username,
        },
        HostLogin::NotSetUp { profile } => LoginView::NotSetUp { profile },
        HostLogin::Prompt => LoginView::Prompt,
        HostLogin::Telnet => LoginView::Telnet,
        HostLogin::Serial => LoginView::Serial,
    })
}

fn profile_login(
    shell: &Shell,
    id: &HostId,
    protocol: Option<Protocol>,
) -> Result<HostLogin, String> {
    with_open_profiles(shell, |profiles| match protocol {
        Some(protocol) => profiles.login_with(id, protocol),
        None => profiles.login(id),
    })
}

/// Asks the open workspace's credential profiles something.
fn with_open_profiles<T>(
    shell: &Shell,
    query: impl FnOnce(&Profiles<'_>) -> Result<T, ProfileError>,
) -> Result<T, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    query(&Profiles::new(shell.credentials.as_ref(), workspace)).map_err(|error| error.to_string())
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

/// What to highlight in each of `lines` (text shown in a terminal), by
/// the open workspace's rules, or the built-in ones when none is open.
#[tauri::command]
fn highlight_lines(shell: State<'_, Shell>, lines: Vec<String>) -> Vec<Vec<Span>> {
    let guard = shell.workspace();
    let built_in;
    let highlighter = match guard.as_ref() {
        Some(workspace) => workspace.highlighter(),
        None => {
            built_in = Highlighter::default();
            &built_in
        }
    };
    lines.iter().map(|line| highlighter.spans(line)).collect()
}

/// The highlight rules when no workspace is open: the built-in ones.
#[tauri::command]
fn highlight_built_in_rules() -> Vec<ListedRule> {
    netterm_core::highlight::built_in_rules()
}

/// Saves a highlight rule in place of the one called `previous`, or as a
/// new one.
#[tauri::command]
fn highlight_rule_save(
    shell: State<'_, Shell>,
    previous: Option<String>,
    rule: HighlightRule,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| {
        workspace.save_highlight_rule(previous.as_deref(), &rule)
    })
}

/// Deletes a workspace highlight rule, or a built-in rule's changes.
#[tauri::command]
fn highlight_rule_delete(shell: State<'_, Shell>, name: String) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.delete_highlight_rule(&name))
}

/// The snippets matching `query` (the Ctrl+K palette's snippet search).
#[tauri::command]
fn snippet_find(shell: State<'_, Shell>, query: String) -> Result<Vec<Snippet>, String> {
    read(&shell, |workspace| {
        workspace
            .find_snippets(&query)
            .into_iter()
            .cloned()
            .collect()
    })
}

/// Saves a snippet in place of the one called `previous`, or as a new one.
#[tauri::command]
fn snippet_save(
    shell: State<'_, Shell>,
    previous: Option<String>,
    snippet: Snippet,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| {
        workspace.save_snippet(previous.as_deref(), &snippet)
    })
}

#[tauri::command]
fn snippet_delete(shell: State<'_, Shell>, name: String) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.delete_snippet(&name))
}

/// A snippet made ready to send, as the interface receives it.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Expansion {
    /// Send these, each as a line.
    Lines { lines: Vec<String> },
    /// Ask the user for these variables first, then expand again.
    Missing { names: Vec<String> },
}

/// The lines of the snippet `name` for a session to `host` (none: not an
/// inventory host), with `answers` for the variables the host has no value
/// for.
#[tauri::command]
fn snippet_expand(
    shell: State<'_, Shell>,
    name: String,
    host: Option<HostId>,
    answers: BTreeMap<String, String>,
) -> Result<Expansion, String> {
    let answers: Variables = answers.into_iter().collect();
    read(&shell, |workspace| {
        match workspace.expand_snippet(&name, host.as_ref(), &answers) {
            Ok(lines) => Ok(Expansion::Lines { lines }),
            Err(ExpandError::Missing(missing)) => Ok(Expansion::Missing {
                names: missing.names,
            }),
            Err(error) => Err(error.to_string()),
        }
    })?
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

/// The CSV file being imported, as read once: the preview and the import
/// both use this copy, so they agree even if the file changes meanwhile.
struct CsvFile(Mutex<Option<CsvTable>>);

impl CsvFile {
    fn lock(&self) -> MutexGuard<'_, Option<CsvTable>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A CSV file as the import dialog shows it before mapping.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CsvFileView {
    separator: Separator,
    headers: Vec<String>,
    /// How many rows below the header.
    rows: usize,
    /// The first rows' values, to recognise the columns by.
    sample: Vec<Vec<String>>,
    suggested: Vec<ColumnTarget>,
}

/// How many rows `csv_import_read` shows.
const CSV_SAMPLE_ROWS: usize = 3;

/// Reads the CSV file at `path` for importing (replacing any read before).
#[tauri::command]
fn csv_import_read(file: State<'_, CsvFile>, path: PathBuf) -> Result<CsvFileView, String> {
    let table = CsvTable::read(&path).map_err(|error| error.to_string())?;
    let view = CsvFileView {
        separator: table.separator,
        headers: table.headers.clone(),
        rows: table.rows.len(),
        sample: table
            .rows
            .iter()
            .take(CSV_SAMPLE_ROWS)
            .map(|row| row.values.clone())
            .collect(),
        suggested: suggest_mapping(&table.headers),
    };
    *file.lock() = Some(table);
    Ok(view)
}

fn csv_import(
    file: &CsvFile,
    mapping: Vec<ColumnTarget>,
    target: String,
) -> Result<CsvImport, String> {
    let table = file.lock().clone().ok_or("no CSV file has been read")?;
    Ok(CsvImport {
        table,
        mapping,
        target,
    })
}

/// What importing the CSV file read last would do; writes nothing.
#[tauri::command]
fn csv_import_preview(
    shell: State<'_, Shell>,
    file: State<'_, CsvFile>,
    mapping: Vec<ColumnTarget>,
    target: String,
) -> Result<ImportPreview, String> {
    let import = csv_import(&file, mapping, target)?;
    read(&shell, |workspace| import.preview(workspace))?.map_err(|error| error.to_string())
}

/// Imports the CSV file read last. Existing hosts are only written if
/// listed in `replace` (the user confirmed them).
#[tauri::command]
fn csv_import_run(
    shell: State<'_, Shell>,
    file: State<'_, CsvFile>,
    mapping: Vec<ColumnTarget>,
    target: String,
    replace: Vec<HostId>,
) -> Result<Edited<ImportReport>, String> {
    let import = csv_import(&file, mapping, target)?;
    let mut guard = shell.workspace();
    let workspace = guard.as_mut().ok_or("no workspace is open")?;
    let report = import
        .run(workspace, &replace)
        .map_err(|error| error.to_string())?;
    Ok(Edited {
        workspace: WorkspaceView::from(&*workspace),
        result: report,
    })
}

/// Opens a session. Output arrives on `on_output` as raw bytes, already
/// batched by the core, state changes on `on_state`, questions the server
/// asks while logging in on `on_prompt` (answered with `session_answer`),
/// host keys to decide on on `on_host_key` (decided with
/// `session_decide_host_key`), and the algorithms each connection agreed on
/// (just before `connected`) on `on_algorithms`, and why its log could not
/// be written (the session goes on) on `on_log_failure`; terminal data
/// never goes through the event system.
/// The interface acknowledges output with `session_acknowledge`.
/// Returns the session's id and whether it is encrypted (telnet is not).
// One argument per channel is how Tauri commands receive them.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn session_open(
    manager: State<'_, SessionManager>,
    shell: State<'_, Shell>,
    target: ConnectTarget,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
) -> Result<Opened, String> {
    let log = session_log(&shell, &target)?;
    let target = match target {
        ConnectTarget::Serial { id } => {
            SessionTarget::Serial(with_open_profiles(&shell, |profiles| {
                profiles.serial_target(&id)
            })?)
        }
        ConnectTarget::QuickSerial { port, parameters } => {
            SessionTarget::Serial(SerialTarget { port, parameters })
        }
        ConnectTarget::Telnet { id } => {
            SessionTarget::Telnet(with_open_profiles(&shell, |profiles| {
                profiles.telnet_target(&id)
            })?)
        }
        ConnectTarget::Quick {
            host,
            port,
            username,
            password,
        } => SessionTarget::Ssh(SshTarget {
            host,
            port,
            username,
            auth: SshAuth::Password(password),
            jump_hosts: Vec::new(),
            // Quick connections have no settings to switch it on.
            legacy_crypto: false,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }),
        ConnectTarget::Host {
            id,
            credentials: Some(Prompted { username, password }),
        } => SessionTarget::Ssh(with_open_profiles(&shell, |profiles| {
            profiles.prompted_target(&id, username, password)
        })?),
        ConnectTarget::Host {
            id,
            credentials: None,
        } => SessionTarget::Ssh(with_open_profiles(&shell, |profiles| {
            profiles.profile_target(&id)
        })?),
    };
    let encrypted = target.encrypted();
    let transport = target.transport();
    let (id, events) = manager.open_logged(target, size.into(), log);
    forward(
        events,
        Channels {
            output: on_output,
            state: on_state,
            prompt: on_prompt,
            host_key: on_host_key,
            algorithms: on_algorithms,
            log_failure: on_log_failure,
        },
    );
    Ok(Opened {
        id: id.0,
        encrypted,
        transport,
    })
}

/// Where a session to `target` is logged: a workspace host as its
/// `logging` setting says, a quick connection always.
fn session_log(shell: &Shell, target: &ConnectTarget) -> Result<Option<SessionLog>, String> {
    Ok(match target {
        ConnectTarget::Host { id, .. }
        | ConnectTarget::Telnet { id }
        | ConnectTarget::Serial { id } => {
            read(shell, |workspace| shell.logs.host_log(workspace, id))?
        }
        ConnectTarget::Quick { host, .. } => Some(shell.logs.quick_log(host)),
        ConnectTarget::QuickSerial { port, .. } => Some(shell.logs.quick_log(port)),
    })
}

/// Shows the folder of the host `id`'s session logs in the system's file
/// manager, creating it if no session has been logged yet.
#[tauri::command]
fn logs_open_folder(app: AppHandle, shell: State<'_, Shell>, id: HostId) -> Result<(), String> {
    let folder = read(&shell, |workspace| {
        shell.logs.create_host_folder(workspace, &id)
    })?
    .map_err(|error| format!("cannot create the log folder: {error}"))?;
    app.opener()
        .open_path(folder.display().to_string(), None::<&str>)
        .map_err(|error| format!("cannot open the log folder {}: {error}", folder.display()))
}

/// The web links of the host `id`, each with the URL it opens or why it
/// is refused.
#[tauri::command]
fn host_web_links(shell: State<'_, Shell>, id: HostId) -> Result<Vec<HostWebLink>, String> {
    Ok(resolved_host(&shell, &id)?.web_links())
}

/// The open workspace's host `id`, with its inherited settings.
fn resolved_host(shell: &Shell, id: &HostId) -> Result<ResolvedHost, String> {
    read(shell, |workspace| workspace.resolve_host(id))?
        .ok_or_else(|| format!("{id} is not in the open workspace"))
}

/// Opens the host `id`'s web link `name` in the default browser, if the
/// core lets it (http and https only); returns the URL opened.
#[tauri::command]
fn web_link_open(
    app: AppHandle,
    shell: State<'_, Shell>,
    id: HostId,
    name: String,
) -> Result<String, String> {
    let url = resolved_host(&shell, &id)?
        .web_link_url(&name)
        .map_err(|error| format!("{name} was not opened: {error}"))?;
    match std::env::var_os(WEB_LINKS_TO_VARIABLE) {
        Some(file) => {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)
                .and_then(|mut file| writeln!(file, "{url}"))
                .map_err(|error| format!("cannot record {url}: {error}"))?;
        }
        None => app
            .opener()
            .open_url(&url, None::<&str>)
            .map_err(|error| format!("cannot open {url} in the browser: {error}"))?,
    }
    Ok(url)
}

/// A session just opened, as the interface receives it.
#[derive(Serialize)]
struct Opened {
    id: u64,
    /// Whether its traffic is encrypted on the network; telnet's is not.
    encrypted: bool,
    /// `ssh`, `telnet` or `serial` (serial sessions can send a break).
    transport: Transport,
}

/// Where a session's events go in the interface.
struct Channels {
    output: Channel<InvokeResponseBody>,
    state: Channel<StateMessage>,
    prompt: Channel<LoginPrompt>,
    host_key: Channel<HostKeyQuestion>,
    algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    /// Why the session's log could not be written.
    log_failure: Channel<String>,
}

/// Passes the session's events on to the interface until they end.
fn forward(mut events: SessionEvents, channels: Channels) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            let delivered = match event {
                SessionEvent::Output(bytes) => channels.output.send(InvokeResponseBody::Raw(bytes)),
                SessionEvent::State(state) => channels.state.send(state.into()),
                SessionEvent::Prompt(prompt) => channels.prompt.send(prompt),
                SessionEvent::HostKey(question) => channels.host_key.send(question),
                SessionEvent::Algorithms(algorithms) => channels.algorithms.send(algorithms),
                SessionEvent::LogFailed(failure) => channels.log_failure.send(failure.to_string()),
            };
            // The webview is gone; dropping the events closes the session.
            if delivered.is_err() {
                break;
            }
        }
    });
}

/// Opens a session that has closed again, in place: the same target,
/// settings and login, the same id. Its events arrive on new channels, as
/// for `session_open`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn session_reconnect(
    manager: State<'_, SessionManager>,
    id: u64,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
) -> Result<(), String> {
    let events = manager
        .reconnect(SessionId(id), size.into())
        .map_err(|error| error.to_string())?;
    forward(
        events,
        Channels {
            output: on_output,
            state: on_state,
            prompt: on_prompt,
            host_key: on_host_key,
            algorithms: on_algorithms,
            log_failure: on_log_failure,
        },
    );
    Ok(())
}

/// Opens a new session to the same target as the session `id`, with the
/// same settings and login; returns it as `session_open` does. Events as
/// for `session_open`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn session_duplicate(
    manager: State<'_, SessionManager>,
    id: u64,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
) -> Result<Opened, String> {
    let (copy, events) = manager
        .duplicate(SessionId(id), size.into())
        .map_err(|error| error.to_string())?;
    forward(
        events,
        Channels {
            output: on_output,
            state: on_state,
            prompt: on_prompt,
            host_key: on_host_key,
            algorithms: on_algorithms,
            log_failure: on_log_failure,
        },
    );
    let encrypted = manager.encrypted(copy).map_err(|error| error.to_string())?;
    let transport = manager.transport(copy).map_err(|error| error.to_string())?;
    Ok(Opened {
        id: copy.0,
        encrypted,
        transport,
    })
}

/// The window's tabs and panes.
struct Tabs(Mutex<Layout>);

/// A change to the window's tabs and panes, as `Layout`'s methods name them.
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum LayoutEdit {
    OpenTab,
    OpenTabBeside {
        tab: TabId,
    },
    Activate {
        tab: TabId,
    },
    ActivatePane {
        pane: PaneId,
    },
    ShowConnectView {
        pane: PaneId,
    },
    NextTab,
    PreviousTab,
    CloseTab {
        tab: TabId,
    },
    DismissTab {
        tab: TabId,
    },
    MoveTabBy {
        tab: TabId,
        offset: isize,
    },
    MoveTab {
        tab: TabId,
        pane: PaneId,
        index: usize,
    },
    Split {
        pane: PaneId,
        direction: Split,
    },
    ClosePane {
        pane: PaneId,
    },
}

/// The layout after an edit, and the tab or pane the edit created.
#[derive(Serialize)]
struct LayoutChanged {
    layout: Layout,
    created: Option<u64>,
}

/// Starts the window's layout afresh: one empty pane (the interface's
/// sessions start afresh too when it loads).
#[tauri::command]
fn layout_reset(tabs: State<'_, Tabs>) -> Layout {
    let mut layout = tabs.0.lock().unwrap_or_else(PoisonError::into_inner);
    *layout = Layout::new();
    layout.clone()
}

#[tauri::command]
fn layout_edit(tabs: State<'_, Tabs>, edit: LayoutEdit) -> Result<LayoutChanged, String> {
    let mut layout = tabs.0.lock().unwrap_or_else(PoisonError::into_inner);
    let created = apply_layout_edit(&mut layout, edit).map_err(|error| error.to_string())?;
    Ok(LayoutChanged {
        layout: layout.clone(),
        created,
    })
}

fn apply_layout_edit(layout: &mut Layout, edit: LayoutEdit) -> Result<Option<u64>, LayoutError> {
    Ok(match edit {
        LayoutEdit::OpenTab => Some(layout.open_tab().0),
        LayoutEdit::OpenTabBeside { tab } => Some(layout.open_tab_beside(tab).map(|t| t.0)?),
        LayoutEdit::Activate { tab } => layout.activate(tab).map(|()| None)?,
        LayoutEdit::ActivatePane { pane } => layout.activate_pane(pane).map(|()| None)?,
        LayoutEdit::ShowConnectView { pane } => layout.show_connect_view(pane).map(|()| None)?,
        LayoutEdit::NextTab => {
            layout.next_tab();
            None
        }
        LayoutEdit::PreviousTab => {
            layout.previous_tab();
            None
        }
        LayoutEdit::CloseTab { tab } => layout.close_tab(tab).map(|()| None)?,
        LayoutEdit::DismissTab { tab } => layout.dismiss_tab(tab).map(|()| None)?,
        LayoutEdit::MoveTabBy { tab, offset } => layout.move_tab_by(tab, offset).map(|()| None)?,
        LayoutEdit::MoveTab { tab, pane, index } => {
            layout.move_tab(tab, pane, index).map(|()| None)?
        }
        LayoutEdit::Split { pane, direction } => Some(layout.split(pane, direction)?.0),
        LayoutEdit::ClosePane { pane } => layout.close_pane(pane).map(|()| None)?,
    })
}

/// Forgets the session (closing it if it is open): its terminal is gone.
#[tauri::command]
fn session_discard(manager: State<'_, SessionManager>, id: u64) {
    manager.discard(SessionId(id));
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

/// Decides whether to trust the host key the session asked about last.
#[tauri::command]
fn session_decide_host_key(
    manager: State<'_, SessionManager>,
    id: u64,
    decision: HostKeyDecision,
) -> Result<(), String> {
    manager
        .decide_host_key(SessionId(id), decision)
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

/// Sends a break on a serial session's line.
#[tauri::command]
fn session_break(manager: State<'_, SessionManager>, id: u64) -> Result<(), String> {
    manager
        .send_break(SessionId(id))
        .map_err(|error| error.to_string())
}

/// The serial ports this computer has now, with their adapters.
#[tauri::command]
async fn serial_ports(manager: State<'_, SessionManager>) -> Result<Vec<SerialPortInfo>, String> {
    manager.serial_ports().map_err(|error| error.to_string())
}

/// The user's preferences, as loaded at launch or last saved.
struct UserPreferences(Mutex<Preferences>);

impl UserPreferences {
    fn lock(&self) -> MutexGuard<'_, Preferences> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The window frame's theme for a preference (`None`: the system's).
fn window_theme(theme: Theme) -> Option<tauri::Theme> {
    match theme {
        Theme::System => None,
        Theme::Light => Some(tauri::Theme::Light),
        Theme::Dark => Some(tauri::Theme::Dark),
    }
}

/// Makes `next` the preferences: saved (normalised) to the data folder, and
/// the window frame given its theme. Returns what was saved.
fn adopt_preferences(
    app: &AppHandle,
    shell: &Shell,
    current: &mut Preferences,
    next: Preferences,
) -> Result<Preferences, String> {
    let next = next.normalized();
    next.save(&shell.data_folder)
        .map_err(|error| format!("the preferences could not be saved: {error}"))?;
    let theme_changed = next.theme != current.theme;
    // Saved: from here on these are the preferences, whatever the frame does.
    *current = next.clone();
    if theme_changed {
        if let Some(window) = app.get_webview_window("main") {
            // Only the frame's colours; the page already follows the theme.
            let _ = window.set_theme(window_theme(next.theme));
        }
    }
    Ok(next)
}

#[tauri::command]
fn preferences_get(preferred: State<'_, UserPreferences>) -> Preferences {
    preferred.lock().clone()
}

#[tauri::command]
fn preferences_save(
    app: AppHandle,
    shell: State<'_, Shell>,
    preferred: State<'_, UserPreferences>,
    preferences: Preferences,
) -> Result<Preferences, String> {
    adopt_preferences(&app, &shell, &mut preferred.lock(), preferences)
}

/// Steps the terminal font size (all terminals share it).
#[tauri::command]
fn preferences_zoom(
    app: AppHandle,
    shell: State<'_, Shell>,
    preferred: State<'_, UserPreferences>,
    zoom: FontZoom,
) -> Result<Preferences, String> {
    let mut current = preferred.lock();
    let mut next = current.clone();
    next.zoom(zoom);
    adopt_preferences(&app, &shell, &mut current, next)
}

/// How `text` is pasted into a session with `safety` (its host's, from
/// the workspace view): its lines, and whether the user confirms it first.
#[tauri::command]
fn paste_plan(
    preferred: State<'_, UserPreferences>,
    text: String,
    safety: PasteSafety,
) -> PastePlan {
    let confirm = preferred.lock().confirm_multi_line_paste;
    plan_paste(&text, &safety, confirm)
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
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let (data_folder, log_folder) = match std::env::var_os(DATA_DIR_VARIABLE) {
                Some(folder) => {
                    let folder = PathBuf::from(folder);
                    let logs = folder.join("logs");
                    (folder, logs)
                }
                // Logs stay on this computer (local, not roaming, app data).
                None => (app.path().app_data_dir()?, app.path().app_log_dir()?),
            };
            let credentials: Box<dyn CredentialStore> =
                match std::env::var(CREDENTIAL_STORE_VARIABLE).as_deref() {
                    Ok("memory") => Box::new(MemoryStore::new()),
                    // Entries are named after the app's identifier, which
                    // stays put if the product is renamed.
                    _ => Box::new(KeychainStore::new(app.config().identifier.clone())),
                };
            // Trusted host keys are per user, beside the app state.
            app.manage(SessionManager::new(KnownHosts::in_data_folder(
                &data_folder,
            )));
            app.manage(Tabs(Mutex::new(Layout::new())));
            app.manage(CsvFile(Mutex::new(None)));
            let preferences = Preferences::load(&data_folder);
            let theme = window_theme(preferences.theme);
            app.manage(UserPreferences(Mutex::new(preferences)));
            app.manage(Shell {
                data_folder,
                app_state: Mutex::new(()),
                logs: LogFolder::new(log_folder),
                credentials,
                workspace: Mutex::new(None),
                watcher: Mutex::new(None),
            });
            let title = product_name(app.handle());
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title(title)
                .inner_size(1100.0, 720.0)
                .min_inner_size(480.0, 320.0)
                .theme(theme)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_name,
            workspace_open,
            workspace_create,
            workspaces_list,
            workspace_forget,
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
            highlight_lines,
            highlight_built_in_rules,
            highlight_rule_save,
            highlight_rule_delete,
            snippet_find,
            snippet_save,
            snippet_delete,
            snippet_expand,
            host_create,
            host_rename,
            host_move,
            host_delete,
            host_save,
            csv_import_read,
            csv_import_preview,
            csv_import_run,
            profiles_list,
            profile_save,
            profile_delete,
            host_login,
            session_open,
            session_write,
            session_answer,
            session_decide_host_key,
            session_acknowledge,
            session_resize,
            session_close,
            session_break,
            serial_ports,
            session_reconnect,
            session_duplicate,
            session_discard,
            logs_open_folder,
            host_web_links,
            web_link_open,
            layout_reset,
            layout_edit,
            preferences_get,
            preferences_save,
            preferences_zoom,
            paste_plan
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the app");
}
