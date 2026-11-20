//! The Files tab's commands (#103): thin over the core's
//! `netterm_core::files::FileBrowsers`. A browser's events reach the
//! interface on a `Channel`; it belongs to the app, is closed when its tab
//! closes (`files_close`), and all are closed with the main window.

use netterm_core::files::{BrowserEvent, BrowserId, EntryKind, FileBrowsers, Listing};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::HostKeyDecision;
use netterm_core::workspace::HostId;
use tauri::ipc::Channel;
use tauri::State;

use super::{calling_window, with_open_profiles, Prompted, Shell};
use netterm_core::layout::WindowId;

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
    let start = folder.or_else(|| {
        shell
            .workspace()
            .as_ref()
            .and_then(|workspace| workspace.host_file_folder(&host).map(str::to_owned))
    });
    let (id, mut events) = files
        .browsers
        .open(target, files.known_hosts.clone(), start);
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
