//! A link between the session and a test server that can stall or break
//! (#118): a TCP relay on a loopback port that passes bytes both ways
//! until told to stop passing them (the sockets stay open, no FIN: a link
//! that went silent, a laptop asleep) or to cut every connection (both
//! sides see it close).
//!
//! An SSH server behind it is reached at the proxy's port, so its host
//! key is trusted there too ([`LinkProxy::to_ssh`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::{known_hosts, target_at, TestSshServer};
use enodix_core::session::SshTarget;

/// What the link does with the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Passing,
    /// Holds them: nothing crosses, nothing closes.
    Stalled,
    /// Closes every connection made so far; later ones pass.
    Cut(u64),
}

pub struct LinkProxy {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    state: watch::Sender<State>,
    open: Arc<AtomicUsize>,
    to_server: Arc<AtomicUsize>,
    to_client: Arc<AtomicUsize>,
}

impl LinkProxy {
    /// A link to `upstream` on the loopback address.
    pub async fn start(upstream: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (state, watching) = watch::channel(State::Passing);
        let open = Arc::<AtomicUsize>::default();
        let counted = open.clone();
        let to_server = Arc::<AtomicUsize>::default();
        let to_client = Arc::<AtomicUsize>::default();
        let (writes_up, writes_down) = (to_server.clone(), to_client.clone());
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = TcpStream::connect(("127.0.0.1", upstream)).await else {
                    continue;
                };
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                // Connections made after a cut are not cut by it.
                let cuts = match *watching.borrow() {
                    State::Cut(n) => n,
                    _ => 0,
                };
                let watching = watching.clone();
                let counted = counted.clone();
                let (writes_up, writes_down) = (writes_up.clone(), writes_down.clone());
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let (client_read, client_write) = client.into_split();
                    let (server_read, server_write) = server.into_split();
                    tokio::select! {
                        () = pass(client_read, server_write, watching.clone(), cuts, writes_up) => {}
                        () = pass(server_read, client_write, watching, cuts, writes_down) => {}
                    }
                    counted.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self {
            port,
            task,
            state,
            open,
            to_server,
            to_client,
        }
    }

    /// A link to `server`, whose host key is trusted at the link's port.
    pub async fn to_ssh(server: &TestSshServer) -> Self {
        let proxy = Self::start(server.port()).await;
        known_hosts()
            .remember("127.0.0.1", proxy.port, server.host_key())
            .unwrap();
        proxy
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// A target that logs in to the server behind the link through it.
    pub fn target(&self) -> SshTarget {
        target_at("127.0.0.1", self.port)
    }

    /// From now on nothing crosses the link, and nothing closes: the far
    /// side hears nothing more, as when a link goes silent.
    pub fn stall(&self) {
        self.state.send_replace(State::Stalled);
    }

    /// Bytes cross again, those held first.
    pub fn resume(&self) {
        self.state.send_replace(State::Passing);
    }

    /// Closes every connection made through the link so far, both ways;
    /// new ones pass.
    pub fn cut(&self) {
        self.state.send_modify(|state| {
            let cuts = match *state {
                State::Cut(n) => n,
                _ => 0,
            };
            *state = State::Cut(cuts + 1);
        });
    }

    /// How many writes the link has passed from the session to the server
    /// so far (each is one or more SSH packets: a keepalive is one).
    pub fn writes_to_server(&self) -> usize {
        self.to_server.load(Ordering::SeqCst)
    }

    /// How many writes the link has passed from the server to the session
    /// so far.
    pub fn writes_to_client(&self) -> usize {
        self.to_client.load(Ordering::SeqCst)
    }

    /// How many connections pass through it now.
    pub fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

impl Drop for LinkProxy {
    fn drop(&mut self) {
        self.task.abort();
        self.cut();
    }
}

/// Passes what `from` reads to `to` while the state allows, until either
/// end closes or a cut after `cuts` comes.
async fn pass(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    mut state: watch::Receiver<State>,
    cuts: u64,
    writes: Arc<AtomicUsize>,
) {
    let mut buffer = vec![0u8; 16 * 1024];
    let cut = |state: State| matches!(state, State::Cut(n) if n > cuts);
    loop {
        // Wait until bytes may cross.
        let passing = state.wait_for(|s| *s != State::Stalled).await.map(|s| *s);
        match passing {
            Ok(s) if cut(s) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        let read = tokio::select! {
            read = from.read(&mut buffer) => read,
            changed = state.changed() => {
                if changed.is_err() {
                    return;
                }
                continue;
            }
        };
        let count = match read {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        // Held while stalled.
        let passing = state.wait_for(|s| *s != State::Stalled).await.map(|s| *s);
        match passing {
            Ok(s) if cut(s) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        if to.write_all(&buffer[..count]).await.is_err() {
            return;
        }
        writes.fetch_add(1, Ordering::SeqCst);
    }
}
