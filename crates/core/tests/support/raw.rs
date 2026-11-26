//! A device that speaks raw bytes on a loopback port, for the telnet and
//! SSH cases the scripted servers cannot play (#118): it sends what it is
//! scripted to (each piece after its pause), records what it receives,
//! and counts what it sent. [`RawDevice::negotiating_forever`] never stops
//! offering and withdrawing the echo and never reads; [`RawDevice::ping_pong`]
//! asks again for an option every time it is refused;
//! [`RawDevice::reading_slowly`] takes input in slowly.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use enodix_core::session::{TelnetTarget, TerminalType, DEFAULT_CONNECT_TIMEOUT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const IAC: u8 = 255;
pub const DO: u8 = 253;
pub const WONT: u8 = 252;
pub const WILL: u8 = 251;
pub const ECHO: u8 = 1;
/// A telnet option nobody implements.
pub const UNKNOWN_OPTION: u8 = 99;

/// What a raw device does with each connection.
#[derive(Clone)]
enum Script {
    /// Sends each piece after its pause, then reads until the end (or
    /// closes, if `close`).
    Pieces {
        pieces: Vec<(Duration, Vec<u8>)>,
        close: bool,
    },
    /// Sends `IAC DO <unknown>` without end and never reads.
    NegotiatingForever,
    /// Asks `DO <unknown>` again every time the client refuses it.
    PingPong,
    /// Sends `greeting`, then reads 1 KiB at a time, pausing after each.
    ReadingSlowly { greeting: Vec<u8>, pause: Duration },
}

pub struct RawDevice {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    sent: Arc<AtomicU64>,
    received: Arc<Mutex<Vec<u8>>>,
    refusals: Arc<AtomicUsize>,
}

impl RawDevice {
    /// Sends each piece after its pause, then reads what comes until the
    /// client closes.
    pub async fn sending(pieces: Vec<(Duration, Vec<u8>)>) -> Self {
        Self::start(Script::Pieces {
            pieces,
            close: false,
        })
        .await
    }

    /// Sends each piece after its pause, then closes the connection.
    pub async fn sending_then_closing(pieces: Vec<(Duration, Vec<u8>)>) -> Self {
        Self::start(Script::Pieces {
            pieces,
            close: true,
        })
        .await
    }

    /// Sends telnet option requests as fast as the client takes them, and
    /// never reads what the client answers.
    pub async fn negotiating_forever() -> Self {
        Self::start(Script::NegotiatingForever).await
    }

    /// Asks for an option the client refuses, again after every refusal.
    pub async fn ping_pong() -> Self {
        Self::start(Script::PingPong).await
    }

    /// Sends `greeting`, then reads what comes 1 KiB at a time, pausing
    /// `pause` after each: a slow device taking a paste.
    pub async fn reading_slowly(greeting: &[u8], pause: Duration) -> Self {
        Self::start(Script::ReadingSlowly {
            greeting: greeting.to_vec(),
            pause,
        })
        .await
    }

    async fn start(script: Script) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sent = Arc::<AtomicU64>::default();
        let received = Arc::<Mutex<Vec<u8>>>::default();
        let refusals = Arc::<AtomicUsize>::default();
        let (counting, keeping, refused) = (sent.clone(), received.clone(), refusals.clone());
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let _ = socket.set_nodelay(true);
                tokio::spawn(serve(
                    socket,
                    script.clone(),
                    counting.clone(),
                    keeping.clone(),
                    refused.clone(),
                ));
            }
        });
        Self {
            port,
            task,
            sent,
            received,
            refusals,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// A telnet target for it.
    pub fn telnet_target(&self) -> TelnetTarget {
        TelnetTarget {
            host: "127.0.0.1".into(),
            port: self.port,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            terminal_type: TerminalType::default(),
        }
    }

    /// How many bytes it has sent, on every connection.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::SeqCst)
    }

    /// What it has received, on every connection.
    pub fn received(&self) -> Vec<u8> {
        self.received.lock().unwrap().clone()
    }

    /// How many times the client refused its option (`PingPong`).
    pub fn refusals(&self) -> usize {
        self.refusals.load(Ordering::SeqCst)
    }
}

impl Drop for RawDevice {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    script: Script,
    sent: Arc<AtomicU64>,
    received: Arc<Mutex<Vec<u8>>>,
    refusals: Arc<AtomicUsize>,
) {
    match script {
        Script::Pieces { pieces, close } => {
            for (pause, piece) in pieces {
                tokio::time::sleep(pause).await;
                if socket.write_all(&piece).await.is_err() {
                    return;
                }
                sent.fetch_add(piece.len() as u64, Ordering::SeqCst);
            }
            let mut buffer = [0u8; 4096];
            if close {
                // An orderly close: what it sent arrives before the end
                // (closing with the client's bytes unread would reset the
                // connection instead, losing them).
                let _ = socket.shutdown().await;
                let drain = async { while let Ok(1..) = socket.read(&mut buffer).await {} };
                let _ = tokio::time::timeout(Duration::from_secs(5), drain).await;
                return;
            }
            while let Ok(count @ 1..) = socket.read(&mut buffer).await {
                received.lock().unwrap().extend_from_slice(&buffer[..count]);
            }
        }
        Script::ReadingSlowly { greeting, pause } => {
            if socket.write_all(&greeting).await.is_err() {
                return;
            }
            sent.fetch_add(greeting.len() as u64, Ordering::SeqCst);
            let mut buffer = [0u8; 1024];
            while let Ok(count @ 1..) = socket.read(&mut buffer).await {
                received.lock().unwrap().extend_from_slice(&buffer[..count]);
                if !pause.is_zero() {
                    tokio::time::sleep(pause).await;
                }
            }
        }
        Script::NegotiatingForever => {
            // Offers to echo and takes it back, each a change the client
            // has to answer.
            let requests = [IAC, WILL, ECHO, IAC, WONT, ECHO].repeat(2048);
            while socket.write_all(&requests).await.is_ok() {
                sent.fetch_add(requests.len() as u64, Ordering::SeqCst);
            }
        }
        Script::PingPong => {
            let ask = [IAC, DO, UNKNOWN_OPTION];
            if socket.write_all(&ask).await.is_err() {
                return;
            }
            let mut buffer = [0u8; 4096];
            let mut seen = Vec::new();
            while let Ok(count @ 1..) = socket.read(&mut buffer).await {
                seen.extend_from_slice(&buffer[..count]);
                // Every refusal of the option is answered with the request.
                while let Some(at) = seen
                    .windows(3)
                    .position(|w| w == [IAC, WONT, UNKNOWN_OPTION])
                {
                    seen.drain(..at + 3);
                    refusals.fetch_add(1, Ordering::SeqCst);
                    if socket.write_all(&ask).await.is_err() {
                        return;
                    }
                    sent.fetch_add(3, Ordering::SeqCst);
                }
                let keep = seen.len().saturating_sub(2);
                seen.drain(..keep);
                received.lock().unwrap().extend_from_slice(&buffer[..count]);
            }
        }
    }
}
