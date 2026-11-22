//! The Files tab's commands (#103, #106): thin over the core's
//! `enodix_core::files::FileBrowsers`. A browser's events reach the
//! interface on a `Channel`; it belongs to the app, is closed when its tab
//! closes (`files_close_host`, with the host's transfers), and all are
//! closed with the main window. Each call adding transfers gives a
//! `Channel` of their states.

use std::path::PathBuf;
use std::time::Duration;

use enodix_core::files::{
    existing_locally, follow, Added, BrowserEvent, BrowserId, EntryKind, FileBrowsers, Listing,
    TransferId, TransferState, TransferWatch,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::HostKeyDecision;
use enodix_core::workspace::HostId;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use super::{calling_window, with_open_profiles, Prompted, Shell};
use enodix_core::layout::WindowId;

/// The open browsers, and the trusted host keys they check against (the
/// sessions' own).
pub(super) struct FileTabs {
    pub(super) browsers: FileBrowsers,
    pub(super) known_hosts: KnownHosts,
}

/// Opens a browser on the open workspace's host `host`: with its stored
/// profile, or with `credentials` the user typed. It starts in `folder`
/// (a reconnect's), else in the host's platform file folder, else in the
/// login's folder. Its events arrive on `on_event`. Main window only.
#[tauri::command]
pub(super) async fn files_open(
    window: tauri::WebviewWindow,
    shell: State<'_, Shell>,
    files: State<'_, FileTabs>,
    host: HostId,
    credentials: Option<Prompted>,
    folder: Option<String>,
    on_event: Channel<BrowserEvent>,
) -> Result<BrowserId, String> {
    if calling_window(&window)? != WindowId::MAIN {
        return Err("the Files tab opens in the main window".into());
    }
    let target = with_open_profiles(&shell, |profiles| match credentials {
        Some(Prompted { username, password }) => {
            profiles.prompted_target(&host, username, password)
        }
        None => profiles.profile_target(&host),
    })?;
    let (start, platform) = {
        let workspace = shell.workspace();
        let workspace = workspace.as_ref();
        let start = folder.or_else(|| {
            workspace.and_then(|workspace| workspace.host_file_folder(&host).map(str::to_owned))
        });
        let platform = workspace.and_then(|workspace| workspace.host_platform(&host).cloned());
        (start, platform)
    };
    let (id, mut events) = files.browsers.open(
        host.as_str(),
        target,
        files.known_hosts.clone(),
        start,
        platform.as_ref(),
    );
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });
    Ok(id)
}

#[tauri::command]
pub(super) fn files_answer(files: State<'_, FileTabs>, id: BrowserId, answers: Vec<String>) {
    files.browsers.answer(id, answers);
}

#[tauri::command]
pub(super) fn files_decide_host_key(
    files: State<'_, FileTabs>,
    id: BrowserId,
    decision: HostKeyDecision,
) {
    files.browsers.decide_host_key(id, decision);
}

#[tauri::command]
pub(super) fn files_cancel(files: State<'_, FileTabs>, id: BrowserId) {
    files.browsers.cancel(id);
}

#[tauri::command]
pub(super) fn files_close(files: State<'_, FileTabs>, id: BrowserId) {
    files.browsers.close(id);
}

/// The host's Files tab closed: its browsers close, its transfers are
/// cancelled.
/// (Async: the core lets a cancelled upload remove its partial file over
/// the closing connection, on the runtime.)
#[tauri::command]
pub(super) async fn files_close_host(
    files: State<'_, FileTabs>,
    host: HostId,
) -> Result<(), String> {
    files.browsers.close_host(host.as_str());
    Ok(())
}

/// A transfer's new state, on the channel of the call that added it.
#[derive(Clone, Serialize)]
pub(super) struct TransferUpdate {
    id: TransferId,
    state: TransferState,
}

/// How often a transfer's progress reaches the interface, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// Sends each transfer's states on `on_update` until it ends. The commands
/// adding transfers are async: the engine starts them on the runtime.
fn forward(added: Vec<(Added, TransferWatch)>, on_update: Channel<TransferUpdate>) -> Vec<Added> {
    added
        .into_iter()
        .map(|(added, watch)| {
            let id = added.id;
            let channel = on_update.clone();
            tauri::async_runtime::spawn(follow(watch, PROGRESS_EVERY, move |state| {
                channel.send(TransferUpdate { id, state }).is_ok()
            }));
            added
        })
        .collect()
}

/// Uploads the local files `paths` into `folder` on the device.
#[tauri::command]
pub(super) async fn files_upload(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    paths: Vec<PathBuf>,
    on_update: Channel<TransferUpdate>,
) -> Result<Vec<Added>, String> {
    let added = files
        .browsers
        .upload(id, &folder, paths)
        .map_err(|error| error.to_string())?;
    Ok(forward(added, on_update))
}

/// Downloads the files `names` in `folder` into the local folder `into`.
#[tauri::command]
pub(super) async fn files_download(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    names: Vec<String>,
    into: PathBuf,
    on_update: Channel<TransferUpdate>,
) -> Result<Vec<Added>, String> {
    let added = files
        .browsers
        .download(id, &folder, names, &into)
        .map_err(|error| error.to_string())?;
    Ok(forward(added, on_update))
}

/// Sends the local file `local` to the device path `remote`.
#[tauri::command]
pub(super) async fn files_send(
    files: State<'_, FileTabs>,
    id: BrowserId,
    local: PathBuf,
    remote: String,
    on_update: Channel<TransferUpdate>,
) -> Result<Added, String> {
    let added = files
        .browsers
        .send(id, local, &remote)
        .map_err(|error| error.to_string())?;
    Ok(forward(vec![added], on_update).remove(0))
}

/// Fetches the device path `remote` into the local folder `into`.
#[tauri::command]
pub(super) async fn files_fetch(
    files: State<'_, FileTabs>,
    id: BrowserId,
    remote: String,
    into: PathBuf,
    on_update: Channel<TransferUpdate>,
) -> Result<Added, String> {
    let added = files
        .browsers
        .fetch(id, &remote, &into)
        .map_err(|error| error.to_string())?;
    Ok(forward(vec![added], on_update).remove(0))
}

/// Which of `names` are already in `folder` on the device (none over SCP).
#[tauri::command]
pub(super) async fn files_present(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    names: Vec<String>,
) -> Result<Vec<String>, String> {
    files
        .browsers
        .present(id, &folder, names)
        .await
        .map_err(|error| error.to_string())
}

/// Which of `names` are already in the local folder `folder`.
#[tauri::command]
pub(super) fn files_existing_locally(folder: PathBuf, names: Vec<String>) -> Vec<String> {
    existing_locally(&folder, &names)
}

#[tauri::command]
pub(super) fn files_cancel_transfer(files: State<'_, FileTabs>, transfer: TransferId) -> bool {
    files.browsers.cancel_transfer(transfer)
}

/// `path` as typed in the path box while `folder` was shown.
#[tauri::command]
pub(super) async fn files_list(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    path: String,
) -> Result<Listing, String> {
    files
        .browsers
        .list(id, &folder, &path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) async fn files_rename(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    from: String,
    to: String,
) -> Result<Listing, String> {
    files
        .browsers
        .rename(id, &folder, &from, &to)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) async fn files_delete(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    name: String,
    kind: EntryKind,
) -> Result<Listing, String> {
    files
        .browsers
        .delete(id, &folder, &name, kind)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(super) async fn files_make_folder(
    files: State<'_, FileTabs>,
    id: BrowserId,
    folder: String,
    name: String,
) -> Result<Listing, String> {
    files
        .browsers
        .make_folder(id, &folder, &name)
        .await
        .map_err(|error| error.to_string())
}
