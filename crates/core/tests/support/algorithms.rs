//! Old devices for the legacy-crypto tests.
//!
//! - [`TestSshServer::start_with_algorithms`](super::TestSshServer::start_with_algorithms)
//!   starts the scripted server
//!   restricted to the algorithms given (with a host key of the matching
//!   type), and every test server records what the client offered in its
//!   key-exchange init ([`super::TestSshServer::client_offer`]);
//!   [`old_device`] gives the algorithms of an old device.
//! - [`FakeDevice`] is not an SSH server at all: it sends a version line
//!   and a key-exchange init naming whatever algorithms a test wants, even
//!   ones `russh` has never heard of (an MD5 MAC, say), then waits. A
//!   client can only fail to negotiate with it, which is what it is for.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use russh::keys::Algorithm;
use russh::{cipher, kex, mac, Preferred};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;

/// The algorithms of an old device: SHA-1 key exchange, an `ssh-rsa` host
/// key, a CBC cipher and a SHA-1 MAC, and nothing newer.
pub fn old_device() -> Preferred {
    Preferred {
        kex: vec![kex::DH_G14_SHA1].into(),
        key: vec![Algorithm::Rsa { hash: None }].into(),
        cipher: vec![cipher::AES_128_CBC].into(),
        mac: vec![mac::HMAC_SHA1].into(),
        ..Preferred::default()
    }
}

/// One side's key-exchange init: the algorithms it offers, per kind, as
/// named on the wire (client-to-server lists for cipher and MAC).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Offer {
    pub kex: Vec<String>,
    pub host_key: Vec<String>,
    pub cipher: Vec<String>,
    pub mac: Vec<String>,
}

impl Offer {
    /// Every algorithm named, of every kind.
    pub fn all(&self) -> Vec<&str> {
        [&self.kex, &self.host_key, &self.cipher, &self.mac]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect()
    }
}

/// How many bytes of a connection are kept: enough for the version line
/// and the key-exchange init.
const RECORDED: usize = 64 * 1024;

/// A stream that keeps a copy of the first bytes it reads.
pub struct Recording<S> {
    inner: S,
    read: Arc<Mutex<Vec<u8>>>,
}

impl<S> Recording<S> {
    pub fn new(inner: S, read: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { inner, read }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Recording<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let mut read = self.read.lock().unwrap();
            let room = RECORDED.saturating_sub(read.len());
            let new = &buf.filled()[before..];
            read.extend_from_slice(&new[..new.len().min(room)]);
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Recording<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// The offer in the key-exchange init that follows the version line at the
/// start of `bytes` (what one side sent first), if it is all there.
pub fn parse_offer(bytes: &[u8]) -> Option<Offer> {
    // Lines before the version line are allowed; the version line ends the
    // text part.
    let mut rest = bytes;
    loop {
        let end = rest.iter().position(|&b| b == b'\n')?;
        let line = &rest[..end];
        rest = &rest[end + 1..];
        if line.starts_with(b"SSH-") {
            break;
        }
    }
    let length = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let packet = rest.get(4..4 + length)?;
    let padding = usize::from(*packet.first()?);
    let payload = packet.get(1..length.checked_sub(padding)?)?;
    // SSH_MSG_KEXINIT, then a 16-byte cookie.
    if payload.first() != Some(&20) {
        return None;
    }
    let mut lists = payload.get(17..)?;
    let mut next = || -> Option<Vec<String>> {
        let length = u32::from_be_bytes(lists.get(..4)?.try_into().ok()?) as usize;
        let names = std::str::from_utf8(lists.get(4..4 + length)?).ok()?;
        lists = &lists[4 + length..];
        Some(
            names
                .split(',')
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    };
    let kex = next()?;
    let host_key = next()?;
    let cipher = next()?;
    let _cipher_from_server = next()?;
    let mac = next()?;
    Some(Offer {
        kex,
        host_key,
        cipher,
        mac,
    })
}

/// A key-exchange init offering `offer` (no compression but `none`).
fn kexinit_packet(offer: &Offer) -> Vec<u8> {
    let mut payload = vec![20u8];
    payload.extend_from_slice(&[7u8; 16]);
    let lists = [
        offer.kex.join(","),
        offer.host_key.join(","),
        offer.cipher.join(","),
        offer.cipher.join(","),
        offer.mac.join(","),
        offer.mac.join(","),
        "none".into(),
        "none".into(),
        String::new(),
        String::new(),
    ];
    for list in lists {
        payload.extend_from_slice(&(list.len() as u32).to_be_bytes());
        payload.extend_from_slice(list.as_bytes());
    }
    payload.push(0); // first_kex_packet_follows
    payload.extend_from_slice(&0u32.to_be_bytes());
    // Padding: at least 4 bytes, the whole packet a multiple of 8.
    let mut padding = 8 - (5 + payload.len()) % 8;
    if padding < 4 {
        padding += 8;
    }
    let mut packet = Vec::new();
    packet.extend_from_slice(&((1 + payload.len() + padding) as u32).to_be_bytes());
    packet.push(padding as u8);
    packet.extend_from_slice(&payload);
    packet.extend(std::iter::repeat_n(0u8, padding));
    packet
}

/// A device that offers `offer` in its key-exchange init and nothing else.
pub struct FakeDevice {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl FakeDevice {
    pub async fn start(offer: Offer) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let packet = kexinit_packet(&offer);
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let packet = packet.clone();
                tokio::spawn(async move {
                    let _ = socket.write_all(b"SSH-2.0-OldDevice_1.0\r\n").await;
                    let _ = socket.write_all(&packet).await;
                    // Read until the client gives up.
                    let mut buffer = [0u8; 4096];
                    while matches!(socket.read(&mut buffer).await, Ok(n) if n > 0) {}
                });
            }
        });
        Self { port, task }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.task.abort();
    }
}
