//! Browsing a device's files, as the Files tab does (#103): a file
//! connection opened attended, SFTP on it, the listing of one folder at a
//! time, and the few changes made on the device (rename, delete, new
//! folder). [`FileBrowsers`] holds every open browser by id, so the shell
//! only passes commands and events through.
//!
//! Each browser belongs to a host (the tab's): its transfers (#106) go
//! through the transfer engine under that host, so they outlive a browser
//! lost and opened again, and end when the host is closed
//! ([`FileBrowsers::close_host`]). A device without SFTP gives a browser
//! that only sends and fetches by path, over SCP, holding no connection
//! between transfers ([`BrowserEvent::TransferOnly`]). Each of those
//! transfers connects attended (#109): what its login asks comes out as the
//! browser's `HostKey` and `Prompt` events and is answered with the same
//! commands as the browser's own login.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::mpsc;

use super::sftp::{EntryKind, FileError, RemoteEntry, Sftp, StartError};
use super::transfer::{
    Connect, ConnectFailure, Connecting, Direction, Hashes, Link, Outcome, Presence, SizeCheck,
    Status, Transfer, TransferError, TransferId, TransferWatch, Transfers,
};
use super::{Files, Scp};
use crate::known_hosts::KnownHosts;
use crate::platform::PlatformDefinition;
use crate::session::connection::{ConnectError, FileConnection, OpeningEvent};
use crate::session::{HostKeyDecision, HostKeyQuestion, LoginPrompt, SshTarget};

/// How long a closed browser's connection stays for its host's cancelled
/// transfer to clean up (remove its partial file).
const CLOSE_WAIT: Duration = Duration::from_secs(30);

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
    /// Logged in, but the device offers no SFTP (to this account): the
    /// browser sends and fetches by path over SCP, connecting for each
    /// transfer, and holds no connection meanwhile. `reason` says why there
    /// is no SFTP, `hint` how to get a listing (Junos), `folder` is the
    /// folder it was asked to start in (the platform's file folder).
    TransferOnly {
        reason: String,
        hint: Option<String>,
        folder: Option<String>,
    },
    /// It could not be opened (cancelled included): why.
    Failed { reason: String },
    /// The connection ended: why.
    Lost { reason: String },
    /// After `TransferOnly`: the question asked for a transfer's
    /// connection is no longer open, though nobody answered it (the
    /// transfer was cancelled, or its connection gave up). The tab takes it
    /// off.
    QuestionClosed,
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

/// What a browser has once logged in: SFTP (listing and transfers), or
/// only the SCP link (transfers by path).
#[derive(Default)]
struct Reach {
    browser: Option<Arc<Browser>>,
    link: Option<Link>,
}

struct Slot {
    host: String,
    commands: mpsc::UnboundedSender<Command>,
    task: tokio::task::JoinHandle<()>,
    open: Arc<Mutex<Reach>>,
    /// Whether an SCP transfer's question is open over this browser.
    asking: Arc<AtomicBool>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A transfer a browser added, as the Files tab lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    pub id: TransferId,
    pub direction: Direction,
    /// The file's name (the last part of its path).
    pub name: String,
    /// The path on the device.
    pub remote: String,
    /// The local file.
    pub local: PathBuf,
}

/// A transfer's [`Status`] as the Files tab shows it: the outcome's parts
/// in the app's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TransferState {
    Waiting,
    /// Connecting for the transfer (SCP).
    Connecting,
    Running {
        done: u64,
        total: Option<u64>,
    },
    Done {
        bytes: u64,
        /// An upload's local file.
        hashes: Option<Hashes>,
        /// Whether the size was checked, in words.
        size: String,
        checked: bool,
    },
    Failed {
        /// The device's words, or the app's.
        reason: String,
        /// The app's hint for that refusal, apart from it.
        hint: Option<String>,
        /// What was left behind.
        partial: String,
    },
    Cancelled {
        partial: String,
    },
}

impl From<&Status> for TransferState {
    fn from(status: &Status) -> Self {
        match status {
            Status::Waiting => Self::Waiting,
            Status::Connecting => Self::Connecting,
            Status::Running { done, total } => Self::Running {
                done: *done,
                total: *total,
            },
            Status::Finished(Outcome::Done(done)) => Self::Done {
                bytes: done.bytes,
                hashes: done.hashes.clone(),
                size: done.size.to_string(),
                checked: done.size == SizeCheck::Checked,
            },
            Status::Finished(Outcome::Failed { error, partial }) => Self::Failed {
                reason: error.to_string(),
                hint: match error {
                    TransferError::Scp(error) => error.hint().map(str::to_owned),
                    _ => None,
                },
                partial: partial.to_string(),
            },
            Status::Finished(Outcome::Cancelled { partial }) => Self::Cancelled {
                partial: partial.to_string(),
            },
        }
    }
}

/// Follows a transfer for the interface: its state now, then each change,
/// at most one per `every` (states in between are skipped, never the last
/// one). Ends after the last state, or when `send` returns `false`.
pub async fn follow(
    mut watch: TransferWatch,
    every: Duration,
    mut send: impl FnMut(TransferState) -> bool,
) {
    let mut status = watch.latest();
    loop {
        let finished = matches!(status, Status::Finished(_));
        if !send(TransferState::from(&status)) || finished {
            return;
        }
        tokio::time::sleep(every).await;
        status = match watch.changed().await {
            Some(status) => status,
            None => return,
        };
    }
}

/// Which of `names` are already in the local folder `folder`.
pub fn existing_locally(folder: &Path, names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| folder.join(name).exists())
        .cloned()
        .collect()
}

/// The open browsers, by id, and their hosts' transfers. Closing one (or
/// dropping this) closes its connection.
#[derive(Default)]
pub struct FileBrowsers {
    slots: Mutex<HashMap<BrowserId, Slot>>,
    next: AtomicU64,
    transfers: Transfers,
    /// Every host given a transfer, for [`FileBrowsers::close_all`].
    hosts: Mutex<HashSet<String>>,
}

impl FileBrowsers {
    pub fn new() -> Self {
        Self::default()
    }

    fn slots(&self) -> MutexGuard<'_, HashMap<BrowserId, Slot>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens a browser on the host `host` at `target` (build it with
    /// `Profiles::profile_target` / `prompted_target`), checking host keys
    /// against `known_hosts`. It starts in `start` (the platform's file
    /// folder, or where a reconnect was) or, without one or when that
    /// cannot be listed, in the login's folder. `platform` is the host's,
    /// for the hint after a fall back to SCP. Once connected, `host`'s
    /// waiting transfers go through it. Returns at once; follow the events.
    /// Must be called from within a Tokio runtime.
    pub fn open(
        &self,
        host: &str,
        target: SshTarget,
        known_hosts: KnownHosts,
        start: Option<String>,
        platform: Option<&PlatformDefinition>,
    ) -> (BrowserId, mpsc::UnboundedReceiver<BrowserEvent>) {
        let id = BrowserId(self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let (events_tx, events) = mpsc::unbounded_channel();
        let (commands_tx, commands) = mpsc::unbounded_channel();
        let open = Arc::new(Mutex::new(Reach::default()));
        let asking = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(run(
            Run {
                host: host.to_owned(),
                transfers: self.transfers.clone(),
                target,
                known_hosts,
                start,
                platform: platform.cloned(),
                asking: asking.clone(),
            },
            events_tx,
            commands,
            open.clone(),
        ));
        self.slots().insert(
            id,
            Slot {
                host: host.to_owned(),
                commands: commands_tx,
                task,
                open,
                asking,
            },
        );
        (id, events)
    }

    fn command(&self, id: BrowserId, command: Command) {
        if let Some(slot) = self.slots().get(&id) {
            let _ = slot.commands.send(command);
        }
    }

    /// Answers the last login question, in its order (the browser's own
    /// login's, or after `TransferOnly` a transfer's).
    pub fn answer(&self, id: BrowserId, answers: Vec<String>) {
        self.command(id, Command::Answer(answers));
    }

    /// Decides the last host-key question.
    pub fn decide_host_key(&self, id: BrowserId, decision: HostKeyDecision) {
        self.command(id, Command::Decide(decision));
    }

    /// Gives up a login still going on: it ends `Failed`, cancelled. After
    /// `TransferOnly`, the login of the transfer asking: that transfer
    /// fails, "the login was cancelled", and the next one connects.
    pub fn cancel(&self, id: BrowserId) {
        self.command(id, Command::Cancel);
    }

    /// Closes the browser and its connection; nothing more is reported. A
    /// transfer asking over it is cancelled, and the host's waiting ones
    /// wait for the next browser to ask over.
    pub fn close(&self, id: BrowserId) {
        let slot = self.slots().remove(&id);
        if let Some(slot) = &slot {
            let link = slot
                .open
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .link
                .clone();
            if let Some(link) = link {
                self.transfers.forget_scp(&slot.host, &link);
            }
            if slot.asking.load(Ordering::SeqCst) {
                if let Some(running) = self.transfers.running(&slot.host) {
                    self.transfers.cancel(running);
                }
            }
        }
        drop(slot);
    }

    /// Closes the host's browsers and cancels its transfers, running and
    /// waiting (its tab closed).
    pub fn close_host(&self, host: &str) {
        let slots: Vec<Slot> = {
            let mut all = self.slots();
            let ids: Vec<BrowserId> = all
                .iter()
                .filter(|(_, slot)| slot.host == host)
                .map(|(id, _)| *id)
                .collect();
            ids.iter().filter_map(|id| all.remove(id)).collect()
        };
        self.transfers.close(host);
        self.hosts().remove(host);
        self.drop_after_cancelled(vec![host.to_owned()], slots);
    }

    /// Closes every browser and cancels every transfer (the window that
    /// showed them is gone).
    pub fn close_all(&self) {
        let slots: Vec<Slot> = self.slots().drain().map(|(_, slot)| slot).collect();
        let hosts: Vec<String> = self.hosts().drain().collect();
        for host in &hosts {
            self.transfers.close(host);
        }
        self.drop_after_cancelled(hosts, slots);
    }

    /// Drops closed browsers once `hosts`' cancelled transfers have ended
    /// (at most [`CLOSE_WAIT`]): a cancelled upload removes its partial
    /// file over the browser's connection. Outside a runtime, at once.
    fn drop_after_cancelled(&self, hosts: Vec<String>, slots: Vec<Slot>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return drop(slots);
        };
        let transfers = self.transfers.clone();
        runtime.spawn(async move {
            let _ = tokio::time::timeout(CLOSE_WAIT, async {
                while hosts.iter().any(|host| transfers.running(host).is_some()) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;
            drop(slots);
        });
    }

    fn hosts(&self) -> MutexGuard<'_, HashSet<String>> {
        self.hosts.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn browser(&self, id: BrowserId) -> Result<Arc<Browser>, BrowseError> {
        let slots = self.slots();
        let slot = slots.get(&id).ok_or(BrowseError::NotConnected)?;
        let open = slot.open.lock().unwrap_or_else(|e| e.into_inner());
        open.browser.clone().ok_or(BrowseError::NotConnected)
    }

    /// The browser's host and its link (SFTP while connected, SCP once
    /// fallen back).
    fn link(&self, id: BrowserId) -> Result<(String, Link), BrowseError> {
        let slots = self.slots();
        let slot = slots.get(&id).ok_or(BrowseError::NotConnected)?;
        let open = slot.open.lock().unwrap_or_else(|e| e.into_inner());
        let link = open.link.clone().ok_or(BrowseError::NotConnected)?;
        Ok((slot.host.clone(), link))
    }

    /// Adds `transfers` (each with its file's name) to the end of the
    /// browser's host's waiting list.
    fn add(
        &self,
        id: BrowserId,
        transfers: Vec<(String, Transfer)>,
    ) -> Result<Vec<(Added, TransferWatch)>, BrowseError> {
        let (host, link) = self.link(id)?;
        // A link the engine dropped (an SCP connection that failed) is
        // given again: the browser still reaches the device that way.
        if !self.transfers.is_connected(&host) {
            self.transfers.connect(&host, link);
        }
        self.hosts().insert(host.clone());
        Ok(transfers
            .into_iter()
            .map(|(name, transfer)| {
                let direction = transfer.direction;
                let remote = transfer.remote.clone();
                let local = transfer.local.clone();
                let (id, watch) = self.transfers.add(&host, transfer);
                let added = Added {
                    id,
                    direction,
                    name,
                    remote,
                    local,
                };
                (added, watch)
            })
            .collect())
    }

    /// Uploads the local files `locals` into `folder` on the device, under
    /// their own names, replacing files there, after the host's other
    /// transfers.
    pub fn upload(
        &self,
        id: BrowserId,
        folder: &str,
        locals: Vec<PathBuf>,
    ) -> Result<Vec<(Added, TransferWatch)>, BrowseError> {
        let transfers = locals
            .into_iter()
            .map(|local| {
                let name = local_name(&local)?;
                let remote = join_path(folder, &name);
                Ok((name, Transfer::upload(local, remote)))
            })
            .collect::<Result<Vec<_>, BrowseError>>()?;
        self.add(id, transfers)
    }

    /// Downloads the files `names` in `folder` into the local folder
    /// `into`, under their own names, replacing files there.
    pub fn download(
        &self,
        id: BrowserId,
        folder: &str,
        names: Vec<String>,
        into: &Path,
    ) -> Result<Vec<(Added, TransferWatch)>, BrowseError> {
        let transfers = names
            .into_iter()
            .map(|name| {
                let name = checked_name(&name)?.to_owned();
                let remote = join_path(folder, &name);
                let local = into.join(&name);
                Ok((name, Transfer::download(remote, local)))
            })
            .collect::<Result<Vec<_>, BrowseError>>()?;
        self.add(id, transfers)
    }

    /// Sends the local file `local` to the path `remote` (the device's own
    /// form), replacing a file there.
    pub fn send(
        &self,
        id: BrowserId,
        local: PathBuf,
        remote: &str,
    ) -> Result<(Added, TransferWatch), BrowseError> {
        let remote = remote.trim();
        let name = checked_name(remote_name(remote))
            .map_err(|_| bad_name("a path on the device, ending in the file's name, is needed"))?
            .to_owned();
        local_name(&local)?;
        let mut added = self.add(id, vec![(name, Transfer::upload(local, remote))])?;
        Ok(added.pop().expect("one added"))
    }

    /// Fetches the file at `remote` into the local folder `into`, under
    /// its own name, replacing a file there.
    pub fn fetch(
        &self,
        id: BrowserId,
        remote: &str,
        into: &Path,
    ) -> Result<(Added, TransferWatch), BrowseError> {
        let remote = remote.trim();
        let name = checked_name(remote_name(remote))
            .map_err(|_| bad_name("a path to a file on the device is needed"))?
            .to_owned();
        let local = into.join(&name);
        let mut added = self.add(id, vec![(name, Transfer::download(remote, local))])?;
        Ok(added.pop().expect("one added"))
    }

    /// Which of `names` are already in `folder` on the device. Over SCP
    /// nothing can be asked: none.
    pub async fn present(
        &self,
        id: BrowserId,
        folder: &str,
        names: Vec<String>,
    ) -> Result<Vec<String>, BrowseError> {
        let (_, link) = self.link(id)?;
        let mut present = Vec::new();
        for name in names {
            let path = join_path(folder, &name);
            if let Presence::Exists { .. } = link.presence(&path).await? {
                present.push(name);
            }
        }
        Ok(present)
    }

    /// Cancels a waiting or running transfer; `false` once it has ended.
    pub fn cancel_transfer(&self, id: TransferId) -> bool {
        self.transfers.cancel(id)
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

/// What a browser's task is given.
struct Run {
    host: String,
    transfers: Transfers,
    target: SshTarget,
    known_hosts: KnownHosts,
    start: Option<String>,
    platform: Option<PlatformDefinition>,
    asking: Arc<AtomicBool>,
}

/// A browser's life: log in (passing questions out and answers in), start
/// SFTP (or fall back to SCP, whose transfers then ask over the browser),
/// list the starting folder, then wait for the connection to end.
async fn run(
    run: Run,
    events: mpsc::UnboundedSender<BrowserEvent>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    open: Arc<Mutex<Reach>>,
) {
    let Run {
        host,
        transfers,
        target,
        known_hosts,
        start,
        platform,
        asking,
    } = run;
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
    let (connection, sftp) = match connection.start_files(platform.as_ref()).await {
        Ok(Files::Sftp { connection, sftp }) => (connection, sftp),
        Ok(Files::Scp(scp)) => {
            let reason = StartError::NoSftp(scp.no_sftp()).to_string();
            let hint = scp.hint().map(str::to_owned);
            let link = Link::scp(Asking {
                scp,
                events: events.clone(),
                commands: tokio::sync::Mutex::new(commands),
                asking,
            });
            open.lock().unwrap_or_else(|e| e.into_inner()).link = Some(link.clone());
            transfers.connect(&host, link);
            return send(BrowserEvent::TransferOnly {
                reason,
                hint,
                folder: start,
            });
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
    let link = Link::sftp(sftp.clone());
    let browser = Arc::new(Browser { sftp, connection });
    *open.lock().unwrap_or_else(|e| e.into_inner()) = Reach {
        browser: Some(browser.clone()),
        link: Some(link.clone()),
    };
    // The host's waiting transfers (from before a loss) go on through it.
    transfers.connect(&host, link);
    send(BrowserEvent::Connected { listing, notice });
    let lost = browser.connection.lost().await;
    *open.lock().unwrap_or_else(|e| e.into_inner()) = Reach::default();
    drop(browser);
    send(BrowserEvent::Lost {
        reason: lost.to_string(),
    });
}

/// How an SCP-only browser's transfers connect, one connection each:
/// attended, as the browser's own login was. A host-key or login question
/// goes out as the browser's event and its answer comes in through the
/// browser's commands. The login typed for the tab is in the target, so
/// only what the device itself asks is asked. Transfers run one at a time
/// per host, so one connection asks at a time.
struct Asking {
    scp: Scp,
    events: mpsc::UnboundedSender<BrowserEvent>,
    commands: tokio::sync::Mutex<mpsc::UnboundedReceiver<Command>>,
    /// Whether a question is open (the slot's, for `close`).
    asking: Arc<AtomicBool>,
}

impl Connect for Asking {
    fn connect(&self) -> Connecting<'_> {
        Box::pin(self.attended())
    }
}

/// The question a transfer's connection has open: when it goes without an
/// answer (the transfer cancelled, the connection given up), the tab is
/// told to take it off.
struct Open<'a> {
    events: &'a mpsc::UnboundedSender<BrowserEvent>,
    asking: &'a AtomicBool,
}

impl Open<'_> {
    fn ask(&self, event: BrowserEvent) {
        self.asking.store(true, Ordering::SeqCst);
        let _ = self.events.send(event);
    }

    fn answered(&self) {
        self.asking.store(false, Ordering::SeqCst);
    }
}

impl Drop for Open<'_> {
    fn drop(&mut self) {
        if self.asking.swap(false, Ordering::SeqCst) {
            let _ = self.events.send(BrowserEvent::QuestionClosed);
        }
    }
}

impl Asking {
    async fn attended(&self) -> Result<FileConnection, ConnectFailure> {
        let failed = |error: ConnectError| ConnectFailure::Failed(error.to_string());
        let mut commands = self.commands.lock().await;
        // What was sent while nothing was asked answers nothing now.
        loop {
            match commands.try_recv() {
                Ok(_) => continue,
                Err(mpsc::error::TryRecvError::Empty) => break,
                // The browser is closed: nobody is there to ask.
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    return self.scp.connect_unattended().await.map_err(failed);
                }
            }
        }
        let open = Open {
            events: &self.events,
            asking: &self.asking,
        };
        let mut opening = self.scp.connect();
        let mut attended = true;
        let mut declined = false;
        loop {
            tokio::select! {
                event = opening.next() => match event {
                    Some(OpeningEvent::HostKey(question)) if !attended => {
                        return Err(failed(ConnectError::HostKeyRefused(question)));
                    }
                    Some(OpeningEvent::Prompt(prompt)) if !attended => {
                        return Err(failed(ConnectError::QuestionRefused(prompt)));
                    }
                    Some(OpeningEvent::HostKey(question)) => {
                        open.ask(BrowserEvent::HostKey { question });
                    }
                    Some(OpeningEvent::Prompt(prompt)) => open.ask(BrowserEvent::Prompt { prompt }),
                    Some(OpeningEvent::Opened(connection)) => return Ok(connection),
                    Some(OpeningEvent::Failed(error)) => {
                        let reason = error.to_string();
                        return Err(if declined {
                            ConnectFailure::Declined(reason)
                        } else {
                            ConnectFailure::Failed(reason)
                        });
                    }
                    None => {
                        return Err(ConnectFailure::Failed(
                            "the connection ended while it was being opened".into(),
                        ));
                    }
                },
                command = commands.recv(), if attended => match command {
                    Some(Command::Answer(answers)) => {
                        open.answered();
                        opening.answer(answers);
                    }
                    Some(Command::Decide(decision)) => {
                        open.answered();
                        declined |= decision == HostKeyDecision::Cancel;
                        opening.decide_host_key(decision);
                    }
                    Some(Command::Cancel) => {
                        open.answered();
                        return Err(ConnectFailure::Declined("the login was cancelled".into()));
                    }
                    // The browser closed while this connected (closing
                    // cancels a transfer whose question is open): what is
                    // asked from now on is refused.
                    None if self.asking.load(Ordering::SeqCst) => {
                        return Err(ConnectFailure::Failed(
                            "the tab that asked for this transfer was closed".into(),
                        ));
                    }
                    None => attended = false,
                },
            }
        }
    }
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

fn bad_name(reason: &str) -> BrowseError {
    BrowseError::BadName {
        reason: reason.into(),
    }
}

/// A local file's name, as it is called on the device.
fn local_name(local: &Path) -> Result<String, BrowseError> {
    local
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| bad_name(&format!("{} is not a file", local.display())))
}

/// The last part of a device path (after `/` or a file system's `:`).
fn remote_name(path: &str) -> &str {
    path.rsplit(['/', ':']).next().unwrap_or(path)
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
