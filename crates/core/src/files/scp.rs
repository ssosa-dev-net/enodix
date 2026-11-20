//! SCP on a file connection, for devices that have no SFTP, and the fall
//! back to it.
//!
//! No library speaks SCP; this is the least of it: one file per transfer,
//! no folders, no timestamps. `scp -t <path>` sends, `scp -f <path>`
//! fetches, over an exec channel: a `C0644 <size> <name>` header, the data,
//! then a zero byte and its acknowledgement. Answers are read the way
//! devices give them (#96, point 5):
//!
//! - a zero byte is success; `\x01` or `\x02` and a line of text is the
//!   device's error;
//! - anything else is an error text too (cEOS answers plain `% ...` lines
//!   with no error byte);
//! - a channel that closes without an answer is explained by what came on
//!   stderr and its exit status (`sh: scp: not found`, exit 127);
//! - a fetch's header is not compared with the path asked for: IOS names
//!   the file without its folder (`ten.bin` for `bootflash:ten.bin`).
//!
//! Paths are the device's own (`flash:name`, `/var/tmp/name`) and passed on
//! as given.

use std::future::Future;
use std::time::Duration;

use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

use super::sftp::STALL_TIMEOUTS;
use super::{
    ConnectError, FileConnection, FileError, Lost, NoSftp, Opening, RemoteEntry, Sftp, StartError,
};
use crate::known_hosts::KnownHosts;
use crate::platform::PlatformDefinition;
use crate::session::SshTarget;

/// The hint for IOS answering `Administratively disabled.`.
const IOS_SCP_OFF: &str = "SCP is turned off on the device: turn it on with ip scp server enable";

/// The hint for a Junos host that fell back to SCP.
const JUNOS_NO_SFTP: &str =
    "for a listing, turn on the SFTP server: set system services ssh sftp-server";

/// How long a finished transfer waits for the device to close its channel.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The size of each piece of an upload.
const SEND_SIZE: usize = 32 * 1024;

/// How a device's files are reached ([`Files::protocol`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Protocol {
    /// SFTP, on one connection kept open: browse and transfer.
    Sftp,
    /// SCP, a connection per transfer: transfer only.
    Scp,
}

/// Why an SCP transfer failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScpError {
    /// The device refused, in its own words (`message`); the app's hint,
    /// when it knows one, is apart from them ([`ScpError::hint`]).
    #[error("the device refused: {message}")]
    Refused {
        message: String,
        hint: Option<&'static str>,
    },
    /// A fetch ended before the size its header announced.
    #[error("the device sent {received} of the {expected} bytes it announced")]
    Short { expected: u64, received: u64 },
    /// The send was cancelled and its channel closed. SCP cannot remove
    /// what the device kept.
    #[error("the transfer was cancelled; a partial file may remain on the device")]
    Cancelled,
    #[error("the connection was lost")]
    ConnectionLost,
    /// No byte moved within the stall limit (three connect timeouts).
    #[error("the device did not answer within {} s", .after.as_secs())]
    TimedOut { after: Duration },
    /// The channel could not be opened (the device refused it, or did not
    /// answer).
    #[error("{0}")]
    Connection(String),
    /// Connecting for the transfer failed.
    #[error("{0}")]
    Connect(ConnectError),
    /// The local file could not be read or written.
    #[error("{0}")]
    Local(String),
    /// The connection carries a channel already.
    #[error("the connection already carries a channel; close it first")]
    Busy,
}

impl ScpError {
    /// What the app knows to do about the refusal, apart from the device's
    /// words: only for IOS's `Administratively disabled.`.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Refused { hint, .. } => *hint,
            _ => None,
        }
    }
}

/// The device's words, with the hint the app has for them.
fn refused(message: String) -> ScpError {
    let hint = message
        .contains("Administratively disabled")
        .then_some(IOS_SCP_OFF);
    ScpError::Refused { message, hint }
}

/// What comes back on an SCP channel, read as the protocol reads it.
struct Answers {
    read: ChannelReadHalf,
    pending: Vec<u8>,
    stderr: Vec<u8>,
    exit: Option<u32>,
    /// Why the channel ended, once it has.
    ended: Option<ScpError>,
    connection: watch::Receiver<Option<Lost>>,
    timeout: Duration,
}

impl Answers {
    /// Waits for more data on the channel. Once it has closed, the error
    /// says why: what was left unread, stderr and the exit status.
    async fn fill(&mut self) -> Result<(), ScpError> {
        if let Some(ended) = &self.ended {
            return Err(ended.clone());
        }
        loop {
            let message = tokio::time::timeout(self.timeout, self.read.wait())
                .await
                .map_err(|_| ScpError::TimedOut {
                    after: self.timeout,
                })?;
            match message {
                Some(ChannelMsg::Data { data }) => {
                    self.pending.extend_from_slice(&data);
                    return Ok(());
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => self.stderr.extend_from_slice(&data),
                Some(ChannelMsg::ExitStatus { exit_status }) => self.exit = Some(exit_status),
                Some(ChannelMsg::Failure) => {
                    return Err(self.end(refused("it would not run scp".into())));
                }
                Some(ChannelMsg::Close) | None => {
                    let reason = self.reason();
                    return Err(self.end(reason));
                }
                Some(_) => {}
            }
        }
    }

    fn end(&mut self, error: ScpError) -> ScpError {
        self.ended = Some(error.clone());
        error
    }

    /// Why the channel closed: the connection went, or the device's words.
    fn reason(&self) -> ScpError {
        let lost = self.connection.borrow().is_some() || self.connection.has_changed().is_err();
        if lost {
            return ScpError::ConnectionLost;
        }
        let words: Vec<String> = [&self.pending, &self.stderr]
            .into_iter()
            .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
            .filter(|text| !text.is_empty())
            .collect();
        let status = self
            .exit
            .map(|status| format!("exit status {status}"))
            .unwrap_or_default();
        let message = match (words.join("; "), status) {
            (words, status) if words.is_empty() && status.is_empty() => {
                "it closed the channel without answering".to_owned()
            }
            (words, status) if words.is_empty() => format!("it closed the channel ({status})"),
            (words, status) if status.is_empty() => words,
            (words, status) => format!("{words} ({status})"),
        };
        refused(message)
    }

    async fn byte(&mut self) -> Result<u8, ScpError> {
        while self.pending.is_empty() {
            self.fill().await?;
        }
        Ok(self.pending.remove(0))
    }

    /// The next line (without its end), or what was left when the channel
    /// closed.
    async fn line(&mut self) -> Result<String, ScpError> {
        loop {
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Ok(String::from_utf8_lossy(&line[..end]).trim().to_owned());
            }
            if let Err(ended) = self.fill().await {
                if self.pending.is_empty() {
                    return Err(ended);
                }
                let line = std::mem::take(&mut self.pending);
                return Ok(String::from_utf8_lossy(&line).trim().to_owned());
            }
        }
    }

    /// One acknowledgement: a zero byte, or the device's error.
    async fn ack(&mut self) -> Result<(), ScpError> {
        match self.byte().await? {
            0 => Ok(()),
            1 | 2 => Err(refused(self.line().await?)),
            other => {
                self.pending.insert(0, other);
                Err(refused(self.line().await?))
            }
        }
    }

    /// After a transfer that went through: gives the device a moment to
    /// close the channel, so the next transfer finds it gone. A device that
    /// keeps it open does not hold the transfer up; dropping the channel
    /// closes it from this side.
    async fn finish(&mut self) {
        let closing = async {
            while self.fill().await.is_ok() {
                self.pending.clear();
            }
        };
        let _ = tokio::time::timeout(CLOSE_WAIT, closing).await;
    }
}

/// The last part of `path`, for the header's name.
fn name_of(path: &str) -> &str {
    match path.trim_end_matches('/').rsplit(['/', ':']).next() {
        Some(name) if !name.is_empty() => name,
        _ => "file",
    }
}

impl FileConnection {
    /// An exec channel running `command`, its answers and its writer.
    async fn scp_channel(
        &self,
        command: &str,
    ) -> Result<(Answers, ChannelWriteHalf<russh::client::Msg>), ScpError> {
        if !self.is_open() {
            return Err(ScpError::ConnectionLost);
        }
        let channel = self.exec(command).await.map_err(|reason| {
            if self.is_open() {
                ScpError::Connection(reason)
            } else {
                ScpError::ConnectionLost
            }
        })?;
        let (read, write) = channel.split();
        let answers = Answers {
            read,
            pending: Vec::new(),
            stderr: Vec::new(),
            exit: None,
            ended: None,
            connection: self.ended(),
            timeout: self.connect_timeout() * STALL_TIMEOUTS,
        };
        Ok((answers, write))
    }

    /// Sends `size` bytes from `source` to the file `path` on the device
    /// over SCP (`scp -t`). When `cancelled` resolves first, the channel is
    /// closed and the send is [`ScpError::Cancelled`]: SCP cannot remove the
    /// partial file the device may keep. SCP cannot check the size the
    /// device wrote.
    ///
    /// Some devices (IOS-XE) close the whole connection once a transfer
    /// ends: [`Scp`] connects for each one.
    pub async fn scp_send(
        &self,
        path: &str,
        size: u64,
        mut source: impl AsyncRead + Unpin,
        cancelled: impl Future<Output = ()>,
    ) -> Result<(), ScpError> {
        let _channel = self.channel_permit().ok_or(ScpError::Busy)?;
        let (mut answers, write) = self.scp_channel(&format!("scp -t {path}")).await?;
        let timeout = answers.timeout;
        let write = &write;
        let send = |bytes: Vec<u8>| put(write, timeout, bytes);
        let sending = async {
            answers.ack().await?;
            let header = format!("C0644 {size} {}\n", name_of(path)).into_bytes();
            if send(header).await.is_err() {
                return Err(why_not_written(&mut answers).await);
            }
            answers.ack().await?;
            let mut buffer = vec![0; SEND_SIZE];
            let mut sent = 0u64;
            while sent < size {
                let want = buffer.len().min((size - sent) as usize);
                let n = source
                    .read(&mut buffer[..want])
                    .await
                    .map_err(|error| ScpError::Local(error.to_string()))?;
                if n == 0 {
                    return Err(ScpError::Local(format!(
                        "the local file ended after {sent} of its {size} bytes"
                    )));
                }
                if send(buffer[..n].to_vec()).await.is_err() {
                    return Err(why_not_written(&mut answers).await);
                }
                sent += n as u64;
            }
            if send(vec![0]).await.is_err() {
                return Err(why_not_written(&mut answers).await);
            }
            answers.ack().await?;
            let _ = write.eof().await;
            answers.finish().await;
            Ok(())
        };
        tokio::select! {
            biased;
            () = cancelled => {
                let _ = write.close().await;
                Err(ScpError::Cancelled)
            }
            sent = sending => sent,
        }
    }

    /// Fetches the file `path` from the device over SCP (`scp -f`) into
    /// `destination`, and gives its size. A fetch that ends before the
    /// size its header announced is [`ScpError::Short`].
    pub async fn scp_fetch(
        &self,
        path: &str,
        mut destination: impl AsyncWrite + Unpin,
    ) -> Result<u64, ScpError> {
        let _channel = self.channel_permit().ok_or(ScpError::Busy)?;
        let (mut answers, write) = self.scp_channel(&format!("scp -f {path}")).await?;
        let timeout = answers.timeout;
        let send = |bytes: Vec<u8>| put(&write, timeout, bytes);
        if send(vec![0]).await.is_err() {
            return Err(why_not_written(&mut answers).await);
        }
        let size = loop {
            match answers.byte().await? {
                b'C' => {
                    let header = answers.line().await?;
                    // "0644 <size> <name>"; the name is not checked.
                    match header.split_whitespace().nth(1).map(str::parse::<u64>) {
                        Some(Ok(size)) => break size,
                        _ => return Err(refused(format!("C{header}"))),
                    }
                }
                // Times, which were not asked for: taken and passed.
                b'T' => {
                    answers.line().await?;
                    if send(vec![0]).await.is_err() {
                        return Err(why_not_written(&mut answers).await);
                    }
                }
                b'D' => {
                    return Err(refused(format!(
                        "{path} is a folder, and SCP here fetches one file"
                    )))
                }
                1 | 2 => return Err(refused(answers.line().await?)),
                other => {
                    answers.pending.insert(0, other);
                    return Err(refused(answers.line().await?));
                }
            }
        };
        if send(vec![0]).await.is_err() {
            return Err(why_not_written(&mut answers).await);
        }
        let mut received = 0u64;
        while received < size {
            if answers.pending.is_empty() {
                match answers.fill().await {
                    Ok(()) => {}
                    Err(ScpError::Refused { .. }) => {
                        return Err(ScpError::Short {
                            expected: size,
                            received,
                        })
                    }
                    Err(error) => return Err(error),
                }
            }
            let take = answers.pending.len().min((size - received) as usize);
            destination
                .write_all(&answers.pending[..take])
                .await
                .map_err(|error| ScpError::Local(error.to_string()))?;
            answers.pending.drain(..take);
            received += take as u64;
        }
        destination
            .flush()
            .await
            .map_err(|error| ScpError::Local(error.to_string()))?;
        answers.ack().await?;
        if send(vec![0]).await.is_ok() {
            let _ = write.eof().await;
        }
        answers.finish().await;
        Ok(size)
    }

    /// SFTP on the connection, or, when the device has no SFTP
    /// ([`StartError::NoSftp`]), SCP: the connection is then closed, since
    /// SCP connects for each transfer (IOS-XE closes the connection after
    /// one, and an idle connection holds one of its few lines). A host
    /// whose `platform` is Junos gets a hint for turning SFTP on.
    ///
    /// After a fall back, what SCP answers is the device's own: a device
    /// with SFTP but not for this account (cEOS at privilege 1) falls back
    /// too, and then refuses SCP in its own words.
    pub async fn start_files(
        self,
        platform: Option<&PlatformDefinition>,
    ) -> Result<Files, StartError> {
        match self.start_sftp().await {
            Ok(sftp) => Ok(Files::Sftp {
                connection: self,
                sftp,
            }),
            Err(StartError::NoSftp(no_sftp)) => {
                let junos = platform.is_some_and(|p| p.is_named("junos") || p.answers_to("junos"));
                let (target, known_hosts) = self.opened_with();
                let scp = Scp {
                    target: target.clone(),
                    known_hosts: known_hosts.clone(),
                    no_sftp,
                    hint: junos.then_some(JUNOS_NO_SFTP),
                };
                self.close().await;
                Ok(Files::Scp(scp))
            }
            Err(error) => Err(error),
        }
    }
}

/// Writes `bytes` on the channel, within the stall limit: `Err` when the
/// channel is gone or the device took nothing for that long.
async fn put(
    write: &ChannelWriteHalf<russh::client::Msg>,
    timeout: Duration,
    bytes: Vec<u8>,
) -> Result<(), ()> {
    match tokio::time::timeout(timeout, write.data_bytes(bytes)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

/// Why a write on the channel failed: the device's answer, if it gave one
/// before closing, or how it closed.
async fn why_not_written(answers: &mut Answers) -> ScpError {
    match answers.ack().await {
        Err(error) => error,
        Ok(()) => ScpError::ConnectionLost,
    }
}

/// A device reached over SCP because it has no SFTP: a connection per
/// transfer, nothing held between them. It can send and fetch one file at
/// a time and nothing else.
pub struct Scp {
    target: SshTarget,
    known_hosts: KnownHosts,
    no_sftp: NoSftp,
    hint: Option<&'static str>,
}

impl Scp {
    /// Why SFTP was not used.
    pub fn no_sftp(&self) -> NoSftp {
        self.no_sftp
    }

    /// The app's hint for getting a listing (Junos: turn on its SFTP
    /// server), apart from anything the device says.
    pub fn hint(&self) -> Option<&'static str> {
        self.hint
    }

    /// Opens a connection for one transfer, attended: a typed login is
    /// asked again each time.
    pub fn connect(&self) -> Opening {
        FileConnection::open(self.target.clone(), self.known_hosts.clone())
    }

    /// Opens a connection for one transfer with nobody to ask.
    pub async fn connect_unattended(&self) -> Result<FileConnection, ConnectError> {
        FileConnection::open_unattended(self.target.clone(), self.known_hosts.clone()).await
    }

    /// [`FileConnection::scp_send`] on a connection of its own, opened
    /// unattended and closed afterwards.
    pub async fn send(
        &self,
        path: &str,
        size: u64,
        source: impl AsyncRead + Unpin,
        cancelled: impl Future<Output = ()>,
    ) -> Result<(), ScpError> {
        let connection = self.connect_unattended().await.map_err(ScpError::Connect)?;
        let sent = connection.scp_send(path, size, source, cancelled).await;
        connection.close().await;
        sent
    }

    /// [`FileConnection::scp_fetch`] on a connection of its own, opened
    /// unattended and closed afterwards.
    pub async fn fetch(
        &self,
        path: &str,
        destination: impl AsyncWrite + Unpin,
    ) -> Result<u64, ScpError> {
        let connection = self.connect_unattended().await.map_err(ScpError::Connect)?;
        let fetched = connection.scp_fetch(path, destination).await;
        connection.close().await;
        fetched
    }
}

impl std::fmt::Debug for Scp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scp")
            .field("host", &self.target.host)
            .field("no_sftp", &self.no_sftp)
            .finish_non_exhaustive()
    }
}

/// A device's files, over the protocol it offers
/// ([`FileConnection::start_files`]).
pub enum Files {
    /// SFTP on the connection, which stays open.
    Sftp {
        connection: FileConnection,
        sftp: Sftp,
    },
    /// No SFTP: SCP, transfer only.
    Scp(Scp),
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sftp { connection, .. } => f.debug_tuple("Sftp").field(connection).finish(),
            Self::Scp(scp) => f.debug_tuple("Scp").field(scp).finish(),
        }
    }
}

/// What SCP cannot do.
fn transfer_only(action: impl Into<String>) -> FileError {
    FileError::TransferOnly {
        action: action.into(),
    }
}

impl Files {
    pub fn protocol(&self) -> Protocol {
        match self {
            Self::Sftp { .. } => Protocol::Sftp,
            Self::Scp(_) => Protocol::Scp,
        }
    }

    /// [`Sftp::list`]; refused over SCP.
    pub async fn list(&self, folder: &str) -> Result<Vec<RemoteEntry>, FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.list(folder).await,
            Self::Scp(_) => Err(transfer_only("list a folder")),
        }
    }

    /// [`Sftp::stat`]; refused over SCP (it cannot tell whether a file is
    /// there, nor its size).
    pub async fn stat(&self, path: &str) -> Result<RemoteEntry, FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.stat(path).await,
            Self::Scp(_) => Err(transfer_only("read the details of a file")),
        }
    }

    /// [`Sftp::rename`]; refused over SCP.
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.rename(from, to).await,
            Self::Scp(_) => Err(transfer_only("rename a file")),
        }
    }

    /// [`Sftp::delete_file`]; refused over SCP (a partial upload cannot be
    /// removed either).
    pub async fn delete_file(&self, path: &str) -> Result<(), FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.delete_file(path).await,
            Self::Scp(_) => Err(transfer_only("delete a file")),
        }
    }

    /// [`Sftp::delete_folder`]; refused over SCP.
    pub async fn delete_folder(&self, path: &str) -> Result<(), FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.delete_folder(path).await,
            Self::Scp(_) => Err(transfer_only("delete a folder")),
        }
    }

    /// [`Sftp::make_folder`]; refused over SCP.
    pub async fn make_folder(&self, path: &str) -> Result<(), FileError> {
        match self {
            Self::Sftp { sftp, .. } => sftp.make_folder(path).await,
            Self::Scp(_) => Err(transfer_only("make a folder")),
        }
    }
}
