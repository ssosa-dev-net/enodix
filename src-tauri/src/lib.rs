//! The Tauri shell: a thin adapter that exposes `enodix-core` to the
//! interface. No behaviour lives here; it only translates between the
//! interface's messages and the core's workspace and session interfaces.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use enodix_core::app_state::{resolved_path, AppState, KnownWorkspace};
use enodix_core::configs::{BackedUpHost, ConfigStore, ConfigVersion};
use enodix_core::credentials::{
    prepare_workspace, vault_file, ActiveStore, AgentEndpoint, AgentServer, HostLogin,
    KeychainStore, MemoryStore, ProfileDraft, ProfileError, ProfileSummary, Profiles, Secrets,
    VaultError, VaultStatus,
};
use enodix_core::data_folders;
use enodix_core::diff::DiffLine;
use enodix_core::enrich::{self, EnrichPreview, EnrichReport};
use enodix_core::highlight::{HighlightRule, Highlighter, ListedRule, Span};
use enodix_core::history::{self, LeftHistory};
use enodix_core::import::csv::{
    suggest_mapping, ColumnTarget, CsvImport, CsvTable, ImportPreview, Separator,
};
use enodix_core::import::nornir::{NornirImport, NornirInventory, NornirPreview};
use enodix_core::import::ssh_config::{SshConfig, SshConfigImport, SshConfigPreview};
use enodix_core::import::ImportReport;
use enodix_core::job::results::{OutputRef, RunList, RunStore, SearchHit, StoredRun};
use enodix_core::job::{
    self, JobEvent, JobOptions, JobPlan, JobStep, JobTarget, KeptPlan, KeptPlans, LiveJob,
    LiveJobs, LoginBackup, LoginBackupOutcome, NewLiveJob, PlanId, RunningJobs,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::layout::{
    InputOrigin, LayoutError, PaneId, Split, TabId, View, WindowId, WindowView, Windows,
};
use enodix_core::logging::{LogFolder, SessionLog};
use enodix_core::paste::{
    confirm_joining, plan_group_paste, plan_paste, GroupPastePlan, PastePlan, PasteSafety,
};
use enodix_core::platform::{ListedPlatform, PlatformDefinition, PlatformName};
use enodix_core::preferences::{FontZoom, Preferences, Theme};
use enodix_core::session::{
    local_environment, local_shells, local_start_folder, pick_local_shell, system_ssh_target,
    AlgorithmKind, AlgorithmLists, AutoReconnects, Consumer, ConsumerGone, ConsumerKey,
    ForwardStatus, HostKeyDecision, HostKeyQuestion, HostSession, HostWay, LoginPrompt,
    NegotiatedAlgorithms, OpenOptions, Reconnection, SerialParameters, SerialPortInfo,
    SerialTarget, SessionEvent, SessionId, SessionManager, SessionRecord, SessionRecords,
    SessionRelays, SessionState, SessionTarget, SshAuth, SshTarget, TerminalSize, TerminalType,
    Transport, TypedLogin, WhenConnected, DEFAULT_CONNECT_TIMEOUT, SYSTEM_SSH_WARNING,
};
use enodix_core::steps::StepRuns;
use enodix_core::template::Variables;
use enodix_core::update::{InstallKind, UpdateChecker, UpdateSource, UpdateStatus};
use enodix_core::workspace::{
    folder_kind, EditError, EffectiveSettings, ExpandError, FilterView, Folder, FolderDeletion,
    FolderKind, Host, HostId, HostMove, HostWebLink, Moved, Problem, Protocol, ResolvedHost, Role,
    SavedFilter, SavedJob, SearchResults, Settings, Snippet, Unavailable, WatchOptions, Workspace,
    WorkspaceWatcher, WORKSPACE_FILE,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

mod files;
mod group;
mod page_watch;
mod python;
mod session_windows;
mod steps;

/// Overrides the per-user application data folder (the smoke test uses it to
/// start from a known state).
const DATA_DIR_VARIABLE: &str = "ENODIX_DATA_DIR";

/// Set to `memory` to keep credential profiles in memory instead of the OS
/// keychain (the smoke test uses it, so that it never touches the keychain).
const CREDENTIAL_STORE_VARIABLE: &str = "ENODIX_CREDENTIAL_STORE";

/// Set to a file path to have web links written there, one URL per line,
/// instead of opened in the browser (the smoke test uses it, so that it
/// never starts a browser).
const WEB_LINKS_TO_VARIABLE: &str = "ENODIX_WEB_LINKS_TO";

/// Overrides the release manifest's address (the smoke test points it at a
/// local server). A release build has its own, from the release script.
const UPDATE_ENDPOINT_VARIABLE: &str = "ENODIX_UPDATE_ENDPOINT";

/// What every window's WebView2 is started with (#115). wry's own defaults
/// (no mini menu, no PDF menu, no SmartScreen, sound without a click),
/// which giving any arguments replaces, and three more: left to itself
/// WebView2 fetches Edge's configuration from `config.edge.skype.com` (with
/// a client id, the OS version and the install date), looks the name
/// `wpad` up for a proxy script and, a minute in, asks
/// `edge.microsoft.com` for component updates and downloads them. The page
/// is the app's own and never leaves the computer, so it needs no
/// background networking, no proxy and no components. The nine features
/// after wry's three are Edge's sign-in with the Windows account (#135):
/// on a computer signed in to a Microsoft account the engine's sign-in
/// library (OneAuth) otherwise fetches that account's profile from
/// Microsoft's Office servers a few seconds after a start; see
/// docs/trust.md. Every window must use the same arguments: WebView2
/// refuses another set for the same data folder.
pub(crate) const WEBVIEW_ARGUMENTS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,msOneAuthWAM,msForceOneAuthWAM,msOneAuthIDHRD,msLoadOneAuthInBackground,msImplicitSignin,msImplicitSignInNetworkRetry,msM365LinksImplicitSignin,msSeamlessWebToBrowserSignIn,msEnableWebToBrowserSignIn --autoplay-policy=no-user-gesture-required --disable-background-networking --no-proxy-server --disable-component-update";

/// The app's pages and its commands are addressed to the engine as
/// `https://tauri.localhost` and `https://ipc.localhost`, never `http://`
/// (#142). WebView2 answers both itself, but a command in flight when its
/// window closes or reloads is let through to the network once the window
/// is gone: over `http://` it reached port 80 of this computer in clear,
/// where another program (or another user's, on a shared computer) can
/// listen. Over `https://` whoever listens on port 443 gets a TLS greeting
/// and, having no certificate the engine trusts for the name, nothing more.
/// Every window must be built with it: a window on `http://` could not use
/// what the others stored, and its commands would go in clear.
pub(crate) const HTTPS_SCHEME: bool = true;

/// The event that brings the interface the workspace after changes made
/// outside the app.
const WORKSPACE_CHANGED_EVENT: &str = "workspace-changed";

/// The event that brings every window the preferences once changed.
const PREFERENCES_CHANGED_EVENT: &str = "preferences-changed";

/// The event that brings every window a job that has just started (its
/// record, `LiveJob`).
const JOB_STARTED_EVENT: &str = "job-started";

/// The event that brings every window a job's event, numbered
/// (`JobBroadcast`).
const JOB_EVENT: &str = "job-event";

/// The event that asks the main window to show a run docked back from its
/// own window (the run id).
const DOCK_RUN_EVENT: &str = "dock-run";

/// The event that asks the main window to show a host's config history
/// docked back from its own window (the host id).
const DOCK_CONFIGS_EVENT: &str = "dock-configs";

/// The event that asks the main window to open a backup of a host (the
/// host id), from a config history in a window of its own.
const BACK_UP_HOST_EVENT: &str = "back-up-host";

/// The event that tells every window a backup on login kept or checked a
/// configuration (a job's backups arrive as `job-event`s).
const CONFIG_SAVED_EVENT: &str = "config-saved";
/// Told to every window when a session's port forwards change (#21): its
/// header marker and the list of open forwards ask again.
const FORWARDS_CHANGED_EVENT: &str = "forwards-changed";

/// The main window's label; a detached run window's is this prefix and
/// the run id; a detached config history's is its prefix and the host id
/// in hex (a label takes no `.`).
const MAIN_WINDOW: &str = "main";
const RUN_WINDOW_PREFIX: &str = "run-";
const CONFIG_WINDOW_PREFIX: &str = "config-";

/// The credential agent, kept for as long as the app runs.
struct CredentialAgent(#[allow(dead_code)] Option<AgentServer>);

/// Where app-level state lives, the open workspace, and where credential
/// profiles are kept.
struct Shell {
    data_folder: PathBuf,
    /// Held while `state.yaml` is read, changed and written back.
    app_state: Mutex<()>,
    /// Where session logs go: per user, outside every workspace.
    logs: LogFolder,
    /// The keychain, or the vault once its file exists (locked until the
    /// interface's unlock screen unlocks it).
    credentials: Arc<ActiveStore>,
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
        let unprepared = prepare_workspace(&*self.credentials, &mut workspace).err();
        // Hosts whose config history changed folder (#127) get theirs
        // before anything reads it; what stays is said on stderr.
        let claimed = app
            .try_state::<Configs>()
            .map(|configs| configs.0.claim_history(&workspace))
            .unwrap_or_default();
        let root = workspace.root().to_path_buf();
        *self.workspace() = Some(workspace);
        // The plans on screen were the other workspace's (#121).
        if let Some(plans) = app.try_state::<JobPlans>() {
            plans.lock().clear();
        }
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
        for shared in &claimed.shared {
            eprintln!("{shared}");
        }
        for (host, message) in &claimed.left {
            eprintln!("{host}: {message}");
        }
        view
    }

    /// Refuses to open a workspace while the vault is locked: its profiles
    /// could not be prepared. The interface unlocks first.
    fn refuse_if_locked(&self) -> Result<(), String> {
        if self.credentials.status() == VaultStatus::Locked {
            return Err(VaultError::Locked.to_string());
        }
        Ok(())
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
    /// sends it to the interface if anything differs (or it can no longer
    /// be reached, or is another workspace now). The app's own edits are
    /// already loaded, so they send nothing.
    ///
    /// The folder is loaded without holding the workspace (20 000 hosts
    /// take seconds, during which every workspace command would wait);
    /// only taking it is done holding it, and a load that an edit's own
    /// reload overtook meanwhile is dropped.
    fn reload_from_disk(&self, app: &AppHandle, root: &std::path::Path) {
        let Some(revision) = self
            .workspace()
            .as_ref()
            .filter(|w| w.root() == root)
            .map(Workspace::revision)
        else {
            return;
        };
        let fresh = Workspace::open(root);
        let mut guard = self.workspace();
        let Some(workspace) = guard
            .as_mut()
            .filter(|w| w.root() == root && w.revision() == revision)
        else {
            return;
        };
        let result = workspace.refresh_with(fresh);
        let view = (workspace.revision() != revision).then(|| WorkspaceView::from(&*workspace));
        let unreachable = matches!(
            workspace.unavailable(),
            Some(Unavailable::Unreachable { .. })
        );
        drop(guard);
        if let Some(view) = view {
            if let Err(error) = app.emit(WORKSPACE_CHANGED_EVENT, view) {
                eprintln!("cannot send the reloaded workspace: {error}");
            }
        }
        if let Err(error) = result {
            eprintln!("cannot reload the workspace: {error}");
        }
        // Its system watch may have ended unseen (a network share's
        // connection dropped): watched anew once it is back.
        if unreachable {
            if let Some(watcher) = self
                .watcher
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                watcher.check_again();
            }
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
    /// A shell on this computer (#22): the one the preference names, or
    /// `shell` (an id from `local_shells`) when the interface says which.
    Local { shell: Option<String> },
    /// A host of the open workspace through the system's ssh client in a
    /// local shell (#27): for a device the app's own SSH cannot negotiate
    /// with. Marked insecure; not logged.
    SystemSsh { id: HostId },
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
    /// The hosts that reconnect by themselves when a session drops.
    reconnecting_hosts: BTreeSet<HostId>,
    /// Every host's environment and paste line delay, by host id.
    host_safety: BTreeMap<HostId, PasteSafety>,
    /// The saved filters, in file order.
    filters: Vec<SavedFilter>,
    /// The highlight rules, the workspace's own first.
    highlight_rules: Vec<ListedRule>,
    /// The platform definitions, the built-in ones first.
    platforms: Vec<ListedPlatform>,
    /// `Workspace::platform_names`: each usable name and other name (lower
    /// case) with the definition it finds.
    platform_names: BTreeMap<String, PlatformName>,
    /// The snippets, in file order.
    snippets: Vec<Snippet>,
    /// The saved jobs, in file order, each with why it cannot run (if so).
    saved_jobs: Vec<ListedSavedJob>,
    problems: Vec<Problem>,
    /// `Workspace::unavailable`: why what is shown is not what the folder
    /// holds now (it cannot be reached, or is another workspace), if so.
    unavailable: Option<UnavailableView>,
}

/// Why the workspace shown is not what its folder holds now, in the user's
/// words.
#[derive(Serialize, Clone)]
struct UnavailableView {
    #[serde(flatten)]
    unavailable: Unavailable,
    /// What it is, as one line.
    message: String,
    /// What happens meanwhile and what to do.
    explanation: &'static str,
}

/// A saved job as the interface lists it.
#[derive(Serialize, Clone)]
struct ListedSavedJob {
    #[serde(flatten)]
    job: SavedJob,
    /// `job::saved_job_problem`: what is gone, so it cannot run; `None`:
    /// it can be planned.
    problem: Option<String>,
}

impl From<&Workspace> for WorkspaceView {
    fn from(workspace: &Workspace) -> Self {
        Self {
            revision: workspace.revision(),
            root: workspace.root().display().to_string(),
            tree: workspace.tree().clone(),
            roles: workspace.roles().to_vec(),
            host_roles: workspace.host_roles(),
            reconnecting_hosts: workspace.hosts_reconnecting_by_themselves(),
            host_safety: workspace.host_paste_safety(),
            filters: workspace.filters().to_vec(),
            highlight_rules: workspace.highlight_rules().to_vec(),
            platforms: workspace.platforms().to_vec(),
            platform_names: workspace.platform_names(),
            snippets: workspace.snippets().to_vec(),
            saved_jobs: workspace
                .saved_jobs()
                .iter()
                .map(|saved| ListedSavedJob {
                    problem: job::saved_job_problem(workspace, saved).map(|e| e.to_string()),
                    job: saved.clone(),
                })
                .collect(),
            problems: workspace.problems().to_vec(),
            unavailable: workspace.unavailable().map(|unavailable| UnavailableView {
                message: unavailable.to_string(),
                explanation: unavailable.explanation(),
                unavailable: unavailable.clone(),
            }),
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
// Sent once per state change and serialized at once: boxing the closed
// variant would only add noise.
#[allow(clippy::large_enum_variant)]
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
        /// What to check next (`CloseReason::advice`, #116).
        advice: Option<String>,
        /// The next automatic try, when the session reconnects by itself
        /// and this reason allows it (`AutoReconnect`, #24).
        retry: Option<RetryMessage>,
        /// The automatic tries ran out.
        gave_up: bool,
    },
}

/// The next automatic reconnect try: wait, then reconnect.
#[derive(Serialize, Clone)]
struct RetryMessage {
    seconds: u64,
    attempt: usize,
    of: usize,
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
                advice: reason.advice(),
                retry: None,
                gave_up: false,
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

/// Whether `session`'s host reconnects by itself (its `auto_reconnect`
/// setting, #24); the interface tells it when the session shows.
#[tauri::command]
fn session_auto_reconnect_enable(auto: State<'_, AutoReconnects>, id: u64, enabled: bool) {
    auto.enable(SessionId(id), enabled);
}

/// The user pressed a key in a closed session: no more automatic tries.
#[tauri::command]
fn session_auto_reconnect_stop(auto: State<'_, AutoReconnects>, id: u64) {
    auto.stop(SessionId(id));
}

/// The user reconnects a session by hand: the automatic tries start afresh.
#[tauri::command]
fn session_auto_reconnect_restart(auto: State<'_, AutoReconnects>, id: u64) {
    auto.restart(SessionId(id));
}

#[tauri::command]
fn app_name(app: AppHandle) -> String {
    product_name(&app)
}

/// What kind of folder `path` is (#131): the interface asks before opening
/// a folder of other things, which would become a workspace.
#[tauri::command]
fn workspace_folder_kind(path: PathBuf) -> Result<FolderKind, String> {
    folder_kind(resolved_path(&path)).map_err(|error| error.to_string())
}

/// Opens the workspace folder at `path` (an existing folder, which becomes
/// a known workspace) and remembers it for the next launch. A folder of
/// other things ([`FolderKind::Other`]) is opened, and so written into,
/// only once the user said yes (`confirmed`).
#[tauri::command]
fn workspace_open(
    app: AppHandle,
    shell: State<'_, Shell>,
    path: PathBuf,
    confirmed: Option<bool>,
) -> Result<WorkspaceView, String> {
    shell.refuse_if_locked()?;
    // One root for the interface however the path was typed.
    let root = resolved_path(&path);
    if !confirmed.unwrap_or(false) {
        refuse_other_folder(&root)?;
    }
    let workspace = Workspace::open(root).map_err(|error| error.to_string())?;
    shell.open_and_remember(&app, workspace)
}

/// Refuses, before anything is written, to make a folder of other things a
/// workspace without the user's yes.
fn refuse_other_folder(root: &Path) -> Result<(), String> {
    if folder_kind(root).map_err(|error| error.to_string())? == FolderKind::Other {
        return Err(format!(
            "{} holds no host files and is not a workspace; nothing was written into it",
            root.display()
        ));
    }
    Ok(())
}

/// Creates a new, empty workspace in the folder at `path` and opens it.
#[tauri::command]
fn workspace_create(
    app: AppHandle,
    shell: State<'_, Shell>,
    path: PathBuf,
) -> Result<WorkspaceView, String> {
    shell.refuse_if_locked()?;
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
    shell.refuse_if_locked()?;
    let Some(path) = AppState::load(&shell.data_folder).last_workspace else {
        return Ok(None);
    };
    // A folder that is no longer a workspace is not made one again unasked.
    // A folder that cannot be read fails below, in opening's own words.
    if folder_kind(&path).is_ok_and(|kind| kind == FolderKind::Other) {
        return Err(format!(
            "{} holds no host files and is not a workspace any more; nothing was written into \
             it; open it from the workspaces to make it one again",
            path.display()
        ));
    }
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

/// The workspace after renaming or moving a host or folder (#99): the new
/// id or path, the hosts that moved, the references to them that could not
/// be rewritten and the history that did not follow them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Relocated<T> {
    workspace: WorkspaceView,
    result: T,
    moved: Vec<HostMove>,
    not_rewritten: Vec<Problem>,
    left: Vec<LeftHistory>,
}

/// Renames or moves a host or folder of the open workspace, then carries
/// what belongs to the hosts concerned: their session logs and config
/// history (open sessions log to the new place) and the app's records of
/// sessions to them.
fn relocate<T>(
    app: &AppHandle,
    change: impl FnOnce(&mut Workspace) -> Result<Moved<T>, EditError>,
) -> Result<Relocated<T>, String> {
    let shell = app.state::<Shell>();
    let mut guard = shell.workspace();
    let workspace = guard.as_mut().ok_or("no workspace is open")?;
    let moved = change(workspace).map_err(|error| error.to_string())?;
    let records = app.state::<SessionRecords>();
    let carried = history::carry(
        workspace,
        &moved.hosts,
        &shell.logs,
        &app.state::<Configs>().0,
        Some(history::Sessions {
            manager: &app.state::<SessionManager>(),
            records: &records,
        }),
    );
    let sessions_moved = !carried.sessions.is_empty();
    let relocated = Relocated {
        workspace: WorkspaceView::from(&*workspace),
        result: moved.to,
        moved: moved.hosts,
        not_rewritten: moved.not_rewritten,
        left: carried.left,
    };
    drop(guard);
    if sessions_moved {
        session_windows::announce(app, &app.state::<Tabs>(), &records);
    }
    Ok(relocated)
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
fn folder_rename(app: AppHandle, path: String, name: String) -> Result<Relocated<String>, String> {
    relocate(&app, |workspace| workspace.rename_folder(&path, &name))
}

#[tauri::command]
fn folder_move(app: AppHandle, path: String, parent: String) -> Result<Relocated<String>, String> {
    relocate(&app, |workspace| workspace.move_folder(&path, &parent))
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

/// The credential profiles and named secrets of the open workspace, as the
/// interface shows them: without secrets or values.
#[derive(Serialize)]
struct ProfilesView {
    profiles: Vec<ProfileSummary>,
    /// Profile names the workspace uses that are not set up here.
    missing: Vec<String>,
    /// The names of the workspace's named secrets (never their values).
    secrets: Vec<String>,
    /// Nothing can be stored until the vault is turned on: the system has
    /// no keychain the app can use (Linux).
    needs_vault: bool,
}

/// Runs `action` on the open workspace's credential profiles and secrets
/// and returns them afterwards.
fn with_profiles<E: std::fmt::Display>(
    shell: &Shell,
    action: impl FnOnce(&Profiles<'_>, &Secrets<'_>) -> Result<(), E>,
) -> Result<ProfilesView, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let profiles = Profiles::new(&*shell.credentials, workspace);
    let secrets = Secrets::new(&*shell.credentials, workspace);
    action(&profiles, &secrets).map_err(|error| error.to_string())?;
    let text = |error: ProfileError| error.to_string();
    Ok(ProfilesView {
        profiles: profiles.list().map_err(text)?,
        missing: profiles.missing().map_err(text)?,
        secrets: secrets.list().map_err(|error| error.to_string())?,
        needs_vault: shell.credentials.needs_vault(),
    })
}

/// Why the vault did not unlock or turn on, for the interface: `kind` tells
/// a wrong password from a damaged file.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultFailure {
    kind: &'static str,
    message: String,
}

impl From<VaultError> for VaultFailure {
    fn from(error: VaultError) -> Self {
        let kind = match error {
            VaultError::WrongPassword => "wrongPassword",
            VaultError::Damaged(_) => "damaged",
            VaultError::Invalid(_) => "invalid",
            VaultError::NoKeychain => "noKeychain",
            _ => "failed",
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

#[tauri::command]
fn vault_status(shell: State<'_, Shell>) -> VaultStatus {
    shell.credentials.status()
}

/// Whether the system has a keychain the app keeps credentials in while
/// the vault is off; without one (Linux) the vault is the only store.
#[tauri::command]
fn vault_keychain(shell: State<'_, Shell>) -> bool {
    shell.credentials.has_keychain()
}

/// Unlocks the vault for as long as the app runs. The password is used for
/// this call only.
#[tauri::command]
fn vault_unlock(shell: State<'_, Shell>, password: String) -> Result<(), VaultFailure> {
    Ok(shell.credentials.unlock(&password)?)
}

/// Turns the vault on, moving every workspace's profiles and secrets out of
/// the keychain into it.
#[tauri::command]
fn vault_turn_on(
    shell: State<'_, Shell>,
    password: String,
    repeated: String,
) -> Result<VaultStatus, VaultFailure> {
    shell.credentials.turn_on(&password, &repeated)?;
    Ok(shell.credentials.status())
}

/// Changes the master password; `current` must open the vault.
#[tauri::command]
fn vault_change_password(
    shell: State<'_, Shell>,
    current: String,
    password: String,
    repeated: String,
) -> Result<(), VaultFailure> {
    Ok(shell
        .credentials
        .change_password(&current, &password, &repeated)?)
}

/// Turns the vault off (the master password asked again), moving
/// everything back to the keychain.
#[tauri::command]
fn vault_turn_off(shell: State<'_, Shell>, password: String) -> Result<VaultStatus, VaultFailure> {
    shell.credentials.turn_off(&password)?;
    Ok(shell.credentials.status())
}

/// Deletes the locked vault and everything in it (the unlock screen's
/// "Start over", after its confirmation).
#[tauri::command]
fn vault_start_over(shell: State<'_, Shell>) -> Result<VaultStatus, VaultFailure> {
    shell.credentials.start_over()?;
    Ok(shell.credentials.status())
}

/// Quits the app (the unlock screen's Quit).
#[tauri::command]
fn app_quit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn profiles_list(shell: State<'_, Shell>) -> Result<ProfilesView, String> {
    with_profiles(&shell, |_, _| Ok::<_, ProfileError>(()))
}

/// Creates a profile (no `previous`) or changes the one called `previous`.
#[tauri::command]
fn profile_save(
    shell: State<'_, Shell>,
    previous: Option<String>,
    draft: ProfileDraft,
) -> Result<ProfilesView, String> {
    with_profiles(&shell, |profiles, _| {
        profiles.save(previous.as_deref(), &draft).map(drop)
    })
}

#[tauri::command]
fn profile_delete(shell: State<'_, Shell>, name: String) -> Result<ProfilesView, String> {
    with_profiles(&shell, |profiles, _| profiles.delete(&name))
}

/// Adds a named secret (`replace` false) or replaces the value of the one
/// called `name`. The value is never sent back.
#[tauri::command]
fn secret_save(
    shell: State<'_, Shell>,
    name: String,
    value: String,
    replace: bool,
) -> Result<ProfilesView, String> {
    with_profiles(&shell, |_, secrets| {
        if replace {
            secrets.replace(&name, &value)
        } else {
            secrets.add(&name, &value).map(drop)
        }
    })
}

#[tauri::command]
fn secret_delete(shell: State<'_, Shell>, name: String) -> Result<ProfilesView, String> {
    with_profiles(&shell, |_, secrets| secrets.delete(&name))
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
    query(&Profiles::new(&*shell.credentials, workspace)).map_err(|error| error.to_string())
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
    enodix_core::highlight::built_in_rules()
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

/// Saves a platform definition in place of the one called `previous`, or
/// as a new one.
#[tauri::command]
fn platform_save(
    shell: State<'_, Shell>,
    previous: Option<String>,
    definition: PlatformDefinition,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| {
        workspace.save_platform(previous.as_deref(), &definition)
    })
}

/// Deletes a workspace platform definition, or resets a changed built-in.
#[tauri::command]
fn platform_delete(shell: State<'_, Shell>, name: String) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.delete_platform(&name))
}

/// The open workspace's Python scripts (`*.py` files), by path relative to
/// its folder: what a script job can run.
#[tauri::command]
fn workspace_scripts(shell: State<'_, Shell>) -> Result<Vec<String>, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    Ok(workspace.scripts())
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
    /// It has steps (#23): they run on the session through `steps_preview`
    /// and `steps_start`, with the same answers.
    Steps,
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
        let expanded = match workspace.expand_snippet(&name, host.as_ref(), &answers) {
            Err(ExpandError::HasSteps(_)) => workspace
                .plan_snippet_steps(&name, host.as_ref(), &answers)
                .map(|_| Expansion::Steps),
            expanded => expanded.map(|lines| Expansion::Lines { lines }),
        };
        match expanded {
            Ok(expansion) => Ok(expansion),
            Err(ExpandError::Missing(missing)) => Ok(Expansion::Missing {
                names: missing.names,
            }),
            Err(error) => Err(error.to_string()),
        }
    })?
}

/// The snippet `name` for the session `id`, filled from its host (while
/// the open workspace is the one it came from, #86) and `answers`: how a
/// session window fills a snippet for one of its terminals.
#[tauri::command]
fn session_snippet_expand(
    shell: State<'_, Shell>,
    records: State<'_, SessionRecords>,
    id: u64,
    name: String,
    answers: BTreeMap<String, String>,
) -> Result<Expansion, String> {
    let host = read(&shell, |workspace| {
        session_windows::snippet_host(&records, workspace, id)
    })?;
    snippet_expand(shell, name, host, answers)
}

/// A snippet made ready to send to every member of the multi-send group.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum GroupExpansion {
    /// Each member's lines, in the order the members were given.
    Lines { members: Vec<Vec<String>> },
    /// Ask the user for these variables once, then expand again.
    Missing { names: Vec<String> },
}

/// The snippet `name` for each member of the multi-send group: `hosts`
/// holds each member's host (none: not a live inventory host, which gets
/// only the answers); `answers` fill what the hosts do not (#36).
#[tauri::command]
fn snippet_expand_group(
    shell: State<'_, Shell>,
    name: String,
    hosts: Vec<Option<HostId>>,
    answers: BTreeMap<String, String>,
) -> Result<GroupExpansion, String> {
    let answers: Variables = answers.into_iter().collect();
    read(&shell, |workspace| {
        match workspace.expand_snippet_for_each(&name, &hosts, &answers) {
            Ok(members) => Ok(GroupExpansion::Lines { members }),
            Err(ExpandError::Missing(missing)) => Ok(GroupExpansion::Missing {
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
fn host_rename(app: AppHandle, id: HostId, name: String) -> Result<Relocated<HostId>, String> {
    relocate(&app, |workspace| workspace.rename_host(&id, &name))
}

#[tauri::command]
fn host_move(app: AppHandle, id: HostId, folder: String) -> Result<Relocated<HostId>, String> {
    relocate(&app, |workspace| workspace.move_host(&id, &folder))
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

/// The SSH config import last previewed (the file as read, and its target):
/// the import does what the preview showed, even if the file changes
/// meanwhile.
struct SshConfigFiles(Mutex<Option<SshConfigImport>>);

impl SshConfigFiles {
    fn lock(&self) -> MutexGuard<'_, Option<SshConfigImport>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The user's own OpenSSH config file (`.ssh/config` in their home
/// folder), if it is there.
#[tauri::command]
fn ssh_config_default_path() -> Option<PathBuf> {
    SshConfig::default_path()
}

/// Reads an OpenSSH config file and shows what importing its hosts into
/// `target` would do; writes nothing.
#[tauri::command]
fn ssh_config_import_preview(
    shell: State<'_, Shell>,
    files: State<'_, SshConfigFiles>,
    path: PathBuf,
    target: String,
) -> Result<SshConfigPreview, String> {
    let config = SshConfig::read(&path).map_err(|error| error.to_string())?;
    let import = SshConfigImport { config, target };
    let preview =
        read(&shell, |workspace| import.preview(workspace))?.map_err(|error| error.to_string())?;
    *files.lock() = Some(import);
    Ok(preview)
}

/// Runs the import the last preview showed. Existing hosts are only written
/// if listed in `replace` (the user confirmed them).
#[tauri::command]
fn ssh_config_import_run(
    shell: State<'_, Shell>,
    files: State<'_, SshConfigFiles>,
    replace: Vec<HostId>,
) -> Result<Edited<ImportReport>, String> {
    let import = files
        .lock()
        .clone()
        .ok_or("no SSH config file has been previewed")?;
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

/// The Nornir import last previewed (its inventory as read, and its target):
/// the import does what the preview showed, even if the files change
/// meanwhile.
struct NornirFiles(Mutex<Option<NornirImport>>);

impl NornirFiles {
    fn lock(&self) -> MutexGuard<'_, Option<NornirImport>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Reads a Nornir simple inventory (a hosts file, and a groups and a
/// defaults file if given) and shows what importing it into `target` would
/// do; writes nothing.
#[tauri::command]
fn nornir_import_preview(
    shell: State<'_, Shell>,
    files: State<'_, NornirFiles>,
    hosts: PathBuf,
    groups: Option<PathBuf>,
    defaults: Option<PathBuf>,
    target: String,
) -> Result<NornirPreview, String> {
    let inventory = NornirInventory::read(&hosts, groups.as_deref(), defaults.as_deref())
        .map_err(|error| error.to_string())?;
    let import = NornirImport { inventory, target };
    let preview =
        read(&shell, |workspace| import.preview(workspace))?.map_err(|error| error.to_string())?;
    *files.lock() = Some(import);
    Ok(preview)
}

/// Runs the import the last preview showed. Existing hosts are only written
/// if listed in `replace` (the user confirmed them).
#[tauri::command]
fn nornir_import_run(
    shell: State<'_, Shell>,
    files: State<'_, NornirFiles>,
    replace: Vec<HostId>,
) -> Result<Edited<ImportReport>, String> {
    let import = files
        .lock()
        .clone()
        .ok_or("no Nornir inventory has been previewed")?;
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

/// The job plans on screen (#121), each under the id its pre-flight shows:
/// a start names one and runs exactly it, as the user saw and confirmed it,
/// even if the workspace changes meanwhile. Forgotten when its job starts,
/// its tab closes (`job_plan_forget`) or another workspace opens.
#[derive(Default)]
struct JobPlans(Mutex<KeptPlans>);

impl JobPlans {
    fn lock(&self) -> MutexGuard<'_, KeptPlans> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A plan as a pre-flight gets it: with the id to start it by.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShownPlan {
    id: PlanId,
    #[serde(flatten)]
    plan: JobPlan,
}

/// Where job runs are kept: per user, outside every workspace.
struct Runs(RunStore);

/// Where config backups are kept: per user, outside every workspace.
struct Configs(ConfigStore);

/// Where the data was found at start, what moved and what to tell the user
/// (#128); holds the running copy's lock on the data folders.
struct DataMove(data_folders::Settled);

/// What the start found about where the data is that the user should be
/// told (a move that failed or waits, data in both folders); empty when
/// nothing.
#[tauri::command]
fn data_notices(data: State<'_, DataMove>) -> Vec<data_folders::Notice> {
    data.0.notices.clone()
}

/// Why a workspace without an id cannot keep job runs or config backups.
fn no_workspace_id() -> String {
    format!(
        "job results and config backups are kept by the workspace's id, and this workspace has none: see {WORKSPACE_FILE}"
    )
}

/// The open workspace's id, which its job runs are kept under.
fn workspace_id(shell: &Shell) -> Result<String, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    workspace
        .id()
        .map(str::to_string)
        .ok_or_else(no_workspace_id)
}

/// Plans in the open workspace, marks the hosts whose host keys the run
/// would refuse as the app's known hosts are now (#129: advice; the run
/// decides), and keeps the plan for `job_start`, under the id it is shown
/// with.
fn keep_plan(
    shell: &Shell,
    plans: &JobPlans,
    manager: &SessionManager,
    target: JobTarget,
    plan: impl FnOnce(&Workspace, &Profiles<'_>) -> Result<JobPlan, String>,
) -> Result<ShownPlan, String> {
    let kept = {
        let guard = shell.workspace();
        let workspace = guard.as_ref().ok_or("no workspace is open")?;
        let mut plan = plan(workspace, &Profiles::new(&*shell.credentials, workspace))?;
        plan.check_host_keys(&manager.known_hosts());
        KeptPlan {
            workspace: workspace.id().map(str::to_string),
            plan,
            target,
        }
    };
    let plan = kept.plan.clone();
    let id = plans.lock().keep(kept);
    Ok(ShownPlan { id, plan })
}

/// The pre-flight list of running `step` on `target` in the open
/// workspace: who runs, who is skipped and why, the production hosts, and
/// the snippet variables still to ask for. Connects to nothing; kept for
/// `job_start` under the id it carries.
#[tauri::command]
fn job_plan(
    shell: State<'_, Shell>,
    plans: State<'_, JobPlans>,
    manager: State<'_, SessionManager>,
    target: JobTarget,
    step: JobStep,
) -> Result<ShownPlan, String> {
    keep_plan(
        &shell,
        &plans,
        &manager,
        target.clone(),
        |workspace, profiles| {
            job::plan(workspace, profiles, &target, &step).map_err(|error| error.to_string())
        },
    )
}

/// Forgets the kept plan `plan` (its tab closed, or planned again).
#[tauri::command]
fn job_plan_forget(plans: State<'_, JobPlans>, plan: PlanId) {
    plans.lock().forget(plan);
}

/// A kept run planned again for "Run again".
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlannedAgain {
    target: JobTarget,
    plan: ShownPlan,
}

/// Plans the kept run `run` again on its target, as the open workspace is
/// now; kept for `job_start` like `job_plan`'s.
#[tauri::command]
fn job_run_again(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    plans: State<'_, JobPlans>,
    manager: State<'_, SessionManager>,
    run: String,
) -> Result<PlannedAgain, String> {
    let stored = runs
        .0
        .run(&workspace_id(&shell)?, &run)
        .map_err(|error| error.to_string())?;
    let plan = keep_plan(
        &shell,
        &plans,
        &manager,
        stored.target.clone(),
        |workspace, profiles| {
            job::plan_again(workspace, profiles, &stored).map_err(|error| error.to_string())
        },
    )?;
    Ok(PlannedAgain {
        target: stored.target,
        plan,
    })
}

/// Saves a job under a name in the workspace's jobs file, in place of the
/// one called `previous` (a rename or a change), or as a new one.
#[tauri::command]
fn saved_job_save(
    shell: State<'_, Shell>,
    previous: Option<String>,
    job: SavedJob,
) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| {
        workspace.save_job(previous.as_deref(), &job)
    })
}

/// The variables of `step`'s answers a saved job does not keep (#126):
/// their names look like secrets, so they are asked at each run.
#[tauri::command]
fn saved_job_secret_answers(step: JobStep) -> Vec<String> {
    enodix_core::workspace::secret_answers(&step)
}

/// Deletes the saved job `name` from the workspace's jobs file.
#[tauri::command]
fn saved_job_delete(shell: State<'_, Shell>, name: String) -> Result<Edited<()>, String> {
    edit(&shell, |workspace| workspace.delete_job(&name))
}

/// Plans the saved job `name` as the open workspace is now (refused, saying
/// what is gone, if its step or target is); kept for `job_start` like
/// `job_plan`'s.
#[tauri::command]
fn saved_job_plan(
    shell: State<'_, Shell>,
    plans: State<'_, JobPlans>,
    manager: State<'_, SessionManager>,
    name: String,
) -> Result<PlannedAgain, String> {
    let (target, plan) = {
        let guard = shell.workspace();
        let workspace = guard.as_ref().ok_or("no workspace is open")?;
        job::plan_saved(
            workspace,
            &Profiles::new(&*shell.credentials, workspace),
            &name,
        )
        .map_err(|error| error.to_string())?
    };
    let plan = keep_plan(&shell, &plans, &manager, target.clone(), |_, _| Ok(plan))?;
    Ok(PlannedAgain { target, plan })
}

/// A started job: its id (for `job_cancel`) and its kept run's.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartedJob {
    id: u64,
    run: String,
}

/// The record of live jobs, shared by every window (#77).
#[derive(Default)]
struct Live(Mutex<LiveJobs>);

impl Live {
    fn lock(&self) -> MutexGuard<'_, LiveJobs> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A job's event as every window gets it: `number` counts from 1, so a
/// window that read the job's record applies only later ones.
#[derive(Serialize, Clone)]
struct JobBroadcast {
    id: u64,
    number: u64,
    event: JobEvent,
}

/// Every job started in this app run and not forgotten, with what it has
/// reported so far.
#[tauri::command]
fn jobs_live(live: State<'_, Arc<Live>>) -> Vec<LiveJob> {
    live.lock().jobs().to_vec()
}

/// Forgets the finished jobs other than the one keeping the run `keep`
/// (their runs stay on disk).
#[tauri::command]
fn jobs_forget_finished(live: State<'_, Arc<Live>>, keep: Option<String>) {
    live.lock().forget_finished(keep.as_deref());
}

/// Starts the kept plan `plan` (the one its pre-flight showed), with its
/// own sessions (never logged, never a terminal's), at most `parallel`
/// hosts at once (default: the plan's), keeping its run on disk. Refused
/// if the plan is no longer kept (plan again), or if it has production
/// hosts that `confirmed_production` (the production hosts its pre-flight's
/// user confirmed) does not all name. It is recorded in `Live` (`label` =
/// what it runs on) and announced to every window (`job-started`), and its
/// progress is broadcast (`job-event`) until `finished`, whoever listens.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn job_start(
    app: AppHandle,
    shell: State<'_, Shell>,
    manager: State<'_, SessionManager>,
    plans: State<'_, JobPlans>,
    runs: State<'_, Runs>,
    configs: State<'_, Configs>,
    jobs: State<'_, RunningJobs>,
    live: State<'_, Arc<Live>>,
    python: State<'_, Arc<python::PythonSetup>>,
    plan: PlanId,
    confirmed_production: Vec<HostId>,
    parallel: Option<usize>,
    label: String,
) -> Result<StartedJob, String> {
    let root = shell
        .workspace()
        .as_ref()
        .map(|workspace| workspace.root().display().to_string())
        .ok_or("no workspace is open")?;
    let peeked = plans
        .lock()
        .ready(plan, &confirmed_production)
        .map_err(|refused| refused.to_string())?
        .plan
        .clone();
    // A script job runs with the interpreter as it is now (checked here,
    // which takes a moment, before the plan is taken).
    let script = match peeked.step {
        ref step if step.runs_script() => {
            let python = python.inner().clone();
            Some(
                tauri::async_runtime::spawn_blocking(move || python.script_options())
                    .await
                    .map_err(|error| error.to_string())??,
            )
        }
        _ => None,
    };
    let mut plans = plans.lock();
    // Kept since the look before the interpreter check (a plan under an
    // id never changes).
    let planned = plans
        .ready(plan, &confirmed_production)
        .map_err(|refused| refused.to_string())?
        .clone();
    let workspace = planned.workspace.clone().ok_or_else(no_workspace_id)?;
    let options = JobOptions {
        parallel: parallel.unwrap_or(planned.plan.default_parallel),
        configs: Some(configs.0.clone()),
        script,
        ..JobOptions::default()
    };
    // A start that would be refused keeps no run.
    job::check(&planned.plan, &options).map_err(|error| error.to_string())?;
    let recorder = runs
        .0
        .begin(&workspace, &planned.target, &planned.plan)
        .map_err(|error| format!("the run cannot be kept, so the job did not start: {error}"))?;
    let started = planned.plan.clone();
    let mut job = job::start(&manager, planned.plan, options).map_err(|e| e.to_string())?;
    // Started: the plan is used up (a refused start keeps it).
    plans.forget(plan);
    drop(plans);
    let plan = started;
    // Running until its end is kept; closing the app ends it (#124).
    let mut recorder = jobs.follow(job.canceller(), recorder);
    let id = recorder.id();
    let run = recorder.run().to_string();
    let record = live
        .lock()
        .start(NewLiveJob {
            id,
            workspace: root,
            run: run.clone(),
            label,
            target: planned.target,
            plan,
        })
        .clone();
    let _ = app.emit(JOB_STARTED_EVENT, &record);
    let live = Arc::clone(&live);
    tauri::async_runtime::spawn(async move {
        let mut unkept = false;
        // The job keeps running whether or not a window listens.
        while let Some(event) = job.next().await {
            if let Err(error) = recorder.record(&event) {
                if !unkept {
                    eprintln!("the job run {} is not kept whole: {error}", recorder.run());
                    unkept = true;
                }
            }
            let number = live.lock().record(id, &event);
            if let Some(number) = number {
                let _ = app.emit(JOB_EVENT, JobBroadcast { id, number, event });
            }
        }
    });
    Ok(StartedJob { id, run })
}

/// Cancels the running job `id`: its sessions close, unfinished hosts end
/// cancelled with the output they had. Nothing happens if it has ended.
#[tauri::command]
fn job_cancel(jobs: State<'_, RunningJobs>, id: u64) {
    jobs.cancel(id);
}

/// The open workspace's kept runs, and which of them still run.
#[derive(Serialize)]
struct RunsView {
    #[serde(flatten)]
    list: RunList,
    running: Vec<String>,
}

/// The open workspace's kept runs, newest first (none without an id).
#[tauri::command]
fn job_runs(shell: State<'_, Shell>, runs: State<'_, Runs>) -> Result<RunsView, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let Some(id) = workspace.id() else {
        return Ok(RunsView {
            list: RunList {
                runs: Vec::new(),
                damaged: Vec::new(),
            },
            running: Vec::new(),
        });
    };
    Ok(RunsView {
        list: runs.0.list(id).map_err(|error| error.to_string())?,
        running: runs.0.running(id),
    })
}

/// The preview last made of a "Fill in from devices" run: applying it
/// writes what it showed (compared again with the hosts as they are then).
struct EnrichPreviews(Mutex<Option<KeptPreview>>);

/// A preview with the workspace (its root) and the run it was made for.
struct KeptPreview {
    root: PathBuf,
    run: String,
    preview: EnrichPreview,
}

impl EnrichPreviews {
    fn lock(&self) -> MutexGuard<'_, Option<KeptPreview>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What filling in the facts of the kept run `run` would change in the
/// open workspace; kept for `enrich_apply`.
#[tauri::command]
fn enrich_preview(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    previews: State<'_, EnrichPreviews>,
    run: String,
) -> Result<EnrichPreview, String> {
    let stored = runs
        .0
        .run(&workspace_id(&shell)?, &run)
        .map_err(|error| error.to_string())?;
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let preview = enrich::preview_run(workspace, &stored).map_err(|error| error.to_string())?;
    *previews.lock() = Some(KeptPreview {
        root: workspace.root().to_path_buf(),
        run,
        preview: preview.clone(),
    });
    Ok(preview)
}

/// Fills in the preview made last of the run `run`: every empty field, and
/// typed values only on the hosts in `replace`.
#[tauri::command]
fn enrich_apply(
    shell: State<'_, Shell>,
    previews: State<'_, EnrichPreviews>,
    run: String,
    replace: Vec<HostId>,
) -> Result<Edited<EnrichReport>, String> {
    let mut guard = shell.workspace();
    let workspace = guard.as_mut().ok_or("no workspace is open")?;
    // Only the preview of this run in this workspace: another workspace
    // may have hosts with the same ids.
    let preview = match previews.lock().as_ref() {
        Some(kept) if kept.run == run && kept.root == workspace.root() => kept.preview.clone(),
        _ => return Err("this run has not been previewed here: review it again".into()),
    };
    let report = enrich::apply(workspace, &preview, &replace);
    Ok(Edited {
        workspace: WorkspaceView::from(&*workspace),
        result: report,
    })
}

/// One kept run of the open workspace.
#[tauri::command]
fn job_run(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    run: String,
) -> Result<StoredRun, String> {
    runs.0
        .run(&workspace_id(&shell)?, &run)
        .map_err(|error| error.to_string())
}

/// One host's output in a kept run of the open workspace.
#[tauri::command]
fn job_run_output(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    output: OutputRef,
) -> Result<String, String> {
    runs.0
        .output(&workspace_id(&shell)?, &output)
        .map_err(|error| error.to_string())
}

/// The hosts of a kept run whose output holds `text`, ignoring case.
#[tauri::command]
fn job_run_search(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    run: String,
    text: String,
) -> Result<Vec<SearchHit>, String> {
    runs.0
        .search(&workspace_id(&shell)?, &run, &text)
        .map_err(|error| error.to_string())
}

/// A line diff of two hosts' output (of one run, or of two).
#[tauri::command]
fn job_compare(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    old: OutputRef,
    new: OutputRef,
) -> Result<Vec<DiffLine>, String> {
    runs.0
        .compare(&workspace_id(&shell)?, &old, &new)
        .map_err(|error| error.to_string())
}

/// Copies a kept run into `folder`; the copy's path.
#[tauri::command]
fn job_run_export(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    run: String,
    folder: PathBuf,
) -> Result<String, String> {
    runs.0
        .export(&workspace_id(&shell)?, &run, &folder)
        .map(|copy| copy.display().to_string())
        .map_err(|error| error.to_string())
}

/// Deletes a kept run from disk; refused while it runs.
#[tauri::command]
fn job_run_delete(
    shell: State<'_, Shell>,
    runs: State<'_, Runs>,
    run: String,
) -> Result<(), String> {
    runs.0
        .delete(&workspace_id(&shell)?, &run)
        .map_err(|error| error.to_string())
}

/// The kept versions of the open workspace's host `host`'s configuration,
/// newest first (none without a workspace id).
#[tauri::command]
fn config_versions(
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
    host: HostId,
) -> Result<Vec<ConfigVersion>, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let Some(id) = workspace.id() else {
        return Ok(Vec::new());
    };
    configs
        .0
        .versions(id, &host)
        .map_err(|error| error.to_string())
}

/// The open workspace's hosts that have kept configurations, in tree order
/// (hosts, then subfolders), each with what its last backup did (none
/// without a workspace id).
#[tauri::command]
fn config_backed_up(
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
) -> Result<Vec<BackedUpHost>, String> {
    fn walk(folder: &enodix_core::workspace::Folder, ids: &mut Vec<HostId>) {
        ids.extend(folder.hosts.iter().map(|host| host.id.clone()));
        for child in &folder.folders {
            walk(child, ids);
        }
    }
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let Some(id) = workspace.id() else {
        return Ok(Vec::new());
    };
    let mut hosts = Vec::new();
    walk(workspace.tree(), &mut hosts);
    configs
        .0
        .backed_up(id, &hosts)
        .map_err(|error| error.to_string())
}

/// The text of one kept version of a host's configuration.
#[tauri::command]
fn config_version(
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
    host: HostId,
    version: String,
) -> Result<String, String> {
    configs
        .0
        .read(&workspace_id(&shell)?, &host, &version)
        .map_err(|error| error.to_string())
}

/// A line diff of two kept versions of a host's configuration.
#[tauri::command]
fn config_diff(
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
    host: HostId,
    old: String,
    new: String,
) -> Result<Vec<DiffLine>, String> {
    configs
        .0
        .diff(&workspace_id(&shell)?, &host, &old, &new)
        .map_err(|error| error.to_string())
}

/// Copies one kept version of a host's configuration into `folder`; the
/// copy's path.
#[tauri::command]
fn config_export(
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
    host: HostId,
    version: String,
    folder: PathBuf,
) -> Result<String, String> {
    configs
        .0
        .export(&workspace_id(&shell)?, &host, &version, &folder)
        .map(|copy| copy.display().to_string())
        .map_err(|error| error.to_string())
}

/// Opens a session. Output arrives on `on_output` as raw bytes, already
/// batched by the core, state changes on `on_state`, questions the server
/// asks while logging in on `on_prompt` (answered with `session_answer`),
/// host keys to decide on on `on_host_key` (decided with
/// `session_decide_host_key`), and the algorithms each connection agreed on
/// (just before `connected`) on `on_algorithms`, and why its log could not
/// be written (the session goes on) on `on_log_failure`; terminal data
/// never goes through the event system.
/// The events reach the interface through the session's relay (#83), and
/// the interface acknowledges output with `session_acknowledge`, naming
/// the consumer key returned here. The session is shown in the window that
/// opens it, in its tab `tab`.
/// `name` and `title` are what its tab and header show: the app keeps them
/// with the host's facts as its record (`SessionRecord`, #84), which any
/// window showing the session reads.
/// Returns the session's id, the consumer key, whether the session is
/// encrypted (telnet is not) and its record.
// One argument per channel is how Tauri commands receive them.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn session_open(
    app: AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, SessionManager>,
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
    target: ConnectTarget,
    tab: TabId,
    name: String,
    title: String,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
    on_backup: Channel<LoginBackupOutcome>,
    python: State<'_, Arc<python::PythonSetup>>,
    preferred: State<'_, UserPreferences>,
) -> Result<Opened, String> {
    let window = calling_window(&window)?;
    let (made, options, record) = match target {
        ConnectTarget::Quick {
            host,
            port,
            username,
            password,
        } => {
            let log = Some(shell.logs.quick_log(&host));
            let target = SessionTarget::Ssh(SshTarget {
                host,
                port,
                username,
                auth: SshAuth::Password(password),
                jump_hosts: Vec::new(),
                // Quick connections have no settings to switch it on.
                legacy_crypto: false,
                connect_timeout: DEFAULT_CONNECT_TIMEOUT,
                terminal_type: TerminalType::default(),
            });
            (
                Made::Quick(target),
                quick_options(log),
                SessionRecord::new(&name, &title),
            )
        }
        ConnectTarget::QuickSerial { port, parameters } => {
            let log = Some(shell.logs.quick_log(&port));
            let target = SessionTarget::Serial(SerialTarget { port, parameters });
            (
                Made::Quick(target),
                quick_options(log),
                SessionRecord::new(&name, &title),
            )
        }
        ConnectTarget::Local { shell: which } => {
            let chosen = which.unwrap_or_else(|| preferred.lock().local_shell.clone());
            let shells = local_shells();
            let picked = pick_local_shell(&shells, Some(&chosen))
                .ok_or("this computer has no shell to open")?;
            let (folder, environment) = local_start(&shell, &python);
            let target = SessionTarget::Local(picked.target(folder, environment));
            // Not logged: it shows the user's own computer, not a device.
            (
                Made::Quick(target),
                OpenOptions::default(),
                SessionRecord::new(&name, &title),
            )
        }
        ConnectTarget::SystemSsh { id } => {
            let guard = shell.workspace();
            let workspace = guard.as_ref().ok_or("no workspace is open")?;
            let profiles = Profiles::new(&*shell.credentials, workspace);
            let host = profiles
                .system_ssh_host(&id)
                .map_err(|error| error.to_string())?;
            let (folder, environment) = local_start(&shell, &python);
            let target = SessionTarget::Local(system_ssh_target(None, &host, folder, environment));
            let record = SessionRecord::for_host(workspace, &id, Transport::Local, &name, &title)
                .unwrap_or_else(|| SessionRecord::new(&name, &title))
                .insecure(SYSTEM_SSH_WARNING);
            // Not logged: it is not an enodix session to a device.
            (Made::Quick(target), OpenOptions::default(), record)
        }
        ConnectTarget::Host { id, credentials } => {
            let typed = credentials
                .map(|Prompted { username, password }| TypedLogin { username, password });
            host_made(&shell, &configs, id, HostWay::Ssh { typed }, &name, &title)?
        }
        ConnectTarget::Telnet { id } => {
            host_made(&shell, &configs, id, HostWay::Telnet, &name, &title)?
        }
        ConnectTarget::Serial { id } => {
            host_made(&shell, &configs, id, HostWay::Serial, &name, &title)?
        }
    };
    let (encrypted, transport) = match &made {
        Made::Quick(target) | Made::Host(HostSession { target, .. }) => {
            (target.encrypted(), target.transport())
        }
    };
    let (id, events) = match made {
        Made::Quick(target) => manager.open_with(target, size.into(), options),
        Made::Host(session) => manager.open_host(session, size.into(), options),
    };
    records.record(id, record.clone());
    tabs.attach(window, tab, id, transport);
    let consumer = relays.start(
        id,
        window,
        events,
        Channels {
            session: id,
            output: on_output,
            state: on_state,
            prompt: on_prompt,
            host_key: on_host_key,
            algorithms: on_algorithms,
            log_failure: on_log_failure,
            backup: on_backup,
            app,
        },
    );
    Ok(Opened {
        id: id.0,
        consumer: consumer.get(),
        encrypted,
        transport,
        record,
    })
}

/// What a session is opened to.
enum Made {
    /// A quick connection: reconnects as made.
    Quick(SessionTarget),
    /// A workspace host, as the core worked it out (#130: reconnecting
    /// works it out again).
    Host(HostSession),
}

/// A quick connection is always logged, and backs nothing up. Its
/// terminal shows the login banner (#134).
fn quick_options(log: Option<SessionLog>) -> OpenOptions {
    OpenOptions {
        log,
        login_banner: true,
        ..OpenOptions::default()
    }
}

/// Where a program the app starts on this computer begins and what it is
/// told: the workspace folder (else the home folder), the workspace and the
/// headless binary in its environment, so that the Python bridge works in it.
fn local_start(
    shell: &Shell,
    python: &python::PythonSetup,
) -> (Option<std::path::PathBuf>, Vec<(String, String)>) {
    let root = shell
        .workspace()
        .as_ref()
        .map(|workspace| workspace.root().to_path_buf());
    (
        local_start_folder(root.as_deref()),
        local_environment(
            root.as_deref(),
            python.cli().as_deref(),
            vec![(
                DATA_DIR_VARIABLE.into(),
                python.data_folder().display().to_string(),
            )],
        ),
    )
}

/// A session to the open workspace's host `id`, reached `way`, with its
/// options and record, as the host is now.
fn host_made(
    shell: &Shell,
    configs: &Configs,
    id: HostId,
    way: HostWay,
    name: &str,
    title: &str,
) -> Result<(Made, OpenOptions, SessionRecord), String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    let profiles = Profiles::new(&*shell.credentials, workspace);
    let session = profiles
        .host_session(&id, way)
        .map_err(|error| error.to_string())?;
    let options = host_options(shell, configs, &profiles, &session);
    let transport = session.target.transport();
    let record = SessionRecord::for_host(workspace, &id, transport, name, title)
        .unwrap_or_else(|| SessionRecord::new(name, title));
    Ok((Made::Host(session), options, record))
}

/// What a session to a workspace host does beside connecting, as the
/// host's settings say now: where it is logged (`logging`), and, over SSH,
/// the backup it makes once connected (`backup_on_login`; the core decides
/// when it applies).
fn host_options(
    shell: &Shell,
    configs: &Configs,
    profiles: &Profiles<'_>,
    session: &HostSession,
) -> OpenOptions {
    let workspace = profiles.workspace();
    let host = session.origin.host();
    let when_connected = match session.target.transport() {
        Transport::Ssh => {
            LoginBackup::plan(workspace, profiles, host, session.login, configs.0.clone())
                .map(|backup| Arc::new(backup) as Arc<dyn WhenConnected>)
        }
        Transport::Telnet | Transport::Serial | Transport::Local => None,
    };
    // The host's port forwards (#21), over its SSH connection.
    let forwards = match session.target.transport() {
        Transport::Ssh => workspace
            .resolve_host(host)
            .map(|host| host.port_forwards())
            .unwrap_or_default(),
        Transport::Telnet | Transport::Serial | Transport::Local => Vec::new(),
    };
    OpenOptions {
        log: shell.logs.host_log(workspace, host),
        when_connected,
        // A terminal shows the login banner (#134); the backup's channel
        // or connection does not.
        login_banner: true,
        forwards,
        encoding: workspace
            .resolve_host(host)
            .map(|host| host.settings.encoding.value)
            .unwrap_or_default(),
    }
}

/// Shows the folder of the host `id`'s session logs in the system's file
/// manager, creating it if no session has been logged yet.
#[tauri::command]
fn logs_open_folder(app: AppHandle, shell: State<'_, Shell>, id: HostId) -> Result<(), String> {
    let folder = read(&shell, |workspace| {
        shell.logs.create_host_folder(workspace, &id)
    })?
    .map_err(|error| format!("cannot create the log folder: {error}"))?;
    let opened = if enodix_core::opener::in_a_bundle() {
        // From an AppImage: the file manager must not get its variables.
        enodix_core::opener::open_detached(&folder).map_err(|error| error.to_string())
    } else {
        app.opener()
            .open_path(folder.display().to_string(), None::<&str>)
            .map_err(|error| error.to_string())
    };
    opened.map_err(|error| format!("cannot open the log folder {}: {error}", folder.display()))
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
    open_in_browser(&app, &url)?;
    Ok(url)
}

/// Opens `url` (already checked by the core) in the default browser, or
/// records it in the `ENODIX_WEB_LINKS_TO` file.
fn open_in_browser(app: &AppHandle, url: &str) -> Result<(), String> {
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
        // From an AppImage: the browser must not get its variables.
        None if enodix_core::opener::in_a_bundle() => enodix_core::opener::open_detached(url)
            .map_err(|error| format!("cannot open {url} in the browser: {error}"))?,
        None => app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|error| format!("cannot open {url} in the browser: {error}"))?,
    }
    Ok(())
}

/// A session just opened, as the interface receives it.
#[derive(Serialize)]
struct Opened {
    id: u64,
    /// The key that acknowledges its output (`session_acknowledge`).
    consumer: u64,
    /// Whether its traffic is encrypted on the network; telnet's is not.
    encrypted: bool,
    /// `ssh`, `telnet` or `serial` (serial and telnet sessions can send a break).
    transport: Transport,
    /// What the app keeps of it, for any window showing it (#84).
    record: SessionRecord,
}

/// Where a session's events go in the interface.
struct Channels {
    /// The session these channels show.
    session: SessionId,
    output: Channel<InvokeResponseBody>,
    state: Channel<StateMessage>,
    prompt: Channel<LoginPrompt>,
    host_key: Channel<HostKeyQuestion>,
    algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    /// Why the session's log could not be written.
    log_failure: Channel<String>,
    /// How its backup on login ended.
    backup: Channel<LoginBackupOutcome>,
    /// Every window hears of a backup on login (`config-saved`), so a
    /// config history in a window of its own refreshes too (#78).
    app: AppHandle,
}

/// A window's channels, as the consumer of a session's relay
/// (`SessionRelays`, #83). A failed send means the webview is gone.
impl Consumer for Channels {
    fn deliver(&mut self, event: SessionEvent) -> Result<(), ConsumerGone> {
        let delivered = match event {
            SessionEvent::Output(bytes) => self.output.send(InvokeResponseBody::Raw(bytes)),
            SessionEvent::State(state) => {
                let auto = self.app.state::<AutoReconnects>();
                let mut message = StateMessage::from(state.clone());
                match (&state, &mut message) {
                    (SessionState::Connected, _) => auto.connected(self.session),
                    (SessionState::Closed(reason), StateMessage::Closed { retry, gave_up, .. }) => {
                        *retry = auto
                            .after_close(self.session, reason)
                            .map(|next| RetryMessage {
                                seconds: next.after.as_secs(),
                                attempt: next.attempt,
                                of: next.of,
                            });
                        *gave_up = auto.gave_up(self.session);
                    }
                    _ => {}
                }
                self.state.send(message)
            }
            SessionEvent::Prompt(prompt) => self.prompt.send(prompt),
            SessionEvent::HostKey(question) => self.host_key.send(question),
            SessionEvent::Algorithms(algorithms) => self.algorithms.send(algorithms),
            SessionEvent::LogFailed(failure) => self.log_failure.send(failure.to_string()),
            SessionEvent::Backup(outcome) => {
                let _ = self.app.emit(CONFIG_SAVED_EVENT, ());
                self.backup.send(outcome)
            }
            // Read back with `session_forwards` / `forwards_list`.
            SessionEvent::Forwards(_) => {
                let _ = self.app.emit(FORWARDS_CHANGED_EVENT, ());
                Ok(())
            }
        };
        delivered.map_err(|_| ConsumerGone)
    }
}

/// The window a layout or session command came from, as the core names
/// it: the window is always the caller, never an argument, so a page
/// cannot edit another window's layout (#83). The main window, or a
/// session window (`session-<n>`, #84).
fn calling_window(window: &tauri::WebviewWindow) -> Result<WindowId, String> {
    let label = window.label();
    if label == MAIN_WINDOW {
        return Ok(WindowId::MAIN);
    }
    session_windows::window_of(label).ok_or_else(|| format!("the window {label} shows no sessions"))
}

/// A reconnect that has started, as the interface receives it.
#[derive(Serialize)]
struct Reconnected {
    /// The key that acknowledges its output (`session_acknowledge`).
    consumer: u64,
    /// When it logs in otherwise than the session did, or reconnects as
    /// made: what the terminal tells the user (#130).
    note: Option<String>,
}

/// Why a reconnect did not start, as the interface shows it: the reason,
/// and what to do about it.
#[derive(Serialize)]
struct ReconnectFailure {
    reason: String,
    advice: Option<String>,
}

impl From<String> for ReconnectFailure {
    fn from(reason: String) -> Self {
        Self {
            reason,
            advice: None,
        }
    }
}

/// Opens a session that has closed again, in place, the same id. A
/// session to a workspace host works its target out again from the host's
/// settings and credential profile as they are now (#130, the core's
/// `Reconnection::work_out`): a host gone, or no longer reached that way,
/// is refused with the reason; a session of a workspace not open, or a
/// quick connection, reconnects as it was made. Its events arrive on new
/// channels, through a new relay, as for `session_open`; returns the new
/// consumer key and any note for the terminal.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn session_reconnect(
    app: AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, SessionManager>,
    shell: State<'_, Shell>,
    configs: State<'_, Configs>,
    relays: State<'_, SessionRelays>,
    id: u64,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
    on_backup: Channel<LoginBackupOutcome>,
) -> Result<Reconnected, ReconnectFailure> {
    let window = calling_window(&window)?;
    let session = SessionId(id);
    let failed = |error: enodix_core::session::SessionError| error.to_string();
    // A quick connection has no origin: it reconnects as made.
    let worked_out = match manager.origin(session).map_err(failed)? {
        None => None,
        Some(origin) => {
            let guard = shell.workspace();
            let open = guard
                .as_ref()
                .map(|workspace| Profiles::new(&*shell.credentials, workspace));
            match Reconnection::work_out(&origin, open.as_ref()) {
                Err(refused) => {
                    return Err(ReconnectFailure {
                        reason: refused.to_string(),
                        advice: refused.advice(),
                    })
                }
                Ok(Reconnection::AsMade { note }) => Some((None, note)),
                Ok(Reconnection::Again(host)) => {
                    let options = open
                        .as_ref()
                        .map(|profiles| host_options(&shell, &configs, profiles, &host))
                        .unwrap_or_default();
                    let note = host.note.clone();
                    Some((Some((host, options)), note.unwrap_or_default()))
                }
            }
        }
    };
    let (events, note) = match worked_out {
        Some((Some((host, options)), note)) => (
            manager.reconnect_host(session, host, size.into(), options),
            Some(note),
        ),
        Some((None, note)) => (manager.reconnect(session, size.into()), Some(note)),
        None => (manager.reconnect(session, size.into()), None),
    };
    let events = events.map_err(failed)?;
    let note = note.filter(|note| !note.is_empty());
    let consumer = relays.start(
        SessionId(id),
        window,
        events,
        Channels {
            session: SessionId(id),
            output: on_output,
            state: on_state,
            prompt: on_prompt,
            host_key: on_host_key,
            algorithms: on_algorithms,
            log_failure: on_log_failure,
            backup: on_backup,
            app,
        },
    );
    Ok(Reconnected {
        consumer: consumer.get(),
        note,
    })
}

/// Opens a new session to the same target as the session `id`, with the
/// same settings and login; returns it as `session_open` does. Events as
/// for `session_open`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn session_duplicate(
    app: AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, SessionManager>,
    tabs: State<'_, Tabs>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
    id: u64,
    tab: TabId,
    size: Size,
    on_output: Channel<InvokeResponseBody>,
    on_state: Channel<StateMessage>,
    on_prompt: Channel<LoginPrompt>,
    on_host_key: Channel<HostKeyQuestion>,
    on_algorithms: Channel<Vec<NegotiatedAlgorithms>>,
    on_log_failure: Channel<String>,
    on_backup: Channel<LoginBackupOutcome>,
) -> Result<Opened, String> {
    let window = calling_window(&window)?;
    let (copy, events) = manager
        .duplicate(SessionId(id), size.into())
        .map_err(|error| error.to_string())?;
    records.copy(SessionId(id), copy);
    let transport = manager.transport(copy).map_err(|error| error.to_string())?;
    tabs.attach(window, tab, copy, transport);
    let consumer = relays.start(
        copy,
        window,
        events,
        Channels {
            session: copy,
            output: on_output,
            state: on_state,
            prompt: on_prompt,
            host_key: on_host_key,
            algorithms: on_algorithms,
            log_failure: on_log_failure,
            backup: on_backup,
            app,
        },
    );
    let encrypted = manager.encrypted(copy).map_err(|error| error.to_string())?;
    let transport = manager.transport(copy).map_err(|error| error.to_string())?;
    let record = records
        .get(copy)
        .unwrap_or_else(|| SessionRecord::new("", ""));
    Ok(Opened {
        id: copy.0,
        consumer: consumer.get(),
        encrypted,
        transport,
        record,
    })
}

/// Every window's tabs and panes, and the app's multi-send group (#83).
struct Tabs {
    windows: Mutex<Windows>,
    /// The group's revision the windows were last told (#87).
    told: AtomicU64,
}

impl Tabs {
    fn new() -> Self {
        Self {
            windows: Mutex::new(Windows::new()),
            told: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Windows> {
        self.windows.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records the session `window`'s `tab` shows. A tab closed while its
    /// session was opening is gone; the interface discards that session.
    fn attach(&self, window: WindowId, tab: TabId, session: SessionId, transport: Transport) {
        let _ = self
            .lock()
            .attach_transport(window, tab, session, transport);
    }
}

/// A change to the window's tabs and panes, as `Layout`'s methods name
/// them, or to the multi-send group, as `Windows`' do.
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum LayoutEdit {
    OpenTab,
    /// Shows a view: its tab if open, else a new one (#68).
    OpenView {
        view: String,
        subject: Option<String>,
    },
    /// The view `view` about `from` is about `to` now (its host moved).
    RetargetView {
        view: String,
        from: String,
        to: String,
    },
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
    /// `confirmed`: the user answered the join question (#123); a
    /// production session joins only then.
    JoinGroup {
        tab: TabId,
        confirmed: bool,
    },
    JoinPane {
        pane: PaneId,
        confirmed: bool,
    },
    LeaveGroup {
        tab: TabId,
    },
    EndGroup,
}

/// The layout after an edit, and the tab or pane the edit created.
#[derive(Serialize)]
struct LayoutChanged {
    layout: WindowView,
    created: Option<u64>,
}

/// Starts the calling window's layout afresh: one empty pane (the
/// interface's sessions start afresh too when it loads); its tabs leave
/// the multi-send group.
#[tauri::command]
fn layout_reset(
    app: AppHandle,
    window: tauri::WebviewWindow,
    tabs: State<'_, Tabs>,
    files: State<'_, files::FileTabs>,
    records: State<'_, SessionRecords>,
) -> Result<WindowView, String> {
    let window = calling_window(&window)?;
    if window != WindowId::MAIN {
        return Err("a session window's layout is its session's".into());
    }
    // A page loaded afresh has no Files tab: their connections go (on the
    // runtime, so that a cancelled upload removes its partial file first).
    {
        let _runtime = tauri::async_runtime::handle().inner().enter();
        files.browsers.close_all();
    }
    let view = {
        let mut windows = tabs.lock();
        windows.reset(window).map_err(|error| error.to_string())?;
        window_view(&windows, window)?
    };
    // Its members left the group: the other windows show it.
    group::tell_windows(&app, &tabs, &records);
    Ok(view)
}

/// Edits the calling window's layout, or the app's multi-send group, from
/// any window (#87). A session window's edit may change the session it is
/// named by: every window is told (#85); so is a change to the group. A
/// join of a production session (by the app's record of it, as a group
/// paste sees it) is refused unless it says it was confirmed (#123).
#[tauri::command]
fn layout_edit(
    app: AppHandle,
    window: tauri::WebviewWindow,
    shell: State<'_, Shell>,
    tabs: State<'_, Tabs>,
    records: State<'_, SessionRecords>,
    edit: LayoutEdit,
) -> Result<LayoutChanged, String> {
    let window = calling_window(&window)?;
    let changed = {
        // The workspace first, then the windows (as everywhere else).
        let workspace = shell.workspace();
        let safety = |session| records.paste_safety(session, workspace.as_ref());
        let mut windows = tabs.lock();
        let created = apply_layout_edit(&mut windows, window, edit, safety)
            .map_err(|error| error.to_string())?;
        LayoutChanged {
            layout: window_view(&windows, window)?,
            created,
        }
    };
    if window != WindowId::MAIN {
        session_windows::announce(&app, &tabs, &records);
    }
    group::tell_windows(&app, &tabs, &records);
    Ok(changed)
}

/// What `window`'s interface is sent of its layout.
fn window_view(windows: &Windows, window: WindowId) -> Result<WindowView, String> {
    windows
        .view(window)
        .ok_or_else(|| LayoutError::NoSuchWindow(window).to_string())
}

/// `safety`: a session's paste safety now, for the joins (#123).
fn apply_layout_edit(
    windows: &mut Windows,
    window: WindowId,
    edit: LayoutEdit,
    safety: impl Fn(SessionId) -> PasteSafety,
) -> Result<Option<u64>, LayoutError> {
    match edit {
        LayoutEdit::JoinGroup { tab, confirmed } => {
            return windows
                .join_group_confirmed(window, tab, confirmed, safety)
                .map(|()| None)
        }
        LayoutEdit::JoinPane { pane, confirmed } => {
            return windows
                .join_pane_confirmed(window, pane, confirmed, safety)
                .map(|_| None)
        }
        LayoutEdit::LeaveGroup { tab } => return windows.leave_group(window, tab).map(|()| None),
        LayoutEdit::EndGroup => {
            windows.end_group();
            return Ok(None);
        }
        _ => {}
    }
    let mut layout = windows
        .layout_mut(window)
        .ok_or(LayoutError::NoSuchWindow(window))?;
    Ok(match edit {
        LayoutEdit::OpenTab => Some(layout.open_tab().0),
        LayoutEdit::OpenView { view, subject } => {
            Some(layout.open_view(View::new(view, subject))?.0)
        }
        LayoutEdit::RetargetView { view, from, to } => layout
            .retarget_view(&View::new(view, Some(from)), Some(to))
            .map(|tab| tab.0),
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
        LayoutEdit::JoinGroup { .. }
        | LayoutEdit::JoinPane { .. }
        | LayoutEdit::LeaveGroup { .. }
        | LayoutEdit::EndGroup => unreachable!("group edits are applied above"),
    })
}

/// Forgets the session (closing it if it is open) and its relay: its
/// terminal is gone.
#[tauri::command]
fn session_discard(
    manager: State<'_, SessionManager>,
    relays: State<'_, SessionRelays>,
    records: State<'_, SessionRecords>,
    auto: State<'_, AutoReconnects>,
    id: u64,
) {
    relays.forget(SessionId(id));
    manager.discard(SessionId(id));
    records.forget(SessionId(id));
    auto.forget(SessionId(id));
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

/// Input in the session `id`'s terminal, from `origin` (#123): typed, it
/// goes to every connected member if its tab is in the multi-send group,
/// whichever window it is in, otherwise to `id` only; a group paste's line
/// to `id` while it is a member's; anything else (a paste of its own, the
/// terminal's replies, mouse reports) to `id` only
/// (`Windows::input_targets`).
#[tauri::command]
fn session_input(
    manager: State<'_, SessionManager>,
    tabs: State<'_, Tabs>,
    runs: State<'_, StepRuns>,
    id: u64,
    data: Vec<u8>,
    origin: InputOrigin,
) {
    let targets = tabs.lock().input_targets(SessionId(id), origin);
    // The user's input stops a snippet's steps running on a target, which
    // then does not get it (#23).
    let targets = runs.user_input(targets, origin);
    manager.write_to_each(&targets, &data);
}

/// Input for the session `id` alone: the terminal's own replies to the
/// device and mouse reports, which are never shared.
#[tauri::command]
fn session_write(manager: State<'_, SessionManager>, id: u64, data: Vec<u8>) -> Result<(), String> {
    manager
        .write(SessionId(id), data)
        .map_err(|error| error.to_string())
}

/// The terminal has consumed `bytes` bytes of the session's output, given
/// to it as the consumer `consumer` (the key `session_open`,
/// `session_duplicate` or `session_reconnect` returned). Without these, the
/// core stops reading from the device (flow control). One naming another
/// consumer (a window the session has left, a connection since
/// reconnected) is ignored, so it can never make room.
#[tauri::command]
fn session_acknowledge(relays: State<'_, SessionRelays>, id: u64, consumer: u64, bytes: usize) {
    relays.acknowledge(SessionId(id), ConsumerKey::from_raw(consumer), bytes);
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

/// The port forwards of the session `id` as they are now (#21); none for
/// a session that is not open.
#[tauri::command]
fn session_forwards(manager: State<'_, SessionManager>, id: u64) -> Vec<ForwardStatus> {
    manager.forwards(SessionId(id)).unwrap_or_default()
}

/// One session's forwards, in the list of every open forward.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionForwards {
    id: u64,
    /// The session's tab label and header title, when the app has them.
    name: Option<String>,
    title: Option<String>,
    forwards: Vec<ForwardStatus>,
}

/// Every open session that has port forwards, with them (#21).
#[tauri::command]
fn forwards_list(
    manager: State<'_, SessionManager>,
    records: State<'_, SessionRecords>,
) -> Vec<SessionForwards> {
    manager
        .all_forwards()
        .into_iter()
        .map(|(id, forwards)| {
            let record = records.get(id);
            SessionForwards {
                id: id.0,
                name: record.as_ref().map(|r| r.name.clone()),
                title: record.map(|r| r.title),
                forwards,
            }
        })
        .collect()
}

/// Stops the forward `name` of the session `id`; the session goes on.
#[tauri::command]
async fn forward_stop(
    manager: State<'_, SessionManager>,
    id: u64,
    name: String,
) -> Result<bool, String> {
    manager
        .stop_forward(SessionId(id), &name)
        .await
        .map_err(|error| error.to_string())
}

/// The serial ports this computer has now, with their adapters.
#[tauri::command]
async fn serial_ports(manager: State<'_, SessionManager>) -> Result<Vec<SerialPortInfo>, String> {
    manager.serial_ports().map_err(|error| error.to_string())
}

/// The shells this computer can open as a local session (#22), the first
/// being what an unset preference opens.
#[tauri::command]
fn local_shell_list() -> Vec<enodix_core::session::LocalShell> {
    local_shells()
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
        for window in app.webview_windows().values() {
            // Only the frame's colours; the page already follows the theme.
            let _ = window.set_theme(window_theme(next.theme));
        }
    }
    // Detached windows follow the theme and font (the main window applies
    // its own answer).
    let _ = app.emit(PREFERENCES_CHANGED_EVENT, &next);
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

/// One member of the multi-send group a paste goes to: the text it is to
/// receive and its safety, as the interface shows them.
#[derive(Deserialize)]
struct GroupPasteMember {
    text: String,
    safety: PasteSafety,
}

/// How a paste goes out to the multi-send group's members (#36): one
/// confirmation for all, each member its own lines and delay.
#[tauri::command]
fn paste_plan_group(
    preferred: State<'_, UserPreferences>,
    members: Vec<GroupPasteMember>,
) -> GroupPastePlan {
    let confirm = preferred.lock().confirm_multi_line_paste;
    let members: Vec<(&str, PasteSafety)> = members
        .iter()
        .map(|member| (member.text.as_str(), member.safety))
        .collect();
    plan_group_paste(&members, confirm)
}

/// A tab about to join the multi-send group, with its session's safety.
#[derive(Deserialize)]
struct Joining {
    tab: TabId,
    safety: PasteSafety,
}

/// Which of the tabs about to join the multi-send group the user confirms
/// first: the production ones.
#[tauri::command]
fn group_join_check(joining: Vec<Joining>) -> Vec<TabId> {
    let joining: Vec<(TabId, PasteSafety)> = joining
        .into_iter()
        .map(|joining| (joining.tab, joining.safety))
        .collect();
    confirm_joining(&joining)
}

/// The update check (`enodix_core::update`) and, in a release build, the
/// Tauri updater that installs what the user agreed to.
struct Updates {
    checker: UpdateChecker,
    /// The updater plugin is registered (only in builds made by the release
    /// script, which gives it the release key and the server's address).
    updater_plugin: bool,
}

/// How this copy was deployed and whether it can check at all.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBuild {
    version: String,
    kind: InstallKind,
    /// The build knows a release server.
    configured: bool,
}

#[tauri::command]
fn update_build(updates: State<'_, Updates>) -> UpdateBuild {
    UpdateBuild {
        version: updates.checker.current_version(),
        kind: updates.checker.kind(),
        configured: updates.checker.source().is_some(),
    }
}

/// The check the app makes on its own, once at start: nothing (and no
/// connection) when the user turned it off.
#[tauri::command]
async fn update_check_automatic(
    preferred: State<'_, UserPreferences>,
    updates: State<'_, Updates>,
) -> Result<UpdateStatus, String> {
    let preferences = preferred.lock().clone();
    Ok(updates.checker.automatic(&preferences).await)
}

/// A check the user asked for.
#[tauri::command]
async fn update_check_now(updates: State<'_, Updates>) -> Result<UpdateStatus, String> {
    Ok(updates.checker.check_now().await)
}

/// Installs the release last offered, after the user agreed, if the core
/// allows it (an installed copy with the updater), and only the version
/// offered. The updater asks the same release server, checks the
/// download's signature against the release key, starts the installer and
/// ends the app.
#[tauri::command]
async fn update_install(app: AppHandle, updates: State<'_, Updates>) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let offer = updates
        .checker
        .installable(updates.updater_plugin)
        .map_err(|refused| refused.to_string())?;
    let source = updates
        .checker
        .source()
        .ok_or("this build has no release server")?;
    let endpoint = source.url().parse().map_err(|error| format!("{error}"))?;
    let update = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .and_then(|builder| builder.build())
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| format!("the update could not be fetched: {error}"))?
        .ok_or("the release server no longer offers a newer version")?;
    if !offer.is_version(&update.version) {
        return Err(format!(
            "the release server now offers {} instead of {}; check again",
            update.version, offer.version
        ));
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|error| format!("the update could not be installed: {error}"))?;
    // On Windows the installer has taken over and the app has ended.
    app.restart();
}

/// Opens the offered release's web page in the default browser.
#[tauri::command]
fn update_open_page(app: AppHandle, updates: State<'_, Updates>) -> Result<String, String> {
    let page = updates
        .checker
        .offer()
        .and_then(|offer| offer.page)
        .ok_or("the release has no web page")?;
    open_in_browser(&app, &page)?;
    Ok(page)
}

#[tauri::command]
fn session_close(manager: State<'_, SessionManager>, id: u64) -> Result<(), String> {
    manager
        .close(SessionId(id))
        .map_err(|error| error.to_string())
}

/// The open workspace as it is, without opening it again (a detached
/// window's first look); `None` with none open.
#[tauri::command]
fn workspace_current(shell: State<'_, Shell>) -> Option<WorkspaceView> {
    shell.workspace().as_ref().map(WorkspaceView::from)
}

/// Shows the run `run` in a window of its own (#77): the existing one if
/// it is open, else a new one. The page reads its run from its label.
/// Async: building a window in a synchronous command deadlocks on Windows.
#[tauri::command]
async fn window_detach_run(
    app: AppHandle,
    preferred: State<'_, UserPreferences>,
    run: String,
) -> Result<(), String> {
    // Run ids are `YYYY-MM-DD_HH-MM-SS[_n]`: valid in a window label.
    if run.is_empty()
        || !run
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{run:?} is not a run"));
    }
    let label = format!("{RUN_WINDOW_PREFIX}{run}");
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        return window.set_focus().map_err(|error| error.to_string());
    }
    let theme = window_theme(preferred.lock().theme);
    open_detached(
        &app,
        label,
        format!("{} - run {run}", product_name(&app)),
        theme,
    )
}

/// Opens a detached window: no system frame, the page draws its own
/// (#80), with the shadow that keeps its edge and Windows 11's corners.
fn open_detached(
    app: &AppHandle,
    label: String,
    title: String,
    theme: Option<tauri::Theme>,
) -> Result<(), String> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::default())
        .additional_browser_args(WEBVIEW_ARGUMENTS)
        .use_https_scheme(HTTPS_SCHEME)
        .general_autofill_enabled(false)
        .title(title)
        .inner_size(900.0, 640.0)
        .min_inner_size(420.0, 320.0)
        .theme(theme)
        .decorations(false)
        .shadow(true)
        .build()
        .map(drop)
        .map_err(|error| format!("the window could not be opened: {error}"))
}

/// The label of the window showing the host `host`'s config history.
fn config_window_label(host: &str) -> String {
    let hex: String = host.bytes().map(|byte| format!("{byte:02x}")).collect();
    format!("{CONFIG_WINDOW_PREFIX}{hex}")
}

/// The host whose config history the window labelled `label` shows.
fn config_window_host(label: &str) -> Option<String> {
    let hex = label.strip_prefix(CONFIG_WINDOW_PREFIX)?;
    if hex.is_empty() || hex.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

/// Shows the host `host`'s config history in a window of its own (#78):
/// the existing one if it is open, else a new one. Async, as for runs.
#[tauri::command]
async fn window_detach_configs(
    app: AppHandle,
    preferred: State<'_, UserPreferences>,
    host: String,
) -> Result<(), String> {
    if host.is_empty() {
        return Err("no host to show".into());
    }
    let label = config_window_label(&host);
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        return window.set_focus().map_err(|error| error.to_string());
    }
    let theme = window_theme(preferred.lock().theme);
    open_detached(
        &app,
        label,
        format!("{} - config history {host}", product_name(&app)),
        theme,
    )
}

/// The main window, brought forward.
fn focused_main(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    let main = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or("the main window is gone")?;
    let _ = main.unminimize();
    let _ = main.set_focus();
    Ok(main)
}

/// Docks the calling detached window back: the main window shows its run
/// (`dock-run`) or config history (`dock-configs`) as a tab and takes the
/// focus, and the window closes.
#[tauri::command]
async fn window_dock(app: AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    let label = window.label();
    let (event, subject) = if let Some(run) = label.strip_prefix(RUN_WINDOW_PREFIX) {
        (DOCK_RUN_EVENT, run.to_string())
    } else if let Some(host) = config_window_host(label) {
        (DOCK_CONFIGS_EVENT, host)
    } else {
        return Err("only a detached window docks back".into());
    };
    let main = focused_main(&app)?;
    main.emit_to(MAIN_WINDOW, event, subject)
        .map_err(|error| error.to_string())?;
    window.destroy().map_err(|error| error.to_string())
}

/// From a detached config history: the main window opens a backup of its
/// host (`back-up-host`, a job tab at pre-flight) and takes the focus.
#[tauri::command]
async fn window_back_up(app: AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    let host = config_window_host(window.label()).ok_or("not a config history window")?;
    focused_main(&app)?
        .emit_to(MAIN_WINDOW, BACK_UP_HOST_EVENT, host)
        .map_err(|error| error.to_string())
}

/// Closes every detached window (their jobs go on).
fn close_detached(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if label != MAIN_WINDOW {
            let _ = window.destroy();
        }
    }
}

/// Closes every run and config history window: the main window switched
/// workspace, and what they show belongs to the other one. Session windows
/// stay (#84): their sessions stay open across a switch, as tabs do.
#[tauri::command]
fn windows_close_detached(app: AppHandle) {
    for (label, window) in app.webview_windows() {
        if label != MAIN_WINDOW && session_windows::window_of(&label).is_none() {
            let _ = window.destroy();
        }
    }
}

pub fn run() {
    let context = tauri::generate_context!();
    // Only a release build (`scripts/release.mjs`) configures the updater:
    // its public key and the release server's address. Only Windows
    // installs updates (`InstallKind::detect`): elsewhere the address is
    // used for the check alone, and the plugin is left out.
    let updater_config = context.config().plugins.0.get("updater").cloned();
    let updater_plugin = cfg!(windows) && updater_config.is_some();
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());
    if updater_plugin {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    builder
        .setup(move |app| {
            let (data_folder, local_folder, log_folder) = match std::env::var_os(DATA_DIR_VARIABLE)
            {
                Some(folder) => {
                    let folder = PathBuf::from(folder);
                    let logs = folder.join("logs");
                    (folder.clone(), folder, logs)
                }
                // Logs, config backups, job results and the Python
                // environment stay on this computer (#128): on Windows
                // the local app data, not the roaming one. Elsewhere
                // the app has one data folder.
                None => (
                    app.path().app_data_dir()?,
                    if cfg!(windows) {
                        app.path().app_local_data_dir()?
                    } else {
                        app.path().app_data_dir()?
                    },
                    app.path().app_log_dir()?,
                ),
            };
            // Before anything reads them: what an older version kept in
            // the roaming folder moves to the local one, once (#128).
            let settled = data_folders::settle(&data_folder, &local_folder);
            for what in &settled.moved {
                eprintln!("moved {what} to {}", local_folder.display());
            }
            for notice in &settled.notices {
                eprintln!("{}", notice.message);
            }
            let folders = settled.folders.clone();
            // The vault, if its file is in the data folder, replaces the
            // keychain (layered the same way over the smoke's memory store).
            // Without a keychain (Linux; the smoke's `none`) it is the only
            // store.
            let vault = vault_file(&data_folder);
            let credentials = Arc::new(match std::env::var(CREDENTIAL_STORE_VARIABLE).as_deref() {
                Ok("memory") => ActiveStore::new(Box::new(MemoryStore::new()), vault),
                Ok("none") => ActiveStore::without_keychain(vault),
                // Entries are named after the app's identifier, which
                // stays put if the product is renamed.
                _ if KeychainStore::is_supported() => ActiveStore::new(
                    Box::new(KeychainStore::new(app.config().identifier.clone())),
                    vault,
                ),
                _ => ActiveStore::without_keychain(vault),
            });
            // Scripts get credentials from the running app over a pipe only
            // this user can open (the headless binary asks there first).
            // Without it (another instance serves already) they read the
            // store themselves.
            let agent = AgentServer::start(
                AgentEndpoint::for_data_folder(&data_folder),
                credentials.clone(),
            )
            .inspect_err(|error| eprintln!("scripts cannot ask for credentials: {error}"))
            .ok();
            app.manage(CredentialAgent(agent));
            // Trusted host keys are per user, beside the app state.
            let known_hosts = KnownHosts::in_data_folder(&data_folder);
            app.manage(SessionManager::new(known_hosts.clone()));
            // The Files tab's connections (#103) check the same keys.
            app.manage(files::FileTabs {
                browsers: enodix_core::files::FileBrowsers::new(),
                known_hosts,
            });
            app.manage(Tabs::new());
            app.manage(SessionRelays::new());
            app.manage(SessionRecords::new());
            app.manage(AutoReconnects::new());
            app.manage(StepRuns::new());
            app.manage(CsvFile(Mutex::new(None)));
            app.manage(NornirFiles(Mutex::new(None)));
            app.manage(SshConfigFiles(Mutex::new(None)));
            app.manage(JobPlans::default());
            app.manage(EnrichPreviews(Mutex::new(None)));
            // Job runs are per user, in the local data folder, keyed by
            // workspace id.
            app.manage(Runs(RunStore::new(folders.jobs.clone())));
            // Config backups likewise.
            app.manage(Configs(ConfigStore::new(folders.configs.clone())));
            // Holds the running copy's lock on the data folders for the
            // app's life.
            app.manage(DataMove(settled));
            app.manage(RunningJobs::new());
            app.manage(Arc::new(Live::default()));
            let preferences = Preferences::load(&data_folder);
            let theme = window_theme(preferences.theme);
            app.manage(UserPreferences(Mutex::new(preferences)));
            let program_folder = std::env::current_exe()?
                .parent()
                .map(PathBuf::from)
                .unwrap_or_default();
            let override_address = std::env::var(UPDATE_ENDPOINT_VARIABLE).ok();
            let built_in = updater_config
                .as_ref()
                .and_then(|config| config.get("endpoints")?.get(0)?.as_str());
            // Where the bundle's resources are (the Python wheel; a Linux
            // package's marker): beside the program on Windows and in a
            // build folder, the package's `/usr/lib/<product>` on Linux.
            let resource_folder = app
                .path()
                .resource_dir()
                .unwrap_or_else(|_| program_folder.clone());
            let checker = UpdateChecker::new(
                UpdateSource::choose(override_address.as_deref(), built_in),
                &app.package_info().version.to_string(),
                InstallKind::detect(&program_folder, &resource_folder),
            )?;
            app.manage(python::PythonSetup::new(
                folders.clone(),
                program_folder.clone(),
                resource_folder,
                app.package_info().version.to_string(),
            ));
            app.manage(Updates {
                checker,
                updater_plugin,
            });
            app.manage(Shell {
                data_folder,
                app_state: Mutex::new(()),
                logs: LogFolder::new(log_folder),
                credentials,
                workspace: Mutex::new(None),
                watcher: Mutex::new(None),
            });
            let title = product_name(app.handle());
            let main = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
                .additional_browser_args(WEBVIEW_ARGUMENTS)
                .use_https_scheme(HTTPS_SCHEME)
                .general_autofill_enabled(false)
                .title(title)
                .inner_size(1100.0, 720.0)
                .min_inner_size(480.0, 320.0)
                .theme(theme)
                // The app draws its own frame (#80): its top bar drags the
                // window and holds the window's buttons. The shadow keeps
                // the frame's edge, and on Windows 11 its rounded corners.
                .decorations(false)
                .shadow(true)
                .build()?;
            // The engine ending as a whole ends the app (#120); the session
            // windows' pages are asked whether they answer.
            session_windows::watch_main(&main);
            app.manage(session_windows::Answers::default());
            session_windows::ask_pages(app.handle().clone());
            Ok(())
        })
        // Closing the main window closes the detached ones (and so the app).
        // A session window gone without docking back hands its sessions to
        // the main window (#84).
        .on_window_event(|window, event| {
            if !matches!(event, tauri::WindowEvent::Destroyed) {
                return;
            }
            if window.label() == MAIN_WINDOW {
                // The Files tabs' connections go with their window.
                window.state::<files::FileTabs>().browsers.close_all();
                close_detached(window.app_handle());
            } else {
                session_windows::destroyed(window.app_handle(), window.label());
            }
        })
        .invoke_handler(tauri::generate_handler![
            files::files_open,
            files::files_answer,
            files::files_decide_host_key,
            files::files_cancel,
            files::files_close,
            files::files_close_host,
            files::files_upload,
            files::files_download,
            files::files_send,
            files::files_fetch,
            files::files_present,
            files::files_existing_locally,
            files::files_cancel_transfer,
            files::files_list,
            files::files_rename,
            files::files_delete,
            files::files_make_folder,
            data_notices,
            enrich_preview,
            workspace_current,
            window_detach_run,
            window_detach_configs,
            window_back_up,
            window_dock,
            windows_close_detached,
            session_windows::session_record,
            session_windows::session_release,
            session_windows::window_open_session,
            session_windows::session_window_take,
            session_windows::session_attach,
            session_windows::session_dock,
            session_windows::session_window_close,
            session_windows::session_window_answering,
            session_windows::session_window_take_back,
            session_windows::session_window_take_tab,
            session_windows::session_dock_all,
            session_windows::session_windows_list,
            session_windows::snippet_target,
            session_windows::terminal_used,
            session_windows::snippet_send_to,
            session_windows::main_window_request,
            session_windows::main_window_connect,
            session_windows::main_window_close,
            session_windows::session_windows_live,
            session_windows::session_windows_close_workspace,
            session_snippet_expand,
            jobs_live,
            jobs_forget_finished,
            enrich_apply,
            app_name,
            session_auto_reconnect_enable,
            session_auto_reconnect_stop,
            session_auto_reconnect_restart,
            app_quit,
            vault_status,
            vault_keychain,
            vault_unlock,
            vault_turn_on,
            vault_change_password,
            vault_turn_off,
            vault_start_over,
            workspace_open,
            workspace_folder_kind,
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
            platform_save,
            platform_delete,
            snippet_find,
            snippet_save,
            snippet_delete,
            snippet_expand,
            snippet_expand_group,
            job_plan,
            job_plan_forget,
            workspace_scripts,
            job_start,
            job_cancel,
            job_run_again,
            saved_job_save,
            saved_job_delete,
            saved_job_secret_answers,
            saved_job_plan,
            job_runs,
            job_run,
            job_run_output,
            job_run_search,
            job_compare,
            job_run_export,
            job_run_delete,
            host_create,
            host_rename,
            host_move,
            host_delete,
            host_save,
            csv_import_read,
            csv_import_preview,
            csv_import_run,
            nornir_import_preview,
            nornir_import_run,
            ssh_config_default_path,
            ssh_config_import_preview,
            ssh_config_import_run,
            profiles_list,
            profile_save,
            profile_delete,
            secret_save,
            secret_delete,
            host_login,
            session_open,
            session_write,
            session_input,
            steps::steps_preview,
            steps::steps_start,
            steps::steps_stop,
            session_answer,
            session_decide_host_key,
            session_acknowledge,
            session_resize,
            session_close,
            session_break,
            session_forwards,
            forwards_list,
            forward_stop,
            serial_ports,
            local_shell_list,
            session_reconnect,
            session_duplicate,
            session_discard,
            logs_open_folder,
            config_versions,
            config_backed_up,
            config_version,
            config_diff,
            config_export,
            host_web_links,
            web_link_open,
            layout_reset,
            layout_edit,
            preferences_get,
            preferences_save,
            preferences_zoom,
            paste_plan,
            paste_plan_group,
            group_join_check,
            group::group_members,
            group::group_paste_send,
            group::group_paste_check,
            group::session_safety,
            update_build,
            update_check_automatic,
            update_check_now,
            update_install,
            update_open_page,
            python::python_status,
            python::python_offer,
            python::python_plan,
            python::python_setup,
            python::python_setup_cancel,
            python::python_use_interpreter,
            python::python_turn_off
        ])
        .build(context)
        .expect("failed to start the app")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                end_with_the_app(app);
            }
        });
}

/// The app is closing (#124): every running job is cancelled as Cancel
/// does and a Python setup under way is stopped, waiting a bounded time
/// for them to end and their runs to be kept. (A crash or a kill runs
/// nothing: a script's processes end with the app all the same, see
/// `enodix_core::job`.)
fn end_with_the_app(app: &AppHandle) {
    let until = std::time::Instant::now() + job::APP_CLOSE_WAIT;
    let setup = app.state::<Arc<python::PythonSetup>>().cancel_running();
    let left = app
        .state::<RunningJobs>()
        .end_all(job::APP_CLOSED, job::APP_CLOSE_WAIT);
    if left > 0 {
        eprintln!("{left} job(s) had not ended when the app closed");
    }
    if let Some(setup) = setup {
        if !setup.wait(until.saturating_duration_since(std::time::Instant::now())) {
            eprintln!("the Python setup had not stopped when the app closed");
        }
    }
}
