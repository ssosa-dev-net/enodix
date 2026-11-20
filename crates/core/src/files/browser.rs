//! Browsing a device's files, as the Files tab does (#103): a file
//! connection opened attended, SFTP on it, the listing of one folder at a
//! time, and the few changes made on the device (rename, delete, new
//! folder). [`FileBrowsers`] holds every open browser by id, so the shell
//! only passes commands and events through.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tokio::sync::mpsc;

use super::sftp::{EntryKind, FileError, RemoteEntry, Sftp, StartError};
use crate::known_hosts::KnownHosts;
use crate::session::connection::{FileConnection, OpeningEvent};
use crate::session::{HostKeyDecision, HostKeyQuestion, LoginPrompt, SshTarget};

/// Which browser a command is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct BrowserId(pub u64);

/// One folder's contents: folders first, then files, each by name (hidden
/// ones included).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    /// The folder, as the device writes it (absolute when it says so).
    pub folder: String,
    /// The folder above it; `None` at the top.
    pub parent: Option<String>,
    pub entries: Vec<RemoteEntry>,
}

/// What a browser reports, in order: its questions while it logs in, then
/// one of `Connected`, `NoSftp` or `Failed`; after `Connected`, `Lost` when
/// the connection ends (not when it is closed).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BrowserEvent {
    /// An untrusted host key: decide with [`FileBrowsers::decide_host_key`].
    HostKey { question: HostKeyQuestion },
    /// A login question: [`FileBrowsers::answer`] or [`FileBrowsers::cancel`].
    Prompt { prompt: LoginPrompt },
    /// Logged in, SFTP started, the starting folder listed. `notice` says
    /// why it is not the folder asked for, when it is not.
    Connected {
        listing: Listing,
        notice: Option<String>,
    },
    /// Logged in, but the device offers no SFTP (to this account).
    NoSftp { reason: String },
    /// It could not be opened (cancelled included): why.
    Failed { reason: String },
    /// The connection ended: why.
    Lost { reason: String },
}

/// Why a browser command failed, in the app's words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowseError {
    /// The browser is not connected (still logging in, lost, or closed).
    #[error("not connected")]
    NotConnected,
    #[error("{reason}")]
    BadName { reason: String },
    #[error("{0}")]
    File(#[from] FileError),
}

enum Command {
    Answer(Vec<String>),
    Decide(HostKeyDecision),
    Cancel,
}

/// A connected browser's connection and SFTP on it. SFTP goes first.
struct Browser {
    sftp: Sftp,
    connection: FileConnection,
}

struct Slot {
    commands: mpsc::UnboundedSender<Command>,
    task: tokio::task::JoinHandle<()>,
    open: Arc<Mutex<Option<Arc<Browser>>>>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The open browsers, by id. Closing one (or dropping this) closes its
/// connection.
#[derive(Default)]
pub struct FileBrowsers {
    slots: Mutex<HashMap<BrowserId, Slot>>,
    next: AtomicU64,
}

impl FileBrowsers {
    pub fn new() -> Self {
        Self::default()
    }

    fn slots(&self) -> MutexGuard<'_, HashMap<BrowserId, Slot>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens a browser on `target` (build it with
    /// `Profiles::profile_target` / `prompted_target`), checking host keys
    /// against `known_hosts`. It starts in `start` (the platform's file
    /// folder, or where a reconnect was) or, without one or when that
    /// cannot be listed, in the login's folder. Returns at once; follow
    /// the events. Must be called from within a Tokio runtime.
    pub fn open(
        &self,
        target: SshTarget,
        known_hosts: KnownHosts,
        start: Option<String>,
    ) -> (BrowserId, mpsc::UnboundedReceiver<BrowserEvent>) {
        let id = BrowserId(self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let (events_tx, events) = mpsc::unbounded_channel();
        let (commands_tx, commands) = mpsc::unbounded_channel();
        let open = Arc::new(Mutex::new(None));
        let task = tokio::spawn(run(
            target,
            known_hosts,
            start,
            events_tx,
            commands,
            open.clone(),
        ));
        self.slots().insert(
            id,
            Slot {
                commands: commands_tx,
                task,
                open,
            },
        );
        (id, events)
    }

    fn command(&self, id: BrowserId, command: Command) {
        if let Some(slot) = self.slots().get(&id) {
            let _ = slot.commands.send(command);
        }
    }

    /// Answers the last login question, in its order.
    pub fn answer(&self, id: BrowserId, answers: Vec<String>) {
        self.command(id, Command::Answer(answers));
    }

    /// Decides the last host-key question.
    pub fn decide_host_key(&self, id: BrowserId, decision: HostKeyDecision) {
        self.command(id, Command::Decide(decision));
    }

    /// Gives up a login still going on: it ends `Failed`, cancelled.
    pub fn cancel(&self, id: BrowserId) {
        self.command(id, Command::Cancel);
    }

    /// Closes the browser and its connection; nothing more is reported.
    pub fn close(&self, id: BrowserId) {
        let slot = self.slots().remove(&id);
        drop(slot);
    }

    /// Closes every browser (the window that showed them is gone).
    pub fn close_all(&self) {
        let slots: Vec<Slot> = self.slots().drain().map(|(_, slot)| slot).collect();
        drop(slots);
    }

    fn browser(&self, id: BrowserId) -> Result<Arc<Browser>, BrowseError> {
        let slots = self.slots();
        let slot = slots.get(&id).ok_or(BrowseError::NotConnected)?;
        let open = slot.open.lock().unwrap_or_else(|e| e.into_inner());
        open.clone().ok_or(BrowseError::NotConnected)
    }

    /// Lists `path`, typed in the path box while `folder` was shown: a
    /// relative path is taken from `folder`, an empty one is `folder`
    /// (Refresh).
    pub async fn list(
        &self,
        id: BrowserId,
        folder: &str,
        path: &str,
    ) -> Result<Listing, BrowseError> {
        let browser = self.browser(id)?;
        Ok(listing(&browser.sftp, &resolve_path(folder, path)).await?)
    }

    /// Renames `from` in `folder` to `to` (never replacing anything), then
    /// lists `folder` again.
    pub async fn rename(
        &self,
        id: BrowserId,
        folder: &str,
        from: &str,
        to: &str,
    ) -> Result<Listing, BrowseError> {
        let to = checked_name(to)?;
        let browser = self.browser(id)?;
        browser
            .sftp
            .rename(&join_path(folder, from), &join_path(folder, to))
            .await?;
        Ok(listing(&browser.sftp, folder).await?)
    }

    /// Deletes the file (or link) or the empty folder `name` in `folder`,
    /// then lists `folder` again.
    pub async fn delete(
        &self,
        id: BrowserId,
        folder: &str,
        name: &str,
        kind: EntryKind,
    ) -> Result<Listing, BrowseError> {
        let browser = self.browser(id)?;
        let path = join_path(folder, name);
        match kind {
            EntryKind::Folder => browser.sftp.delete_folder(&path).await?,
            _ => browser.sftp.delete_file(&path).await?,
        }
        Ok(listing(&browser.sftp, folder).await?)
    }

    /// Makes the folder `name` in `folder`, then lists `folder` again.
    pub async fn make_folder(
        &self,
        id: BrowserId,
        folder: &str,
        name: &str,
    ) -> Result<Listing, BrowseError> {
        let name = checked_name(name)?;
        let browser = self.browser(id)?;
        browser.sftp.make_folder(&join_path(folder, name)).await?;
        Ok(listing(&browser.sftp, folder).await?)
    }
}

/// A browser's life: log in (passing questions out and answers in), start
/// SFTP, list the starting folder, then wait for the connection to end.
async fn run(
    target: SshTarget,
    known_hosts: KnownHosts,
    start: Option<String>,
    events: mpsc::UnboundedSender<BrowserEvent>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    open: Arc<Mutex<Option<Arc<Browser>>>>,
) {
    let send = |event| {
        let _ = events.send(event);
    };
    let mut opening = FileConnection::open(target, known_hosts);
    let connection = loop {
        tokio::select! {
            event = opening.next() => match event {
                Some(OpeningEvent::HostKey(question)) => send(BrowserEvent::HostKey { question }),
                Some(OpeningEvent::Prompt(prompt)) => send(BrowserEvent::Prompt { prompt }),
                Some(OpeningEvent::Opened(connection)) => break connection,
                Some(OpeningEvent::Failed(error)) => {
                    return send(BrowserEvent::Failed { reason: error.to_string() });
                }
                None => {
                    return send(BrowserEvent::Failed {
                        reason: "the connection ended while it was being opened".into(),
                    });
                }
            },
            command = commands.recv() => match command {
                Some(Command::Answer(answers)) => opening.answer(answers),
                Some(Command::Decide(decision)) => opening.decide_host_key(decision),
                Some(Command::Cancel) | None => {
                    drop(opening);
                    return send(BrowserEvent::Failed { reason: "the login was cancelled".into() });
                }
            },
        }
    };
    drop(opening);
    let sftp = match connection.start_sftp().await {
        Ok(sftp) => sftp,
        Err(StartError::NoSftp(why)) => {
            return send(BrowserEvent::NoSftp {
                reason: StartError::NoSftp(why).to_string(),
            })
        }
        Err(error) => {
            return send(BrowserEvent::Failed {
                reason: error.to_string(),
            })
        }
    };
    let mut notice = None;
    let first = match &start {
        Some(folder) => match listing(&sftp, folder).await {
            Ok(listing) => Ok(listing),
            Err(error) => {
                notice = Some(format!(
                    "{folder} could not be opened ({error}); this is the login's folder"
                ));
                login_folder(&sftp).await
            }
        },
        None => login_folder(&sftp).await,
    };
    let listing = match first {
        Ok(listing) => listing,
        Err(error) => {
            return send(BrowserEvent::Failed {
                reason: error.to_string(),
            })
        }
    };
    let browser = Arc::new(Browser { sftp, connection });
    *open.lock().unwrap_or_else(|e| e.into_inner()) = Some(browser.clone());
    send(BrowserEvent::Connected { listing, notice });
    let lost = browser.connection.lost().await;
    open.lock().unwrap_or_else(|e| e.into_inner()).take();
    drop(browser);
    send(BrowserEvent::Lost {
        reason: lost.to_string(),
    });
}

async fn login_folder(sftp: &Sftp) -> Result<Listing, FileError> {
    let home = sftp.home().await?;
    listing(sftp, &home).await
}

/// `path` listed, sorted, under the name the device gives it (its
/// canonical path, when it tells one that lists).
async fn listing(sftp: &Sftp, path: &str) -> Result<Listing, FileError> {
    let canonical = sftp.canonical(path).await.ok();
    let (folder, mut entries) = match canonical {
        Some(canonical) => match sftp.list(&canonical).await {
            Ok(entries) => (canonical, entries),
            Err(error) => match sftp.list(path).await {
                Ok(entries) => (path.to_owned(), entries),
                Err(_) => return Err(error),
            },
        },
        None => (path.to_owned(), sftp.list(path).await?),
    };
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Folder)
            .cmp(&(b.kind != EntryKind::Folder))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(Listing {
        parent: parent_folder(&folder),
        folder,
        entries,
    })
}

/// A name for a new folder or a rename, trimmed, unless it cannot be one.
fn checked_name(name: &str) -> Result<&str, BrowseError> {
    let name = name.trim();
    let reason = if name.is_empty() {
        "a name is needed"
    } else if name.contains('/') {
        "a name cannot contain /"
    } else if name == "." || name == ".." {
        "a name cannot be . or .."
    } else {
        return Ok(name);
    };
    Err(BrowseError::BadName {
        reason: reason.into(),
    })
}

/// The folder above `path`, as the device writes paths (`/var/tmp`,
/// `flash:/dir`, `flash:dir`); `None` at the top (`/`, `flash:`).
pub fn parent_folder(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some("/".into()),
        Some(at) => {
            let head = &trimmed[..at];
            Some(if head.ends_with(':') {
                format!("{head}/")
            } else {
                head.to_owned()
            })
        }
        None => match trimmed.find(':') {
            Some(at) if at + 1 < trimmed.len() => Some(trimmed[..=at].to_owned()),
            _ => None,
        },
    }
}

/// `name` in `folder`.
pub fn join_path(folder: &str, name: &str) -> String {
    if folder.ends_with('/') || folder.ends_with(':') {
        format!("{folder}{name}")
    } else {
        format!("{folder}/{name}")
    }
}

/// What `typed` names while `folder` is shown: itself when absolute (or a
/// device's file system, `flash:`), else taken from `folder`; empty is
/// `folder`.
pub fn resolve_path(folder: &str, typed: &str) -> String {
    let typed = typed.trim();
    if typed.is_empty() {
        folder.to_owned()
    } else if typed.starts_with('/') || typed.contains(':') {
        typed.to_owned()
    } else {
        join_path(folder, typed)
    }
}
