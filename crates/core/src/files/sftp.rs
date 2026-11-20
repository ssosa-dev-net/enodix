//! SFTP on a file connection, with `russh-sftp`.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::{File, Metadata};
use russh_sftp::client::{Config, SftpSession};
use russh_sftp::protocol::StatusCode;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{watch, OwnedSemaphorePermit};

use super::{FileConnection, Lost};
use crate::session::connection::SubsystemRefusal;

/// How many SFTP requests may wait for their answer at once, for reads and
/// for writes: about 2 MiB of writes in flight, which the lab's fastest
/// path needs to reach its line rate (#96; russh-sftp's 16 halves uploads).
const REQUESTS_IN_FLIGHT: usize = 64;

/// The size of each write request.
const WRITE_SIZE: u32 = 32 * 1024;

/// How many connect timeouts a request may wait for its answer: the
/// spec's stall rule.
const STALL_TIMEOUTS: u32 = 3;

/// Why a device has no SFTP ([`StartError::NoSftp`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NoSftp {
    /// It refused the `sftp` subsystem.
    #[error("it refused the SFTP subsystem")]
    Refused,
    /// It accepted the subsystem (or did not say), then closed the channel
    /// before SFTP's first reply, as IOS-XE does. A device that has SFTP
    /// but not for this account (cEOS below privilege 15) does the same.
    #[error("it closed the SFTP channel before answering")]
    ClosedBeforeReply,
}

/// Why SFTP did not start on a file connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// The device does not offer SFTP (to this account, at least).
    #[error("the device does not offer SFTP: {0}")]
    NoSftp(NoSftp),
    /// The connection failed: it was lost, or the device did not answer.
    #[error("{0}")]
    Connection(String),
    /// The connection carries a channel already (SFTP still open).
    #[error("the connection already carries a channel; close it first")]
    Busy,
}

/// Why an SFTP operation failed, in the app's words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    #[error("no such file or folder: {path}")]
    NotFound { path: String },
    #[error("permission denied: {path}")]
    PermissionDenied { path: String },
    /// A file or folder of that name is there already (making a folder,
    /// renaming onto it: nothing was replaced).
    #[error("{path} exists already")]
    AlreadyExists { path: String },
    #[error("the folder {path} is not empty")]
    NotEmpty { path: String },
    #[error("the connection was lost")]
    ConnectionLost,
    /// The device did not answer a request within the stall limit.
    #[error("the device did not answer within {} s", .after.as_secs())]
    TimedOut { after: Duration },
    /// The device ended SFTP while the connection stayed up.
    #[error("the device ended the SFTP session")]
    SessionEnded,
    /// The device refused for a reason SFTP has no word for; `message` is
    /// what it said, if it said more than its bare code.
    #[error("the device could not {action}{}", detail(.message))]
    Refused { action: String, message: String },
}

fn detail(message: &str) -> String {
    let message = message.trim();
    let bare = ["", "failure", "ok", "bad message", "operation unsupported"];
    if bare.contains(&message.to_lowercase().as_str()) {
        String::new()
    } else {
        format!(" ({message})")
    }
}

/// What kind of thing a [`RemoteEntry`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
    /// A symbolic link (listed as itself, not as what it points to).
    Link,
    /// Anything else, or a device that did not say.
    Other,
}

/// A file or folder on the device, as a listing or [`Sftp::stat`] gives
/// it. A device may leave any of the details out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// Its name, without its folder.
    pub name: String,
    pub kind: EntryKind,
    /// In bytes.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// As `ls` shows them: `rwxr-xr-x`.
    pub permissions: Option<String>,
}

impl RemoteEntry {
    fn new(name: String, metadata: &Metadata) -> Self {
        let kind = match metadata.permissions.map(|mode| mode & 0o170_000) {
            Some(0o040_000) => EntryKind::Folder,
            Some(0o100_000) => EntryKind::File,
            Some(0o120_000) => EntryKind::Link,
            _ => EntryKind::Other,
        };
        Self {
            name,
            kind,
            size: metadata.size,
            modified: metadata
                .mtime
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(u64::from(seconds))),
            permissions: metadata.permissions.map(permission_text),
        }
    }
}

/// `mode`'s permission bits as `ls` shows them.
fn permission_text(mode: u32) -> String {
    let mut text = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        text.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        text.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        text.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    text
}

/// The last part of `path`, for the name of what it names.
fn last_part(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => path.to_owned(),
    }
}

/// SFTP on a file connection: the device's file system. Dropping it (and
/// its streams) ends SFTP and frees the connection's channel; dropping the
/// connection ends it too, and its operations then fail with
/// [`FileError::ConnectionLost`].
pub struct Sftp {
    shared: Arc<Shared>,
}

/// What an SFTP session and its streams share.
struct Shared {
    session: SftpSession,
    ended: watch::Receiver<Option<Lost>>,
    timeout: Duration,
    /// The connection's channel, held while SFTP is open.
    _channel: OwnedSemaphorePermit,
}

impl FileConnection {
    /// Starts SFTP on the connection. Tells apart a device without SFTP
    /// (it refused the subsystem, or closed the channel before SFTP's first
    /// reply) from a connection that failed. Each request then waits at
    /// most three connect timeouts for its answer.
    pub async fn start_sftp(&self) -> Result<Sftp, StartError> {
        let permit = self.channel_permit().ok_or(StartError::Busy)?;
        let lost = || StartError::Connection(FileError::ConnectionLost.to_string());
        if !self.is_open() {
            return Err(lost());
        }
        let channel = self
            .subsystem("sftp")
            .await
            .map_err(|refusal| match refusal {
                _ if !self.is_open() => lost(),
                SubsystemRefusal::Refused => StartError::NoSftp(NoSftp::Refused),
                SubsystemRefusal::Closed => StartError::NoSftp(NoSftp::ClosedBeforeReply),
                SubsystemRefusal::Failed(reason) => StartError::Connection(reason),
            })?;
        let timeout = self.connect_timeout() * STALL_TIMEOUTS;
        let config = Config {
            max_concurrent_reads: REQUESTS_IN_FLIGHT,
            max_concurrent_writes: REQUESTS_IN_FLIGHT,
            max_write_packet_len: WRITE_SIZE,
            request_timeout_secs: timeout.as_secs_f64().ceil().max(1.0) as u64,
            ..Config::default()
        };
        let answered = Arc::new(AtomicBool::new(false));
        let stream = Answered {
            inner: channel.into_stream(),
            answered: answered.clone(),
        };
        match SftpSession::new_with_config(stream, config).await {
            Ok(session) => Ok(Sftp {
                shared: Arc::new(Shared {
                    session,
                    ended: self.ended(),
                    timeout,
                    _channel: permit,
                }),
            }),
            Err(_) if !self.is_open() => Err(lost()),
            Err(_) if !answered.load(Ordering::Acquire) => {
                Err(StartError::NoSftp(NoSftp::ClosedBeforeReply))
            }
            Err(SftpError::Timeout) => Err(StartError::Connection(
                FileError::TimedOut { after: timeout }.to_string(),
            )),
            Err(error) => Err(StartError::Connection(format!(
                "SFTP did not start ({error})"
            ))),
        }
    }
}

impl Shared {
    /// Whether the connection has ended.
    fn lost(&self) -> bool {
        self.ended.borrow().is_some() || self.ended.has_changed().is_err()
    }

    /// `error` in the app's words, for an operation on `path` that `action`
    /// describes ("delete the file /x").
    fn failed(&self, error: SftpError, path: &str, action: impl FnOnce() -> String) -> FileError {
        match error {
            SftpError::Status(status) => match status.status_code {
                StatusCode::NoSuchFile => FileError::NotFound { path: path.into() },
                StatusCode::PermissionDenied => FileError::PermissionDenied { path: path.into() },
                StatusCode::NoConnection | StatusCode::ConnectionLost => FileError::ConnectionLost,
                _ => FileError::Refused {
                    action: action(),
                    message: status.error_message,
                },
            },
            _ if self.lost() => FileError::ConnectionLost,
            SftpError::Timeout => FileError::TimedOut {
                after: self.timeout,
            },
            _ => FileError::SessionEnded,
        }
    }

    /// An error of a stream (the file at `path`) in the app's words.
    fn stream_failed(&self, error: io::Error, path: &str, action: &str) -> io::Error {
        let error = match error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<SftpError>())
        {
            Some(sftp) => self.failed(sftp.clone(), path, || format!("{action} {path}")),
            None if self.lost() => FileError::ConnectionLost,
            None if error.kind() == io::ErrorKind::TimedOut => FileError::TimedOut {
                after: self.timeout,
            },
            None => FileError::Refused {
                action: format!("{action} {path}"),
                message: error.to_string(),
            },
        };
        io::Error::other(error)
    }
}

/// A stream's `io::Error` as the [`FileError`] it carries.
fn file_error(error: io::Error) -> FileError {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<FileError>())
    {
        Some(error) => error.clone(),
        None => FileError::Refused {
            action: "finish the transfer".into(),
            message: error.to_string(),
        },
    }
}

impl Sftp {
    /// The folder the login lands in, as an absolute path.
    pub async fn home(&self) -> Result<String, FileError> {
        let shared = &self.shared;
        shared
            .session
            .canonicalize(".")
            .await
            .map_err(|error| shared.failed(error, ".", || "tell its home folder".into()))
    }

    /// What is in the folder `path`: files, folders and links, hidden ones
    /// included, without `.` and `..`, in the device's order.
    pub async fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, FileError> {
        let shared = &self.shared;
        let entries = shared
            .session
            .read_dir(path)
            .await
            .map_err(|error| shared.failed(error, path, || format!("list {path}")))?;
        Ok(entries
            .filter(|entry| !matches!(entry.file_name().as_str(), "." | ".."))
            .map(|entry| RemoteEntry::new(entry.file_name(), &entry.metadata()))
            .collect())
    }

    /// The file or folder `path` (what a link points to, for a link).
    pub async fn stat(&self, path: &str) -> Result<RemoteEntry, FileError> {
        let shared = &self.shared;
        let metadata = shared.session.metadata(path).await.map_err(|error| {
            shared.failed(error, path, || format!("read the details of {path}"))
        })?;
        Ok(RemoteEntry::new(last_part(path), &metadata))
    }

    /// Whether `path` exists (for telling what a bare failure meant).
    async fn exists(&self, path: &str) -> bool {
        self.shared.session.symlink_metadata(path).await.is_ok()
    }

    /// Makes the folder `path`.
    pub async fn make_folder(&self, path: &str) -> Result<(), FileError> {
        let shared = &self.shared;
        let made = shared.session.create_dir(path).await;
        match made {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = shared.failed(error, path, || format!("make the folder {path}"));
                if matches!(error, FileError::Refused { .. }) && self.exists(path).await {
                    return Err(FileError::AlreadyExists { path: path.into() });
                }
                Err(error)
            }
        }
    }

    /// Renames (or moves) `from` to `to`. A file or folder already called
    /// `to` is [`FileError::AlreadyExists`], and nothing changes: SFTP has
    /// no replacing rename on every device.
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), FileError> {
        let shared = &self.shared;
        match shared.session.rename(from, to).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = shared.failed(error, from, || format!("rename {from} to {to}"));
                if matches!(error, FileError::Refused { .. }) && self.exists(to).await {
                    return Err(FileError::AlreadyExists { path: to.into() });
                }
                Err(error)
            }
        }
    }

    /// Deletes the file `path`.
    pub async fn delete_file(&self, path: &str) -> Result<(), FileError> {
        let shared = &self.shared;
        shared
            .session
            .remove_file(path)
            .await
            .map_err(|error| shared.failed(error, path, || format!("delete the file {path}")))
    }

    /// Deletes the folder `path`, which must be empty
    /// ([`FileError::NotEmpty`] otherwise).
    pub async fn delete_folder(&self, path: &str) -> Result<(), FileError> {
        let shared = &self.shared;
        match shared.session.remove_dir(path).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = shared.failed(error, path, || format!("delete the folder {path}"));
                if matches!(error, FileError::Refused { .. })
                    && self
                        .list(path)
                        .await
                        .is_ok_and(|entries| !entries.is_empty())
                {
                    return Err(FileError::NotEmpty { path: path.into() });
                }
                Err(error)
            }
        }
    }

    /// Opens the file `path` for reading, as a stream.
    pub async fn open_read(&self, path: &str) -> Result<RemoteReader, FileError> {
        let shared = &self.shared;
        let file = shared
            .session
            .open(path)
            .await
            .map_err(|error| shared.failed(error, path, || format!("open {path}")))?;
        Ok(RemoteReader {
            file,
            path: path.into(),
            shared: shared.clone(),
        })
    }

    /// Creates the file `path` (emptying one of that name), for writing, as
    /// a stream. [`RemoteWriter::finish`] waits for the device to have
    /// taken everything written.
    pub async fn create(&self, path: &str) -> Result<RemoteWriter, FileError> {
        let shared = &self.shared;
        let file = shared
            .session
            .create(path)
            .await
            .map_err(|error| shared.failed(error, path, || format!("create {path}")))?;
        Ok(RemoteWriter {
            file,
            path: path.into(),
            shared: shared.clone(),
        })
    }
}

/// A file on the device, being read. Its errors carry a [`FileError`]
/// (`io::Error::get_ref`).
pub struct RemoteReader {
    file: File,
    path: String,
    shared: Arc<Shared>,
}

impl AsyncRead for RemoteReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        Pin::new(&mut this.file)
            .poll_read(cx, buf)
            .map_err(|error| this.shared.stream_failed(error, &this.path, "read"))
    }
}

/// A file on the device, being written. Its errors carry a [`FileError`]
/// (`io::Error::get_ref`).
pub struct RemoteWriter {
    file: File,
    path: String,
    shared: Arc<Shared>,
}

impl RemoteWriter {
    /// Waits until the device has taken everything written, and closes
    /// the file. Dropping the writer instead closes it without waiting.
    pub async fn finish(mut self) -> Result<(), FileError> {
        self.shutdown().await.map_err(file_error)
    }
}

impl AsyncWrite for RemoteWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        Pin::new(&mut this.file)
            .poll_write(cx, buf)
            .map_err(|error| this.shared.stream_failed(error, &this.path, "write"))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        Pin::new(&mut this.file)
            .poll_flush(cx)
            .map_err(|error| this.shared.stream_failed(error, &this.path, "write"))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        Pin::new(&mut this.file)
            .poll_shutdown(cx)
            .map_err(|error| this.shared.stream_failed(error, &this.path, "write"))
    }
}

/// The SFTP channel's stream, noting whether the device sent anything:
/// a channel closed before a byte came back means no SFTP.
struct Answered<S> {
    inner: S,
    answered: Arc<AtomicBool>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Answered<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            self.answered.store(true, Ordering::Release);
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Answered<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
