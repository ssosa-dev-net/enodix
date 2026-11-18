//! A plain TCP service on a loopback port, standing in for a device's
//! second service (NETCONF on 830, an API on 443) beside its SSH server:
//! the tunnels' extra ports reach it. No protocol, just bytes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the service does with each connection.
#[derive(Clone, Copy)]
enum Mode {
    /// Sends `<banner>\n`, then echoes whatever it receives.
    Echo(&'static str),
    /// Sends that many bytes, then waits for the client to go away.
    Flood(usize),
}

pub struct TcpService {
    port: u16,
    accepted: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl TcpService {
    /// A service that greets each connection with `banner` and a newline,
    /// then echoes.
    pub async fn echo(banner: &'static str) -> Self {
        Self::start(Mode::Echo(banner)).await
    }

    /// A service that sends `bytes` bytes to each connection at once, as
    /// fast as the connection takes them.
    pub async fn flood(bytes: usize) -> Self {
        Self::start(Mode::Flood(bytes)).await
    }

    async fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = accepted.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            while let Ok((mut socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                connections.spawn(async move {
                    let mut buffer = vec![0; 32 * 1024];
                    match mode {
                        Mode::Echo(banner) => {
                            if socket
                                .write_all(format!("{banner}\n").as_bytes())
                                .await
                                .is_err()
                            {
                                return;
                            }
                            loop {
                                match socket.read(&mut buffer).await {
                                    Ok(0) | Err(_) => return,
                                    Ok(n) => {
                                        if socket.write_all(&buffer[..n]).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        Mode::Flood(bytes) => {
                            let chunk = vec![b'x'; 32 * 1024];
                            let mut left = bytes;
                            while left > 0 {
                                let n = left.min(chunk.len());
                                if socket.write_all(&chunk[..n]).await.is_err() {
                                    return;
                                }
                                left -= n;
                            }
                            while matches!(socket.read(&mut buffer).await, Ok(n) if n > 0) {}
                        }
                    }
                });
            }
        });
        Self {
            port,
            accepted,
            task,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// How many connections it accepted.
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

impl Drop for TcpService {
    fn drop(&mut self) {
        self.task.abort();
    }
}
