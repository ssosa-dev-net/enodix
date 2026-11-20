//! The test SSH server's SCP side: `scp -t <path>` (the client sends) and
//! `scp -f <path>` (the client fetches) over an exec channel, on a folder on
//! disk, and the ways real devices answer it (#96, point 5).
//!
//! Paths are absolute from the served folder (`/x`), or IOS-style with a
//! file system's name in front (`flash:x`, `bootflash:x`), which names the
//! same folder. A fetched file's header names it without its folder, as
//! IOS does (`x` for `flash:x`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use russh::server::Msg;
use russh::{Channel, ChannelMsg, Disconnect};

/// How a device answers SCP besides speaking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScpQuirk {
    /// IOS before `ip scp server enable`: `\x01Administratively disabled.`
    AdministrativelyDisabled,
    /// cEOS below privilege 15: a plain `% ...` line, no error byte.
    PlainText,
    /// No `scp` program: `sh: scp: not found` on stderr, exit status 127,
    /// no SCP answer at all (FRR).
    NoProgram,
    /// The whole connection is closed when a transfer ends (IOS-XE).
    CloseConnectionAfter,
    /// A send takes all the data, then answers `\x01Write failed` (IOS
    /// sending into a missing folder).
    WriteFailedAfterData,
    /// A fetch announces the file's size, sends half of it and ends.
    ShortFetch,
    /// Half way through a file it stops: a send is no longer read, a fetch
    /// no longer sent, and the channel stays open (a device that stopped
    /// answering).
    StallMidway,
}

/// What the server's SCP does.
#[derive(Debug, Clone, Default)]
pub struct ScpMode {
    /// The folder served; `None`: SCP is not offered (exec refused).
    pub root: Option<PathBuf>,
    pub quirks: Vec<ScpQuirk>,
}

/// The plain text cEOS answers at privilege 1.
pub const PLAIN_TEXT_REFUSAL: &str = "% The 'scp' command is only available in privileged mode.";

/// What stderr says on a host without the `scp` program.
pub const NOT_FOUND: &str = "sh: scp: not found";

/// How many SCP transfers the server is running now.
pub type Running = Arc<AtomicUsize>;

/// `scp -t <path>` or `scp -f <path>`: sending (to the server) or not.
pub fn parse(command: &str) -> Option<(bool, String)> {
    let mut words = command.split_whitespace();
    if words.next() != Some("scp") {
        return None;
    }
    let mut to_server = None;
    let mut path = None;
    for word in words {
        match word {
            "-t" => to_server = Some(true),
            "-f" => to_server = Some(false),
            flag if flag.starts_with('-') => {}
            other => path = Some(other.to_owned()),
        }
    }
    Some((to_server?, path?))
}

/// The path on disk of the device's `path`.
fn local(root: &Path, path: &str) -> PathBuf {
    let path = match path.split_once(':') {
        Some((_, rest)) if !path.starts_with('/') => rest,
        _ => path,
    };
    let mut local = root.to_path_buf();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if local != root {
                    local.pop();
                }
            }
            part => local.push(part),
        }
    }
    local
}

/// The channel's side of the server: what came in, unread.
struct Link {
    channel: Channel<Msg>,
    pending: Vec<u8>,
    closed: bool,
}

impl Link {
    /// More data, or `false` once the client has sent EOF or closed.
    async fn fill(&mut self) -> bool {
        if self.closed {
            return false;
        }
        loop {
            match self.channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    self.pending.extend_from_slice(&data);
                    return true;
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                    self.closed = true;
                    return false;
                }
                Some(_) => {}
            }
        }
    }

    async fn byte(&mut self) -> Option<u8> {
        while self.pending.is_empty() {
            if !self.fill().await {
                return None;
            }
        }
        Some(self.pending.remove(0))
    }

    async fn line(&mut self) -> Option<String> {
        loop {
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Some(String::from_utf8_lossy(&line[..end]).into_owned());
            }
            if !self.fill().await {
                return None;
            }
        }
    }

    async fn send(&self, bytes: &[u8]) {
        let _ = self.channel.data_bytes(bytes.to_vec()).await;
    }

    /// Ends the channel with `status`.
    async fn end(&self, status: u32) {
        let _ = self.channel.exit_status(status).await;
        let _ = self.channel.eof().await;
        let _ = self.channel.close().await;
    }

    /// Ends the channel with an SCP error line.
    async fn refuse(&self, text: &str) {
        self.send(format!("\x01{text}\n").as_bytes()).await;
        self.end(1).await;
    }
}

/// Serves one SCP command on `channel`; `connection` is the connection's
/// handle (to close it whole, as IOS does).
pub async fn serve(
    channel: Channel<Msg>,
    to_server: bool,
    path: String,
    mode: ScpMode,
    connection: russh::server::Handle,
    running: Running,
) {
    running.fetch_add(1, Ordering::SeqCst);
    let mut link = Link {
        channel,
        pending: Vec::new(),
        closed: false,
    };
    let quirk = |quirk| mode.quirks.contains(&quirk);
    let root = mode.root.clone().unwrap_or_default();
    if quirk(ScpQuirk::NoProgram) {
        let _ = link
            .channel
            .extended_data_bytes(1, format!("{NOT_FOUND}\n").into_bytes())
            .await;
        link.end(127).await;
    } else if quirk(ScpQuirk::AdministrativelyDisabled) {
        link.refuse("Administratively disabled.").await;
    } else if quirk(ScpQuirk::PlainText) {
        link.send(format!("{PLAIN_TEXT_REFUSAL}\r\n").as_bytes())
            .await;
        link.end(1).await;
    } else {
        let finished = if to_server {
            receive(
                &mut link,
                &local(&root, &path),
                quirk(ScpQuirk::WriteFailedAfterData),
                quirk(ScpQuirk::StallMidway),
            )
            .await
        } else {
            send(
                &mut link,
                &path,
                &local(&root, &path),
                quirk(ScpQuirk::ShortFetch),
                quirk(ScpQuirk::StallMidway),
            )
            .await
        };
        if finished && quirk(ScpQuirk::CloseConnectionAfter) {
            let _ = connection
                .disconnect(Disconnect::ByApplication, "".into(), "en".into())
                .await;
        }
    }
    running.fetch_sub(1, Ordering::SeqCst);
}

/// `scp -t`: takes one file into `target` (a file, or a folder to put it
/// in). Whether the transfer went through.
async fn receive(link: &mut Link, target: &Path, write_fails: bool, stall: bool) -> bool {
    link.send(&[0]).await;
    let Some(header) = link.line().await else {
        return false;
    };
    let mut parts = header.splitn(3, ' ');
    let (Some(mode), Some(size), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        link.refuse("protocol error: bad header").await;
        return false;
    };
    let Ok(size) = size.parse::<u64>() else {
        link.refuse("protocol error: bad size").await;
        return false;
    };
    if !mode.starts_with('C') {
        link.refuse("protocol error: expected a file").await;
        return false;
    }
    let file_path = if target.is_dir() {
        target.join(name)
    } else {
        target.to_path_buf()
    };
    let mut file = if write_fails {
        None
    } else {
        match fs::File::create(&file_path) {
            Ok(file) => Some(file),
            Err(error) => {
                link.refuse(&format!("scp: {name}: {error}")).await;
                return false;
            }
        }
    };
    link.send(&[0]).await;
    let mut left = size;
    while left > 0 {
        if stall && left <= size / 2 {
            std::future::pending::<()>().await;
        }
        if link.pending.is_empty() && !link.fill().await {
            // Cut short: the partial file stays, as on cEOS and Junos.
            return false;
        }
        let take = link.pending.len().min(left as usize);
        if let Some(file) = &mut file {
            std::io::Write::write_all(file, &link.pending[..take]).unwrap();
        }
        link.pending.drain(..take);
        left -= take as u64;
    }
    if link.byte().await != Some(0) {
        return false;
    }
    if write_fails {
        link.refuse("Write failed").await;
        return true;
    }
    drop(file);
    link.send(&[0]).await;
    // The client ends with EOF.
    while link.fill().await {}
    link.end(0).await;
    true
}

/// `scp -f`: sends the file at `source`. Whether the transfer went through.
async fn send(link: &mut Link, asked: &str, source: &Path, short: bool, stall: bool) -> bool {
    if link.byte().await != Some(0) {
        return false;
    }
    // Read a piece at a time, so that a large file is never in memory.
    let mut file = match fs::File::open(source) {
        Ok(file) if source.is_file() => file,
        _ => {
            link.refuse(&format!("scp: {asked}: No such file or directory"))
                .await;
            return false;
        }
    };
    let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    link.send(format!("C0644 {size} {name}\n").as_bytes()).await;
    if link.byte().await != Some(0) {
        return false;
    }
    let mut left = if short || stall { size / 2 } else { size };
    let mut chunk = vec![0; 32 * 1024];
    while left > 0 {
        let want = chunk.len().min(left as usize);
        std::io::Read::read_exact(&mut file, &mut chunk[..want]).unwrap();
        link.send(&chunk[..want]).await;
        left -= want as u64;
    }
    if stall {
        std::future::pending::<()>().await;
    }
    if short {
        link.end(1).await;
        return false;
    }
    link.send(&[0]).await;
    if link.byte().await != Some(0) {
        return false;
    }
    while link.fill().await {}
    link.end(0).await;
    true
}
