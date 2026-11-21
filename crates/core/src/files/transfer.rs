//! The transfer engine: whole files up to a device and down from it, over
//! the protocol its files are reached by (SFTP, or SCP with a connection
//! per transfer), one at a time per host, the rest waiting in order.
//!
//! A caller (the Files tab, the file job) gives the engine a [`Link`] for
//! each host and adds [`Transfer`]s; each comes back with a
//! [`TransferWatch`] that follows it from waiting to its [`Outcome`].
//! Watching never holds a transfer up: a watch keeps only the latest
//! status, however slowly (or never) it is read.
//!
//! What a transfer does besides moving bytes:
//!
//! - an upload computes the local file's MD5 and SHA-256 while reading it
//!   and gives them when it ends;
//! - the size is checked: after an SFTP upload the remote file is stat'ed,
//!   after a download the local file's size is compared with what the
//!   device said (SFTP's stat, SCP's header); an SCP upload cannot be
//!   checked, and its outcome says so;
//! - a failed or cancelled SFTP upload removes its partial remote file (or
//!   says why it remains), an SCP upload says one may remain, a failed or
//!   cancelled download removes its partial local file;
//! - a transfer that moves no byte for three connect timeouts fails (the
//!   stall rule);
//! - a lost connection fails the running transfer with the reason; the
//!   waiting ones wait until the caller gives the host a new link
//!   ([`Transfers::connect`]) or cancels them.
//!
//! Uploads write to the final name, with no temporary name and rename. A
//! file is never held in memory: it goes through a buffer of
//! [`BUFFER_SIZE`].

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use sha2::Digest;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::watch;

use super::sftp::file_error;
use super::{FileConnection, FileError, Files, Lost, Protocol, Scp, ScpError, Sftp};

/// How much of a file is read (or written) at a time, locally.
const BUFFER_SIZE: usize = 256 * 1024;

/// How long a transfer that failed for a lost SCP connection waits to
/// learn why.
const LOST_REASON_WAIT: Duration = Duration::from_secs(1);

/// One transfer, as [`Transfers::add`] numbers it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct TransferId(u64);

/// Which way a transfer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// A local file to the device.
    Upload,
    /// A file on the device to a local file.
    Download,
}

/// What to transfer: a local file and a path on the device (its own
/// form: `/var/tmp/x`, `flash:x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: String,
}

impl Transfer {
    /// Sends the local file `local` to `remote`, replacing a file there.
    pub fn upload(local: impl Into<PathBuf>, remote: impl Into<String>) -> Self {
        Self {
            direction: Direction::Upload,
            local: local.into(),
            remote: remote.into(),
        }
    }

    /// Fetches `remote` into the local file `local`, replacing a file
    /// there.
    pub fn download(remote: impl Into<String>, local: impl Into<PathBuf>) -> Self {
        Self {
            direction: Direction::Download,
            local: local.into(),
            remote: remote.into(),
        }
    }
}

/// Where a transfer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// In its host's waiting list.
    Waiting,
    /// Connecting for the transfer (SCP).
    Connecting,
    /// Moving bytes: `done` of `total` (`None` while the size is not known).
    Running { done: u64, total: Option<u64> },
    /// Ended. The last status.
    Finished(Outcome),
}

/// How a transfer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done(Done),
    Failed {
        error: TransferError,
        partial: Partial,
    },
    /// Cancelled, waiting or running.
    Cancelled {
        partial: Partial,
    },
}

/// A transfer that went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The file's size.
    pub bytes: u64,
    /// The local file's hashes, for an upload.
    pub hashes: Option<Hashes>,
    pub size: SizeCheck,
}

/// A file's hashes, in lower-case hex.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hashes {
    pub md5: String,
    pub sha256: String,
}

/// Whether the size of what arrived was checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
pub enum SizeCheck {
    /// The size that arrived is the size that was sent.
    #[error("the size was checked")]
    Checked,
    /// It could not be: an SCP upload (SCP has no way to ask), or a device
    /// that did not say a file's size.
    #[error("the size the device wrote could not be checked")]
    NotChecked,
}

/// What a transfer that did not go through left behind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Partial {
    #[error("no partial file was left")]
    Nothing,
    #[error("the partial file was removed from the device")]
    RemovedFromDevice,
    /// The remove failed (a Junos read-only login cannot delete; the
    /// connection was lost).
    #[error("a partial file remains on the device; it could not be removed ({reason})")]
    RemainsOnDevice { reason: String },
    /// SCP cannot remove a file; some devices (IOS) remove it themselves.
    #[error("a partial file may remain on the device (SCP cannot remove it)")]
    MayRemainOnDevice,
    #[error("the partial local file was removed")]
    RemovedLocally,
    #[error("a partial local file remains; it could not be removed ({reason})")]
    RemainsLocally { reason: String },
}

/// Why a transfer failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    /// The local file could not be read or written.
    #[error("{0}")]
    Local(String),
    /// SFTP's refusal or failure, in the app's words.
    #[error("{0}")]
    Device(FileError),
    /// SCP's.
    #[error("{0}")]
    Scp(ScpError),
    #[error("the connection was lost: {0}")]
    Lost(Lost),
    /// No byte moved for the stall limit (three connect timeouts).
    #[error("no byte moved for {} s: the device stopped answering", .after.as_secs())]
    Stalled { after: Duration },
    /// After an SFTP upload the device holds another size than was sent.
    #[error("the device holds {found} bytes of the {sent} sent")]
    SizeOnDevice { sent: u64, found: u64 },
    /// A download ended with another size than the device announced.
    #[error("the device announced {announced} bytes and {received} arrived")]
    SizeDiffers { announced: u64, received: u64 },
    /// Connecting for an SCP transfer failed.
    #[error("could not connect for the transfer: {0}")]
    Connect(String),
    /// The user said no to a question while the transfer connected (the
    /// login cancelled, the host key refused): why. Only this transfer
    /// fails; the host's next one connects and asks for itself.
    #[error("{0}")]
    Declined(String),
}

impl TransferError {
    /// Whether the host's connection is gone: its waiting transfers then
    /// wait for a new link.
    fn connection_gone(&self) -> bool {
        matches!(self, Self::Lost(_) | Self::Connect(_))
    }
}

/// Whether a file is on the device, for an overwrite question and the
/// file job's "already there" rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Presence {
    /// It is there (`size` when the device says).
    Exists {
        size: Option<u64>,
    },
    Absent,
    /// The protocol cannot ask (SCP).
    CannotTell,
}

impl Sftp {
    /// Whether `path` is there, and its size.
    pub async fn presence(&self, path: &str) -> Result<Presence, FileError> {
        match self.stat(path).await {
            Ok(entry) => Ok(Presence::Exists { size: entry.size }),
            Err(FileError::NotFound { .. }) => Ok(Presence::Absent),
            Err(error) => Err(error),
        }
    }
}

impl Files {
    /// [`Sftp::presence`]; over SCP, [`Presence::CannotTell`].
    pub async fn presence(&self, path: &str) -> Result<Presence, FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.presence(path).await,
            Self::Scp(_) => Ok(Presence::CannotTell),
        }
    }
}

/// A future the engine awaits; `Send`, for its tasks.
pub type Connecting<'a> =
    Pin<Box<dyn Future<Output = Result<FileConnection, ConnectFailure>> + Send + 'a>>;

/// Why [`Connect::connect`] gave no connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectFailure {
    /// The connection failed (`TransferError::Connect`): the host's
    /// waiting transfers wait for a new link.
    #[error("{0}")]
    Failed(String),
    /// The user said no to a question (`TransferError::Declined`): this
    /// transfer fails alone and the next one connects.
    #[error("{0}")]
    Declined(String),
}

impl From<String> for ConnectFailure {
    fn from(reason: String) -> Self {
        Self::Failed(reason)
    }
}

/// How an SCP transfer gets its connection: one per transfer. [`Scp`]
/// connects unattended; a caller that asks the user (the Files tab) gives
/// its own.
pub trait Connect: Send + Sync + 'static {
    /// A new logged-in connection, or why there is none.
    fn connect(&self) -> Connecting<'_>;
}

impl Connect for Scp {
    fn connect(&self) -> Connecting<'_> {
        Box::pin(async move {
            self.connect_unattended()
                .await
                .map_err(|error| ConnectFailure::Failed(error.to_string()))
        })
    }
}

/// How the engine reaches a host's files.
#[derive(Clone)]
pub struct Link {
    inner: LinkInner,
}

#[derive(Clone)]
enum LinkInner {
    /// SFTP on an open connection, shared by the host's transfers.
    Sftp {
        sftp: Sftp,
        /// Held when the link owns it ([`Link::from`] a [`Files`]).
        _connection: Option<Arc<FileConnection>>,
    },
    /// SCP: a connection per transfer.
    Scp(Arc<dyn Connect>),
}

impl Link {
    /// SFTP on a connection the caller keeps open (the Files tab's
    /// browser shares it).
    pub fn sftp(sftp: Sftp) -> Self {
        Self {
            inner: LinkInner::Sftp {
                sftp,
                _connection: None,
            },
        }
    }

    /// SCP, connecting for each transfer with `connect`.
    pub fn scp(connect: impl Connect) -> Self {
        Self {
            inner: LinkInner::Scp(Arc::new(connect)),
        }
    }

    pub fn protocol(&self) -> Protocol {
        match self.inner {
            LinkInner::Sftp { .. } => Protocol::Sftp,
            LinkInner::Scp(_) => Protocol::Scp,
        }
    }

    /// Whether `path` is there ([`Sftp::presence`]); over SCP,
    /// [`Presence::CannotTell`].
    pub async fn presence(&self, path: &str) -> Result<Presence, FileError> {
        match &self.inner {
            LinkInner::Sftp { sftp, .. } => sftp.presence(path).await,
            LinkInner::Scp(_) => Ok(Presence::CannotTell),
        }
    }

    /// Whether `other` is this SCP link (or a clone of it).
    fn is_scp(&self, other: &Link) -> bool {
        match (&self.inner, &other.inner) {
            (LinkInner::Scp(this), LinkInner::Scp(other)) => Arc::ptr_eq(this, other),
            _ => false,
        }
    }

    /// Whether the link's connection is known to be gone.
    fn lost(&self) -> bool {
        match &self.inner {
            LinkInner::Sftp { sftp, .. } => sftp.lost_reason().is_some(),
            LinkInner::Scp(_) => false,
        }
    }
}

impl From<Files> for Link {
    /// A link that owns the connection (SFTP) or connects per transfer
    /// (SCP, unattended).
    fn from(files: Files) -> Self {
        match files {
            Files::Sftp { connection, sftp } => Self {
                inner: LinkInner::Sftp {
                    sftp,
                    _connection: Some(Arc::new(connection)),
                },
            },
            Files::Scp(scp) => Self::scp(scp),
        }
    }
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Link").field(&self.protocol()).finish()
    }
}

/// Follows one transfer: its latest [`Status`], ending with
/// [`Status::Finished`]. Only the latest status is kept, so a watch read
/// slowly (or never) costs the transfer nothing.
#[derive(Debug)]
pub struct TransferWatch {
    status: watch::Receiver<Status>,
}

impl TransferWatch {
    /// The latest status.
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// The latest status, marked as seen.
    pub(crate) fn latest(&mut self) -> Status {
        self.status.borrow_and_update().clone()
    }

    /// The next status not seen yet (statuses in between may be skipped);
    /// `None` once the finished one has been seen.
    pub async fn changed(&mut self) -> Option<Status> {
        self.status.changed().await.ok()?;
        Some(self.status.borrow_and_update().clone())
    }

    /// Waits for the transfer to end.
    pub async fn outcome(mut self) -> Outcome {
        let finished = self
            .status
            .wait_for(|status| matches!(status, Status::Finished(_)))
            .await;
        match finished.as_deref() {
            Ok(Status::Finished(outcome)) => outcome.clone(),
            _ => Outcome::Failed {
                error: TransferError::Local("the transfer ended without an outcome".into()),
                partial: Partial::Nothing,
            },
        }
    }
}

/// The transfer engine: a waiting list per host, one transfer running per
/// host, hosts side by side. Cloning gives another handle on the same
/// engine.
///
/// [`Transfers::add`] and [`Transfers::connect`] start tasks: they must be
/// called within a Tokio runtime.
#[derive(Clone, Default)]
pub struct Transfers {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    next: u64,
    hosts: HashMap<String, Host>,
}

#[derive(Default)]
struct Host {
    link: Option<Link>,
    /// Counts the links given, so that a transfer that lost the old one
    /// does not drop a new one.
    generation: u64,
    waiting: VecDeque<Queued>,
    running: Option<Running>,
}

struct Queued {
    id: TransferId,
    transfer: Transfer,
    status: watch::Sender<Status>,
}

struct Running {
    id: TransferId,
    cancel: watch::Sender<bool>,
}

impl Transfers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives `host` the link its transfers use from now on (a running one
    /// keeps its own), and starts the next waiting one.
    pub fn connect(&self, host: &str, link: Link) {
        {
            let mut state = self.state.lock().unwrap();
            let entry = state.hosts.entry(host.to_owned()).or_default();
            entry.link = Some(link);
            entry.generation += 1;
        }
        self.pump(host);
    }

    /// Whether `host` has a link that is not known to be lost. Without
    /// one, its transfers wait.
    pub fn is_connected(&self, host: &str) -> bool {
        let state = self.state.lock().unwrap();
        state
            .hosts
            .get(host)
            .and_then(|host| host.link.as_ref())
            .is_some_and(|link| !link.lost())
    }

    /// Adds `transfer` to the end of `host`'s waiting list. It starts when
    /// it comes first and the host has a link.
    pub fn add(&self, host: &str, transfer: Transfer) -> (TransferId, TransferWatch) {
        let (status, watch) = watch::channel(Status::Waiting);
        let id = {
            let mut state = self.state.lock().unwrap();
            state.next += 1;
            let id = TransferId(state.next);
            let entry = state.hosts.entry(host.to_owned()).or_default();
            entry.waiting.push_back(Queued {
                id,
                transfer,
                status,
            });
            id
        };
        self.pump(host);
        (id, TransferWatch { status: watch })
    }

    /// `host`'s waiting transfers, in the order they will run.
    pub fn waiting(&self, host: &str) -> Vec<TransferId> {
        let state = self.state.lock().unwrap();
        state
            .hosts
            .get(host)
            .map(|host| host.waiting.iter().map(|queued| queued.id).collect())
            .unwrap_or_default()
    }

    /// `host`'s running transfer.
    pub fn running(&self, host: &str) -> Option<TransferId> {
        let state = self.state.lock().unwrap();
        state.hosts.get(host)?.running.as_ref().map(|r| r.id)
    }

    /// Cancels a transfer: a waiting one leaves its list at once
    /// ([`Outcome::Cancelled`], nothing left behind); a running one stops
    /// and deals with its partial file. `false` when it is not waiting or
    /// running (finished, or unknown).
    pub fn cancel(&self, id: TransferId) -> bool {
        let mut state = self.state.lock().unwrap();
        for host in state.hosts.values_mut() {
            if let Some(at) = host.waiting.iter().position(|queued| queued.id == id) {
                let queued = host.waiting.remove(at).expect("just found");
                queued
                    .status
                    .send_replace(Status::Finished(Outcome::Cancelled {
                        partial: Partial::Nothing,
                    }));
                return true;
            }
            if let Some(running) = host.running.as_ref().filter(|r| r.id == id) {
                running.cancel.send_replace(true);
                return true;
            }
        }
        false
    }

    /// Cancels every transfer of `host`, waiting and running, and forgets
    /// its link (the Files tab closing).
    pub fn close(&self, host: &str) {
        let ids: Vec<TransferId> = {
            let mut state = self.state.lock().unwrap();
            let Some(entry) = state.hosts.get_mut(host) else {
                return;
            };
            entry.link = None;
            entry.generation += 1;
            entry
                .running
                .iter()
                .map(|r| r.id)
                .chain(entry.waiting.iter().map(|queued| queued.id))
                .collect()
        };
        for id in ids {
            self.cancel(id);
        }
    }

    /// Forgets `host`'s link if it is the SCP link `link` (whoever asked
    /// its questions is gone): the waiting transfers wait for a new one. A
    /// running one keeps its own.
    pub(crate) fn forget_scp(&self, host: &str, link: &Link) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.hosts.get_mut(host) {
            if entry.link.as_ref().is_some_and(|held| held.is_scp(link)) {
                entry.link = None;
                entry.generation += 1;
            }
        }
    }

    /// Starts `host`'s next transfer if none runs and it has a link.
    fn pump(&self, host: &str) {
        let mut state = self.state.lock().unwrap();
        let Some(entry) = state.hosts.get_mut(host) else {
            return;
        };
        if entry.running.is_some() || entry.waiting.is_empty() {
            return;
        }
        let Some(link) = entry.link.clone() else {
            return;
        };
        if link.lost() {
            // The waiting ones wait for a new link.
            entry.link = None;
            return;
        }
        let queued = entry.waiting.pop_front().expect("not empty");
        let (cancel, cancelled) = watch::channel(false);
        entry.running = Some(Running {
            id: queued.id,
            cancel,
        });
        let generation = entry.generation;
        let engine = self.clone();
        let host = host.to_owned();
        tokio::spawn(async move {
            let outcome = run(&link, &queued.transfer, &queued.status, cancelled).await;
            let gone = matches!(&outcome, Outcome::Failed { error, .. } if error.connection_gone());
            {
                let mut state = engine.state.lock().unwrap();
                if let Some(entry) = state.hosts.get_mut(&host) {
                    entry.running = None;
                    if gone && entry.generation == generation {
                        entry.link = None;
                    }
                }
                // Sent under the lock, so that a caller who sees the
                // outcome sees the next transfer started (or the host
                // waiting for a link).
                queued.status.send_replace(Status::Finished(outcome));
            }
            engine.pump(&host);
        });
    }
}

/// Resolves when the transfer is cancelled (never, if the engine is gone).
async fn cancelled(mut cancel: watch::Receiver<bool>) {
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Runs one transfer to its outcome.
async fn run(
    link: &Link,
    transfer: &Transfer,
    status: &watch::Sender<Status>,
    cancel: watch::Receiver<bool>,
) -> Outcome {
    let progress = Progress {
        status,
        total: Mutex::new(None),
    };
    let local = transfer.local.as_path();
    let remote = transfer.remote.as_str();
    match (&link.inner, transfer.direction) {
        (LinkInner::Sftp { sftp, .. }, Direction::Upload) => {
            sftp_upload(sftp, local, remote, &progress, cancel).await
        }
        (LinkInner::Sftp { sftp, .. }, Direction::Download) => {
            sftp_download(sftp, remote, local, &progress, cancel).await
        }
        (LinkInner::Scp(connect), Direction::Upload) => {
            scp_upload(connect.as_ref(), local, remote, &progress, cancel).await
        }
        (LinkInner::Scp(connect), Direction::Download) => {
            scp_download(connect.as_ref(), remote, local, &progress, cancel).await
        }
    }
}

/// A running transfer's progress, sent to its watch.
struct Progress<'a> {
    status: &'a watch::Sender<Status>,
    total: Mutex<Option<u64>>,
}

impl Progress<'_> {
    fn total(&self, total: Option<u64>) {
        *self.total.lock().unwrap() = total;
        self.moved(0);
    }

    fn moved(&self, done: u64) {
        let total = *self.total.lock().unwrap();
        self.status.send_replace(Status::Running { done, total });
    }

    fn connecting(&self) {
        self.status.send_replace(Status::Connecting);
    }
}

/// MD5 and SHA-256 together.
struct Hasher {
    md5: md5::Context,
    sha256: sha2::Sha256,
}

impl Hasher {
    fn new() -> Self {
        Self {
            md5: md5::Context::new(),
            sha256: sha2::Sha256::new(),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.md5.consume(bytes);
        self.sha256.update(bytes);
    }

    fn finish(self) -> Hashes {
        Hashes {
            md5: hex(&self.md5.finalize().0),
            sha256: hex(&self.sha256.finalize()),
        }
    }
}

/// The hashes of the local file at `path`, read through a buffer of
/// [`BUFFER_SIZE`] (blocking: call it off the runtime's threads).
pub(crate) fn hash_file(path: &std::path::Path) -> std::io::Result<Hashes> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0; BUFFER_SIZE];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finish());
        }
        hasher.update(&buffer[..read]);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `operation`, failing as stalled when it takes longer than `limit`.
async fn within<T>(
    limit: Duration,
    operation: impl Future<Output = Result<T, TransferError>>,
) -> Result<T, TransferError> {
    tokio::time::timeout(limit, operation)
        .await
        .unwrap_or(Err(TransferError::Stalled { after: limit }))
}

/// `operation` on `sftp`, failing at once when the connection ends.
async fn while_connected<T>(
    sftp: &Sftp,
    operation: impl Future<Output = Result<T, TransferError>>,
) -> Result<T, TransferError> {
    tokio::select! {
        biased;
        lost = sftp.ended() => Err(TransferError::Lost(lost)),
        done = operation => done,
    }
}

/// SFTP's `error` as a transfer's: the lost connection with its reason, a
/// request that timed out as the stall rule.
fn device_error(sftp: &Sftp, error: FileError) -> TransferError {
    if let Some(lost) = sftp.lost_reason() {
        return TransferError::Lost(lost);
    }
    match error {
        FileError::TimedOut { after } => TransferError::Stalled { after },
        FileError::ConnectionLost => {
            TransferError::Lost(Lost::Broken("the connection ended".into()))
        }
        error => TransferError::Device(error),
    }
}

fn local_error(action: &str, path: &Path, error: io::Error) -> TransferError {
    TransferError::Local(format!("could not {action} {}: {error}", path.display()))
}

/// Removes a failed SFTP upload's partial file.
async fn remove_remote(sftp: &Sftp, remote: &str) -> Partial {
    let removing = async {
        sftp.delete_file(remote)
            .await
            .map_err(|error| device_error(sftp, error))
    };
    match while_connected(sftp, removing).await {
        Ok(()) => Partial::RemovedFromDevice,
        Err(TransferError::Device(FileError::NotFound { .. })) => Partial::Nothing,
        Err(error) => Partial::RemainsOnDevice {
            reason: error.to_string(),
        },
    }
}

/// Removes a failed download's partial local file.
async fn remove_local(local: &Path) -> Partial {
    match tokio::fs::remove_file(local).await {
        Ok(()) => Partial::RemovedLocally,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Partial::Nothing,
        Err(error) => Partial::RemainsLocally {
            reason: error.to_string(),
        },
    }
}

async fn sftp_upload(
    sftp: &Sftp,
    local: &Path,
    remote: &str,
    progress: &Progress<'_>,
    cancel: watch::Receiver<bool>,
) -> Outcome {
    let failed = |error| Outcome::Failed {
        error,
        partial: Partial::Nothing,
    };
    let mut file = match tokio::fs::File::open(local).await {
        Ok(file) => file,
        Err(error) => return failed(local_error("open", local, error)),
    };
    let total = file.metadata().await.ok().map(|metadata| metadata.len());
    progress.total(total);
    let limit = sftp.stall_limit();
    let writer = tokio::select! {
        biased;
        () = cancelled(cancel.clone()) => return Outcome::Cancelled { partial: Partial::Nothing },
        created = while_connected(sftp, within(limit, async {
            sftp.create(remote).await.map_err(|error| device_error(sftp, error))
        })) => match created {
            Ok(writer) => writer,
            Err(error) => return failed(error),
        },
    };
    let mut hasher = Hasher::new();
    let copying = async {
        let mut writer = writer;
        let mut buffer = vec![0; BUFFER_SIZE];
        let mut done = 0u64;
        loop {
            let n = file
                .read(&mut buffer)
                .await
                .map_err(|error| local_error("read", local, error))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            within(limit, async {
                writer
                    .write_all(&buffer[..n])
                    .await
                    .map_err(|error| device_error(sftp, file_error(error)))
            })
            .await?;
            done += n as u64;
            progress.moved(done);
        }
        within(limit, async {
            writer
                .finish()
                .await
                .map_err(|error| device_error(sftp, error))
        })
        .await?;
        Ok(done)
    };
    let copied = tokio::select! {
        biased;
        () = cancelled(cancel) => None,
        copied = while_connected(sftp, copying) => Some(copied),
    };
    let sent = match copied {
        None => {
            return Outcome::Cancelled {
                partial: remove_remote(sftp, remote).await,
            }
        }
        Some(Err(error)) => {
            return Outcome::Failed {
                error,
                partial: remove_remote(sftp, remote).await,
            }
        }
        Some(Ok(sent)) => sent,
    };
    let stat = async {
        sftp.stat(remote)
            .await
            .map_err(|error| device_error(sftp, error))
    };
    let size = match while_connected(sftp, stat).await {
        Ok(entry) => entry.size,
        Err(error) => {
            return Outcome::Failed {
                error,
                partial: remove_remote(sftp, remote).await,
            }
        }
    };
    match size {
        Some(found) if found != sent => Outcome::Failed {
            error: TransferError::SizeOnDevice { sent, found },
            partial: remove_remote(sftp, remote).await,
        },
        found => Outcome::Done(Done {
            bytes: sent,
            hashes: Some(hasher.finish()),
            size: if found.is_some() {
                SizeCheck::Checked
            } else {
                SizeCheck::NotChecked
            },
        }),
    }
}

async fn sftp_download(
    sftp: &Sftp,
    remote: &str,
    local: &Path,
    progress: &Progress<'_>,
    cancel: watch::Receiver<bool>,
) -> Outcome {
    let failed = |error| Outcome::Failed {
        error,
        partial: Partial::Nothing,
    };
    let limit = sftp.stall_limit();
    let opened = tokio::select! {
        biased;
        () = cancelled(cancel.clone()) => return Outcome::Cancelled { partial: Partial::Nothing },
        opened = while_connected(sftp, within(limit, async {
            let opening = async {
                let entry = sftp.stat(remote).await?;
                let reader = sftp.open_read(remote).await?;
                Ok((entry.size, reader))
            };
            opening.await.map_err(|error| device_error(sftp, error))
        })) => opened,
    };
    let (announced, reader) = match opened {
        Ok(opened) => opened,
        Err(error) => return failed(error),
    };
    progress.total(announced);
    let file = match tokio::fs::File::create(local).await {
        Ok(file) => file,
        Err(error) => return failed(local_error("create", local, error)),
    };
    let copying = async {
        let (mut reader, mut file) = (reader, file);
        let mut buffer = vec![0; BUFFER_SIZE];
        let mut received = 0u64;
        loop {
            let n = within(limit, async {
                reader
                    .read(&mut buffer)
                    .await
                    .map_err(|error| device_error(sftp, file_error(error)))
            })
            .await?;
            if n == 0 {
                break;
            }
            file.write_all(&buffer[..n])
                .await
                .map_err(|error| local_error("write", local, error))?;
            received += n as u64;
            progress.moved(received);
        }
        file.flush()
            .await
            .map_err(|error| local_error("write", local, error))?;
        Ok(received)
    };
    let copied = tokio::select! {
        biased;
        () = cancelled(cancel) => None,
        copied = while_connected(sftp, copying) => Some(copied),
    };
    match copied {
        None => Outcome::Cancelled {
            partial: remove_local(local).await,
        },
        Some(Err(error)) => Outcome::Failed {
            error,
            partial: remove_local(local).await,
        },
        Some(Ok(received)) => match announced {
            Some(announced) => checked_on_disk(local, announced, received).await,
            None => Outcome::Done(Done {
                bytes: received,
                hashes: None,
                size: SizeCheck::NotChecked,
            }),
        },
    }
}

/// A download that went through, once the local file holds the size the
/// device announced; it is removed otherwise.
async fn checked_on_disk(local: &Path, announced: u64, received: u64) -> Outcome {
    let on_disk = match tokio::fs::metadata(local).await {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return Outcome::Failed {
                error: local_error("read the size of", local, error),
                partial: remove_local(local).await,
            }
        }
    };
    if on_disk != announced {
        return Outcome::Failed {
            error: TransferError::SizeDiffers {
                announced,
                received: on_disk,
            },
            partial: remove_local(local).await,
        };
    }
    Outcome::Done(Done {
        bytes: received,
        hashes: None,
        size: SizeCheck::Checked,
    })
}

/// Connects for an SCP transfer, unless it is cancelled first (`None`).
async fn scp_connection(
    connect: &dyn Connect,
    progress: &Progress<'_>,
    cancel: watch::Receiver<bool>,
) -> Option<Result<FileConnection, TransferError>> {
    progress.connecting();
    tokio::select! {
        biased;
        () = cancelled(cancel) => None,
        connected = connect.connect() => Some(connected.map_err(|failure| match failure {
            ConnectFailure::Failed(reason) => TransferError::Connect(reason),
            ConnectFailure::Declined(reason) => TransferError::Declined(reason),
        })),
    }
}

/// SCP's `error` as a transfer's.
async fn scp_error(connection: &FileConnection, error: ScpError) -> TransferError {
    match error {
        ScpError::ConnectionLost => {
            let reason = tokio::time::timeout(LOST_REASON_WAIT, connection.lost())
                .await
                .unwrap_or_else(|_| Lost::Broken("the connection ended".into()));
            TransferError::Lost(reason)
        }
        ScpError::TimedOut { after } => TransferError::Stalled { after },
        ScpError::Local(reason) => TransferError::Local(reason),
        error => TransferError::Scp(error),
    }
}

async fn scp_upload(
    connect: &dyn Connect,
    local: &Path,
    remote: &str,
    progress: &Progress<'_>,
    cancel: watch::Receiver<bool>,
) -> Outcome {
    let failed = |error| Outcome::Failed {
        error,
        partial: Partial::Nothing,
    };
    let file = match tokio::fs::File::open(local).await {
        Ok(file) => file,
        Err(error) => return failed(local_error("open", local, error)),
    };
    let size = match file.metadata().await {
        Ok(metadata) => metadata.len(),
        Err(error) => return failed(local_error("read", local, error)),
    };
    let connection = match scp_connection(connect, progress, cancel.clone()).await {
        None => {
            return Outcome::Cancelled {
                partial: Partial::Nothing,
            }
        }
        Some(Err(error)) => return failed(error),
        Some(Ok(connection)) => connection,
    };
    progress.total(Some(size));
    let mut source = Reading {
        inner: file,
        hasher: Hasher::new(),
        done: 0,
        progress,
    };
    let sent = connection
        .scp_send(remote, size, &mut source, cancelled(cancel))
        .await;
    let outcome = match sent {
        Ok(()) => Outcome::Done(Done {
            bytes: size,
            hashes: Some(source.hasher.finish()),
            size: SizeCheck::NotChecked,
        }),
        Err(ScpError::Cancelled) => Outcome::Cancelled {
            partial: Partial::MayRemainOnDevice,
        },
        // Some devices create the file when the header comes.
        Err(error) => Outcome::Failed {
            error: scp_error(&connection, error).await,
            partial: Partial::MayRemainOnDevice,
        },
    };
    connection.close().await;
    outcome
}

async fn scp_download(
    connect: &dyn Connect,
    remote: &str,
    local: &Path,
    progress: &Progress<'_>,
    cancel: watch::Receiver<bool>,
) -> Outcome {
    let connection = match scp_connection(connect, progress, cancel.clone()).await {
        None => {
            return Outcome::Cancelled {
                partial: Partial::Nothing,
            }
        }
        Some(Err(error)) => {
            return Outcome::Failed {
                error,
                partial: Partial::Nothing,
            }
        }
        Some(Ok(connection)) => connection,
    };
    progress.total(None);
    let mut destination = Writing {
        path: local,
        file: None,
        done: 0,
        progress,
    };
    let fetching = connection.scp_fetch_announcing(remote, &mut destination, |size| {
        progress.total(Some(size));
    });
    let fetched = tokio::select! {
        biased;
        () = cancelled(cancel) => None,
        fetched = fetching => Some(fetched),
    };
    let created = destination.file.is_some();
    drop(destination);
    // Only a file this transfer created is removed: a refused fetch leaves
    // an existing file of that name as it was.
    let partial = || async {
        if created {
            remove_local(local).await
        } else {
            Partial::Nothing
        }
    };
    let outcome = match fetched {
        None => Outcome::Cancelled {
            partial: partial().await,
        },
        Some(Ok(bytes)) => {
            // An empty file has no first byte to create it.
            let made = if created {
                Ok(())
            } else {
                tokio::fs::File::create(local).await.map(drop)
            };
            match made {
                Ok(()) => checked_on_disk(local, bytes, bytes).await,
                Err(error) => Outcome::Failed {
                    error: local_error("create", local, error),
                    partial: Partial::Nothing,
                },
            }
        }
        Some(Err(ScpError::Short { expected, received })) => Outcome::Failed {
            error: TransferError::SizeDiffers {
                announced: expected,
                received,
            },
            partial: partial().await,
        },
        Some(Err(error)) => Outcome::Failed {
            error: scp_error(&connection, error).await,
            partial: partial().await,
        },
    };
    connection.close().await;
    outcome
}

/// The local file of an upload, hashed and counted as it is read.
struct Reading<'a, 'b, R> {
    inner: R,
    hasher: Hasher,
    done: u64,
    progress: &'a Progress<'b>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Reading<'_, '_, R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let before = buf.filled().len();
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let read = &buf.filled()[before..];
            if !read.is_empty() {
                this.hasher.update(read);
                this.done += read.len() as u64;
                this.progress.moved(this.done);
            }
        }
        polled
    }
}

/// The local file of a download, created at the first byte (so that a
/// fetch the device refuses never touches a file of that name) and counted
/// as it is written.
struct Writing<'a, 'b> {
    path: &'a Path,
    file: Option<tokio::fs::File>,
    done: u64,
    progress: &'a Progress<'b>,
}

impl Writing<'_, '_> {
    fn file(&mut self) -> io::Result<&mut tokio::fs::File> {
        if self.file.is_none() {
            let created = std::fs::File::create(self.path)?;
            self.file = Some(tokio::fs::File::from_std(created));
        }
        Ok(self.file.as_mut().expect("just created"))
    }
}

impl AsyncWrite for Writing<'_, '_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let file = match this.file() {
            Ok(file) => file,
            Err(error) => return Poll::Ready(Err(error)),
        };
        let polled = Pin::new(file).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = polled {
            if n > 0 {
                this.done += n as u64;
                this.progress.moved(this.done);
            }
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.file.as_mut() {
            Some(file) => Pin::new(file).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.file.as_mut() {
            Some(file) => Pin::new(file).poll_shutdown(cx),
            None => Poll::Ready(Ok(())),
        }
    }
}
