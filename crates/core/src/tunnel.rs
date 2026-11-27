//! Loopback tunnels to hosts behind jump hosts, for scripts.
//!
//! Python libraries (Netmiko, scrapli, NAPALM) make their own SSH
//! connections and know nothing of the app's jump hosts. The headless
//! binary (`enodix-cli tunnel`) therefore opens each host's jump chain here,
//! exactly as a terminal session would (the app's known host keys, stored
//! profile or agent logins, legacy crypto and connect timeouts of each
//! hop), and listens on a loopback port per host; the inventory plugin
//! hands the library `127.0.0.1` and that port.
//!
//! [`plan`] works out, connecting to nothing, which hosts are behind jump
//! hosts and which of them cannot be tunnelled ([`Planned::Refused`]: a
//! broken chain, a jump host with no stored profile, telnet, or a device
//! whose host key the app has not trusted yet: the library checks the
//! device's key, and gets the trusted ones in [`TunnelRequest::host_keys`]).
//! [`Tunnels::open`] then connects each distinct chain once, all at once,
//! before anything listens, so every refusal (an untrusted jump-host key
//! named by its fingerprint, a login that asks questions, an unreachable
//! jump host) is in [`Tunnels::status`]. Nobody is ever asked anything,
//! and nothing is trusted on the user's behalf.
//!
//! A host gets a listener for its SSH port and one for each extra device
//! port the plan was given (NETCONF on 830, an API on 443: any TCP port).
//! Every connection accepted on one of them is carried, through a
//! `direct-tcpip` channel of the last jump host, to the host's address and
//! that port as that jump host reaches them; nothing is tried before.
//! Hosts behind the same chain share its connections, whatever the port; a
//! chain that has dropped is connected again on the next connection, and
//! a failure on another port than SSH's names it. Each channel's output is
//! drained into its own buffer
//! at once (russh blocks the whole connection while one channel's messages
//! are not taken, so one slow reader would stall every tunnel of the
//! chain); a reader that lets more than [`TunnelOptions::buffer_limit`]
//! wait loses its connection, with a [`TunnelEvent::Failed`] saying so.
//! Listeners bind to `127.0.0.1` only; dropping [`Tunnels`] closes them and
//! the chains.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use russh::ChannelMsg;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::{JoinHandle, JoinSet};

use crate::credentials::Profiles;
use crate::known_hosts::KnownHosts;
use crate::session::{open_jump_chain_unattended, JumpConnections, JumpHost, SshAuth};
use crate::workspace::{HostId, Protocol, ResolvedHost, Workspace};

/// How much of one tunnel's output may wait for its reader by default.
pub const DEFAULT_BUFFER_LIMIT: usize = 8 * 1024 * 1024;

/// How the tunnels behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelOptions {
    /// How many bytes of a connection's output may wait for the program
    /// that reads it; more, and that connection is closed.
    pub buffer_limit: usize,
}

impl Default for TunnelOptions {
    fn default() -> Self {
        Self {
            buffer_limit: DEFAULT_BUFFER_LIMIT,
        }
    }
}

/// A host to tunnel to, as planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelRequest {
    pub host: HostId,
    /// The device's address and SSH port, as the last jump host reaches
    /// them.
    pub address: String,
    pub port: u16,
    /// The device's other ports to tunnel too, ascending, without `port`.
    pub ports: Vec<u16>,
    /// The chain, in connect order; at least one.
    pub jump_hosts: Vec<JumpHost>,
    /// The device's trusted host keys (`<type> <base64>`, as in a
    /// known-hosts file), for the library to check it by.
    pub host_keys: Vec<String>,
    /// The device's own connect timeout: how long the last jump host may
    /// take to reach it.
    pub connect_timeout: Duration,
}

/// What [`plan`] makes of a host behind jump hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    Ready(TunnelRequest),
    /// It cannot be tunnelled, and why.
    Refused {
        host: HostId,
        reason: String,
    },
}

impl Planned {
    pub fn host(&self) -> &HostId {
        match self {
            Self::Ready(request) => &request.host,
            Self::Refused { host, .. } => host,
        }
    }
}

/// The hosts behind jump hosts among those asked for, in the order asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelPlan {
    pub hosts: Vec<Planned>,
}

/// Works out which of `ids` are behind jump hosts and how to tunnel to
/// each, connecting to nothing. Hosts reached directly (and serial ones)
/// are left out; ids the workspace lacks too. `ports` are device ports to
/// tunnel besides each host's SSH port (it, a port named twice and port 0
/// add nothing); whether a device listens on them is not checked, and they
/// change nothing about which hosts are refused.
pub fn plan(
    workspace: &Workspace,
    profiles: &Profiles<'_>,
    known_hosts: &KnownHosts,
    ids: &[HostId],
    ports: &[u16],
) -> TunnelPlan {
    let ports: BTreeSet<u16> = ports.iter().copied().filter(|&port| port != 0).collect();
    let mut plan = TunnelPlan::default();
    for id in ids {
        let Some(host) = workspace.resolve_host(id) else {
            continue;
        };
        let protocol = host.settings.protocol.value;
        if protocol == Protocol::Serial {
            continue;
        }
        let refused = |reason: String| Planned::Refused {
            host: id.clone(),
            reason,
        };
        let planned = match workspace.jump_chain(&host) {
            Ok(chain) if chain.is_empty() => continue,
            _ if protocol == Protocol::Telnet => refused(
                "it is reached over telnet through a jump host, and only SSH is tunnelled".into(),
            ),
            Err(error) => refused(format!("its jump hosts cannot be used: {error}")),
            Ok(_) => match request(profiles, known_hosts, &host) {
                Ok(mut request) => {
                    request.ports = ports
                        .iter()
                        .copied()
                        .filter(|&p| p != request.port)
                        .collect();
                    Planned::Ready(request)
                }
                Err(reason) => refused(reason),
            },
        };
        plan.hosts.push(planned);
    }
    plan
}

/// Those of `ids` that [`plan`] would list (behind jump hosts, a broken
/// chain included; serial hosts and ids the workspace lacks left out),
/// worked out without a credential store.
pub fn behind_jump_hosts(workspace: &Workspace, ids: &[HostId]) -> Vec<HostId> {
    ids.iter()
        .filter(|id| {
            workspace.resolve_host(id).is_some_and(|host| {
                host.settings.protocol.value != Protocol::Serial
                    && workspace
                        .jump_chain(&host)
                        .map_or(true, |chain| !chain.is_empty())
            })
        })
        .cloned()
        .collect()
}

/// The tunnel to the SSH host `host`, behind a working chain.
fn request(
    profiles: &Profiles<'_>,
    known_hosts: &KnownHosts,
    host: &ResolvedHost,
) -> Result<TunnelRequest, String> {
    let id = &host.host.id;
    let jump_hosts = profiles.jump_hosts(id).map_err(|error| error.to_string())?;
    if let Some(jump) = jump_hosts.iter().find(|jump| jump.auth == SshAuth::Prompt) {
        return Err(format!(
            "its jump host {} has no stored credential profile, and a tunnel cannot ask for its login",
            jump.name
        ));
    }
    let target = host.ssh_target(String::new(), String::new());
    let keys = known_hosts
        .trusted(&target.host, target.port)
        .map_err(|error| error.to_string())?;
    if keys.is_empty() {
        let revoked = known_hosts
            .revoked_remembered(&target.host, target.port)
            .map_err(|error| error.to_string())?;
        if !revoked.is_empty() {
            return Err(format!(
                "its host key (at {} port {}) is revoked in the known hosts, so the script \
                 could not trust it: the device has to present another key, which is then \
                 trusted by connecting to it once in the app",
                target.host, target.port
            ));
        }
        return Err(format!(
            "the app has not trusted its host key (at {} port {}) yet, so the script could not \
             check it: connect to it once in the app and trust and remember its key",
            target.host, target.port
        ));
    }
    let host_keys = keys
        .iter()
        .filter_map(|key| key.to_openssh().ok())
        .map(|text| {
            text.split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    Ok(TunnelRequest {
        host: id.clone(),
        address: target.host,
        port: target.port,
        ports: Vec::new(),
        jump_hosts,
        host_keys,
        connect_timeout: target.connect_timeout,
    })
}

/// A host's tunnel, once [`Tunnels::open`] has tried its chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TunnelStatus {
    pub host: HostId,
    #[serde(flatten)]
    pub state: TunnelState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TunnelState {
    /// Connect to `address` (`127.0.0.1`) at `port` to reach the host's
    /// SSH port.
    Listening {
        address: String,
        port: u16,
        /// [`TunnelRequest::host_keys`].
        host_keys: Vec<String>,
        /// For each of [`TunnelRequest::ports`], the port on `address`
        /// that reaches it; left out of the JSON when there are none.
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        extra_ports: BTreeMap<u16, u16>,
    },
    /// Not tunnelled, and why.
    Refused { reason: String },
}

/// What happens to the tunnels after they opened, for the owner to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// A chain (its jump hosts' names, in order) was connected.
    ChainConnected { chain: Vec<String> },
    /// A chain could not be connected.
    ChainFailed { chain: Vec<String>, reason: String },
    /// A connection to `host` could not be carried through, or broke. The
    /// reason starts `port <n>: ` when it was to another port than SSH's.
    Failed { host: HostId, reason: String },
}

impl fmt::Display for TunnelEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChainConnected { chain } => write!(f, "connected through {}", chain.join(" > ")),
            Self::ChainFailed { chain, reason } => {
                write!(f, "cannot connect through {}: {reason}", chain.join(" > "))
            }
            Self::Failed { host, reason } => write!(f, "{host}: {reason}"),
        }
    }
}

type Report = Arc<dyn Fn(TunnelEvent) + Send + Sync>;

/// One chain's jump hosts and, once connected, its connections.
struct Chain {
    jump_hosts: Vec<JumpHost>,
    connected: Mutex<Option<Arc<JumpConnections>>>,
}

fn names(jump_hosts: &[JumpHost]) -> Vec<String> {
    jump_hosts.iter().map(|jump| jump.name.clone()).collect()
}

/// Running tunnels. Dropping them stops them: the listeners close, and so
/// do the connections they carry and the chains.
pub struct Tunnels {
    status: Vec<TunnelStatus>,
    tasks: Vec<JoinHandle<()>>,
}

impl Tunnels {
    /// Connects each distinct chain of `plan` (at once), then listens for
    /// every host whose chain connected; the rest are refused with the
    /// chain's reason. Runs on the current tokio runtime.
    pub async fn open(
        plan: TunnelPlan,
        known_hosts: KnownHosts,
        options: TunnelOptions,
        report: impl Fn(TunnelEvent) + Send + Sync + 'static,
    ) -> io::Result<Tunnels> {
        let report: Report = Arc::new(report);
        let mut chains: HashMap<Vec<String>, Arc<Chain>> = HashMap::new();
        for planned in &plan.hosts {
            if let Planned::Ready(request) = planned {
                chains.entry(names(&request.jump_hosts)).or_insert_with(|| {
                    Arc::new(Chain {
                        jump_hosts: request.jump_hosts.clone(),
                        connected: Mutex::new(None),
                    })
                });
            }
        }
        let mut attempts = JoinSet::new();
        for (key, chain) in &chains {
            let (key, chain, known_hosts) = (key.clone(), chain.clone(), known_hosts.clone());
            attempts.spawn(async move {
                let opened = open_jump_chain_unattended(&chain.jump_hosts, &known_hosts).await;
                let failure = match opened {
                    Ok(connections) => {
                        *chain.connected.lock().await = Some(Arc::new(connections));
                        None
                    }
                    Err(reason) => Some(reason),
                };
                (key, failure)
            });
        }
        let mut failures: HashMap<Vec<String>, Option<String>> = HashMap::new();
        while let Some(attempt) = attempts.join_next().await {
            let (key, failure) = attempt.map_err(io::Error::other)?;
            failures.insert(key, failure);
        }

        let mut status = Vec::with_capacity(plan.hosts.len());
        let mut tasks = Vec::new();
        for planned in plan.hosts {
            let request = match planned {
                Planned::Refused { host, reason } => {
                    status.push(TunnelStatus {
                        host,
                        state: TunnelState::Refused { reason },
                    });
                    continue;
                }
                Planned::Ready(request) => request,
            };
            let key = names(&request.jump_hosts);
            if let Some(Some(reason)) = failures.get(&key) {
                status.push(TunnelStatus {
                    host: request.host,
                    state: TunnelState::Refused {
                        reason: reason.clone(),
                    },
                });
                continue;
            }
            // One listener for the SSH port, then one per extra port.
            let mut listeners = Vec::with_capacity(1 + request.ports.len());
            for device_port in std::iter::once(request.port).chain(request.ports.iter().copied()) {
                let listener =
                    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
                listeners.push((device_port, listener.local_addr()?, listener));
            }
            let address = listeners[0].1;
            status.push(TunnelStatus {
                host: request.host.clone(),
                state: TunnelState::Listening {
                    address: address.ip().to_string(),
                    port: address.port(),
                    host_keys: request.host_keys.clone(),
                    extra_ports: listeners[1..]
                        .iter()
                        .map(|(device_port, local, _)| (*device_port, local.port()))
                        .collect(),
                },
            });
            let request = Arc::new(request);
            for (device_port, _, listener) in listeners {
                tasks.push(tokio::spawn(serve(
                    listener,
                    request.clone(),
                    device_port,
                    chains[&key].clone(),
                    known_hosts.clone(),
                    options,
                    report.clone(),
                )));
            }
        }
        Ok(Tunnels { status, tasks })
    }

    /// Each planned host's tunnel, in the plan's order.
    pub fn status(&self) -> &[TunnelStatus] {
        &self.status
    }
}

impl Drop for Tunnels {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Accepts connections to one port of one host until aborted; aborting it
/// ends the connections it carries too.
async fn serve(
    listener: TcpListener,
    request: Arc<TunnelRequest>,
    device_port: u16,
    chain: Arc<Chain>,
    known_hosts: KnownHosts,
    options: TunnelOptions,
    report: Report,
) {
    let mut carried = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((socket, _)) => {
                    let request = request.clone();
                    let chain = chain.clone();
                    let known_hosts = known_hosts.clone();
                    let report = report.clone();
                    carried.spawn(async move {
                        let carried = carry(
                            socket,
                            &request,
                            device_port,
                            &chain,
                            &known_hosts,
                            options,
                            &report,
                        )
                        .await;
                        if let Err(reason) = carried {
                            let reason = if device_port == request.port {
                                reason
                            } else {
                                format!("port {device_port}: {reason}")
                            };
                            report(TunnelEvent::Failed {
                                host: request.host.clone(),
                                reason,
                            });
                        }
                    });
                }
                // Out of sockets, say: try again shortly.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            },
            Some(_) = carried.join_next(), if !carried.is_empty() => {}
        }
    }
}

/// The chain's connections, connected again if they have dropped.
async fn connected(
    chain: &Chain,
    known_hosts: &KnownHosts,
    report: &Report,
) -> Result<Arc<JumpConnections>, String> {
    let mut connected = chain.connected.lock().await;
    if let Some(connections) = connected.as_ref().filter(|c| c.lost().is_none()) {
        return Ok(connections.clone());
    }
    let names = names(&chain.jump_hosts);
    match open_jump_chain_unattended(&chain.jump_hosts, known_hosts).await {
        Ok(connections) => {
            let connections = Arc::new(connections);
            *connected = Some(connections.clone());
            report(TunnelEvent::ChainConnected { chain: names });
            Ok(connections)
        }
        Err(reason) => {
            *connected = None;
            report(TunnelEvent::ChainFailed {
                chain: names,
                reason: reason.clone(),
            });
            Err(reason)
        }
    }
}

/// Carries `socket` to `device_port` of the request's host until either
/// side ends.
async fn carry(
    socket: TcpStream,
    request: &TunnelRequest,
    device_port: u16,
    chain: &Chain,
    known_hosts: &KnownHosts,
    options: TunnelOptions,
    report: &Report,
) -> Result<(), String> {
    let connections = connected(chain, known_hosts, report)
        .await
        .map_err(|reason| format!("not connected: {reason}"))?;
    let channel = connections
        .forward(&request.address, device_port, request.connect_timeout)
        .await
        .map_err(|error| error.to_string())?;
    let carried = pump(socket, channel, options.buffer_limit).await;
    if let Some(name) = connections.lost() {
        return Err(format!("the connection to the jump host {name} was lost"));
    }
    carried
}

/// Moves bytes both ways between `socket` and `channel` until the device
/// side ends (or either breaks). The device's output is taken from the
/// channel at once, whatever the socket's reader does, and waits in a
/// buffer of at most `limit` bytes.
pub(crate) async fn pump(
    socket: TcpStream,
    channel: russh::Channel<russh::client::Msg>,
    limit: usize,
) -> Result<(), String> {
    let _ = socket.set_nodelay(true);
    let (mut from_device, to_device) = channel.split();
    let (mut from_client, mut to_client) = socket.into_split();
    let waiting = Arc::new(AtomicUsize::new(0));
    let (queue, mut queued) = mpsc::unbounded_channel();

    // `Err(None)`: over without anything to report (the program went away).
    let drain = {
        let waiting = waiting.clone();
        async move {
            loop {
                match from_device.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        let now = waiting.fetch_add(data.len(), Ordering::SeqCst) + data.len();
                        if now > limit {
                            return Err(Some(format!(
                                "the program using the tunnel did not read its output: more than \
                                 {} KiB waited, so its connection was closed",
                                limit / 1024
                            )));
                        }
                        if queue.send(data).is_err() {
                            return Err(None);
                        }
                    }
                    Some(ChannelMsg::Eof | ChannelMsg::Close) | None => return Ok(()),
                    Some(_) => {}
                }
            }
        }
    };
    let write = async move {
        while let Some(data) = queued.recv().await {
            to_client.write_all(&data).await.map_err(|_| None)?;
            waiting.fetch_sub(data.len(), Ordering::SeqCst);
        }
        let _ = to_client.shutdown().await;
        Ok::<(), Option<String>>(())
    };
    let up = async {
        let mut buffer = vec![0; 32 * 1024];
        loop {
            match from_client.read(&mut buffer).await {
                Ok(0) => {
                    // Half closed: the device side decides when it is over.
                    let _ = to_device.eof().await;
                    std::future::pending::<()>().await;
                }
                Ok(n) => {
                    if to_device.data(&buffer[..n]).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    };
    let ended = tokio::select! {
        ended = async { tokio::try_join!(drain, write).map(|_| ()) } => ended,
        () = up => Ok(()),
    };
    let _ = to_device.close().await;
    match ended {
        Ok(()) | Err(None) => Ok(()),
        Err(Some(reason)) => Err(reason),
    }
}
