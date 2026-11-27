//! Loopback tunnels through jump chains (`enodix_core::tunnel`), against
//! the in-process test servers. Most checks read the device's SSH banner
//! through the tunnel and count what the jump hosts forwarded.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::{HostKeyDecision, SessionState, SessionTarget, TerminalSize};
use enodix_core::tunnel::{
    self, Planned, TunnelEvent, TunnelOptions, TunnelState, TunnelStatus, Tunnels,
};
use enodix_core::workspace::{HostId, Workspace};
use support::tcp::TcpService;
use support::{TestSshServer, OTP_USERNAME, PASSWORD, USERNAME};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("tunnels");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

fn host(port: u16, extra: &str) -> String {
    format!("address: 127.0.0.1\nport: {port}\n{extra}")
}

fn profile(name: &str, username: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: username.into(),
        auth: ProfileAuth::Password {
            password: PASSWORD.into(),
        },
        enable_secret: None,
    }
}

/// A store holding the profile `lab`.
fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    store
        .put(&workspace.credential_scope(), &profile("lab", USERNAME))
        .unwrap();
    store
}

fn ids(ids: &[&str]) -> Vec<HostId> {
    ids.iter().map(|id| HostId::new(*id)).collect()
}

type Events = Arc<Mutex<Vec<TunnelEvent>>>;

/// Plans and opens tunnels to `hosts`, keeping the events.
async fn open(
    workspace: &Workspace,
    store: &MemoryStore,
    known_hosts: KnownHosts,
    hosts: &[&str],
    options: TunnelOptions,
) -> (Tunnels, Events) {
    open_with_ports(workspace, store, known_hosts, hosts, &[], options).await
}

/// [`open`], with extra device ports.
async fn open_with_ports(
    workspace: &Workspace,
    store: &MemoryStore,
    known_hosts: KnownHosts,
    hosts: &[&str],
    ports: &[u16],
    options: TunnelOptions,
) -> (Tunnels, Events) {
    let plan = tunnel::plan(
        workspace,
        &Profiles::new(store, workspace),
        &known_hosts,
        &ids(hosts),
        ports,
    );
    let events: Events = Arc::default();
    let kept = events.clone();
    let tunnels = Tunnels::open(plan, known_hosts, options, move |event| {
        kept.lock().unwrap().push(event);
    })
    .await
    .unwrap();
    (tunnels, events)
}

fn status<'a>(tunnels: &'a Tunnels, host: &str) -> &'a TunnelStatus {
    tunnels
        .status()
        .iter()
        .find(|status| status.host.as_str() == host)
        .unwrap_or_else(|| panic!("no status for {host}: {:?}", tunnels.status()))
}

fn port(tunnels: &Tunnels, host: &str) -> u16 {
    match &status(tunnels, host).state {
        TunnelState::Listening { port, .. } => *port,
        TunnelState::Refused { reason } => panic!("{host} was refused: {reason}"),
    }
}

/// The loopback ports serving `host`'s extra device ports.
fn extra_ports(tunnels: &Tunnels, host: &str) -> BTreeMap<u16, u16> {
    match &status(tunnels, host).state {
        TunnelState::Listening { extra_ports, .. } => extra_ports.clone(),
        TunnelState::Refused { reason } => panic!("{host} was refused: {reason}"),
    }
}

/// The loopback port serving `host`'s device port `device_port`.
fn extra_port(tunnels: &Tunnels, host: &str, device_port: u16) -> u16 {
    *extra_ports(tunnels, host)
        .get(&device_port)
        .unwrap_or_else(|| panic!("no listener for port {device_port} of {host}"))
}

fn refusal(tunnels: &Tunnels, host: &str) -> String {
    match &status(tunnels, host).state {
        TunnelState::Refused { reason } => reason.clone(),
        listening => panic!("{host} was not refused: {listening:?}"),
    }
}

/// The first line the far end sends through the tunnel at `port`.
async fn banner(port: u16) -> String {
    try_banner(port, Duration::from_secs(10))
        .await
        .expect("a banner within 10 s")
}

/// [`banner`], or `None` if none comes within `wait`.
async fn try_banner(port: u16, wait: Duration) -> Option<String> {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    first_line(&mut socket, wait).await
}

async fn first_line(socket: &mut TcpStream, wait: Duration) -> Option<String> {
    let mut read = Vec::new();
    let mut buffer = [0; 256];
    tokio::time::timeout(wait, async {
        while !read.contains(&b'\n') {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(
                n > 0,
                "the tunnel closed: {:?}",
                String::from_utf8_lossy(&read)
            );
            read.extend_from_slice(&buffer[..n]);
        }
    })
    .await
    .ok()?;
    Some(
        String::from_utf8_lossy(&read)
            .lines()
            .next()
            .unwrap()
            .to_owned(),
    )
}

/// Reads until the tunnel closes `socket`, within 10 s.
async fn closed(socket: &mut TcpStream) {
    let mut buffer = [0; 1024];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    })
    .await
    .expect("closed within 10 s");
}

async fn event_within(events: &Events, wait: Duration, wanted: impl Fn(&TunnelEvent) -> bool) {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if events.lock().unwrap().iter().any(&wanted) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no such event: {:?}", events.lock().unwrap());
}

fn openssh(key: &russh::keys::PublicKey) -> String {
    let text = key.to_openssh().unwrap();
    text.split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::test]
async fn one_jump_host() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, _) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    // The chain is connected before anything listens.
    assert_eq!(bastion.connections(), 1);
    assert_eq!(bastion.forwarded(), 0);
    let TunnelState::Listening {
        address, host_keys, ..
    } = &status(&tunnels, "r1.yaml").state
    else {
        panic!("{:?}", tunnels.status());
    };
    assert_eq!(address, "127.0.0.1");
    assert_eq!(host_keys, &vec![openssh(router.host_key())]);
    assert!(extra_ports(&tunnels, "r1.yaml").is_empty());

    assert!(banner(port(&tunnels, "r1.yaml"))
        .await
        .starts_with("SSH-2.0-"));
    assert_eq!(bastion.forwarded(), 1);
    assert_eq!(router.forwarded(), 0);
}

#[tokio::test]
async fn a_chain_of_two_jump_hosts() {
    let core = TestSshServer::start().await;
    let site = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("core.yaml", host(core.port(), "")),
        ("site.yaml", host(site.port(), "jump_host: core.yaml\n")),
        ("r1.yaml", host(router.port(), "jump_host: site.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, _) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    assert!(banner(port(&tunnels, "r1.yaml"))
        .await
        .starts_with("SSH-2.0-"));
    // The core forwarded to the site (connecting the chain), the site to
    // the router.
    assert_eq!(core.forwarded(), 1);
    assert_eq!(site.forwarded(), 1);
}

#[tokio::test]
async fn hosts_behind_one_jump_host_share_its_connection() {
    let bastion = TestSshServer::start().await;
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(r1.port(), "jump_host: bastion.yaml\n")),
        ("r2.yaml", host(r2.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml", "r2.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let (a, b) = tokio::join!(
        banner(port(&tunnels, "r1.yaml")),
        banner(port(&tunnels, "r2.yaml"))
    );
    assert!(a.starts_with("SSH-2.0-") && b.starts_with("SSH-2.0-"));
    banner(port(&tunnels, "r1.yaml")).await;
    assert_eq!(bastion.connections(), 1);
    assert_eq!(bastion.forwarded(), 3);
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn hosts_that_cannot_be_tunnelled_are_refused_before_connecting() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let untrusted = TestSshServer::start_device("r9").await;
    let (_temp, ws) = workspace(&[
        // A jump host with no credential profile.
        ("open.yaml", host(bastion.port(), "")),
        ("lab/_folder.yaml", "credential_profile: lab\n".into()),
        ("lab/bastion.yaml", host(bastion.port(), "")),
        ("lab/direct.yaml", host(router.port(), "")),
        (
            "lab/no-login.yaml",
            host(router.port(), "jump_host: open.yaml\n"),
        ),
        (
            "lab/missing.yaml",
            host(router.port(), "jump_host: nowhere.yaml\n"),
        ),
        (
            "lab/telnet.yaml",
            host(23, "protocol: telnet\njump_host: lab/bastion.yaml\n"),
        ),
        (
            "lab/serial.yaml",
            "address: x\nprotocol: serial\njump_host: lab/bastion.yaml\n".into(),
        ),
        (
            "lab/unknown-key.yaml",
            host(untrusted.port(), "jump_host: lab/bastion.yaml\n"),
        ),
    ]);
    let store = store(&ws);
    // Only the jump hosts' and the router's keys are trusted.
    let known_hosts = KnownHosts::in_memory();
    known_hosts
        .remember("127.0.0.1", bastion.port(), bastion.host_key())
        .unwrap();
    known_hosts
        .remember("127.0.0.1", router.port(), router.host_key())
        .unwrap();
    let all = [
        "lab/direct.yaml",
        "lab/no-login.yaml",
        "lab/missing.yaml",
        "lab/telnet.yaml",
        "lab/serial.yaml",
        "lab/unknown-key.yaml",
    ];
    // Extra ports change nothing about who is refused.
    let ports = [830, 443];
    let plan = tunnel::plan(
        &ws,
        &Profiles::new(&store, &ws),
        &known_hosts,
        &ids(&all),
        &ports,
    );
    let planned: Vec<&str> = plan.hosts.iter().map(|p| p.host().as_str()).collect();
    assert_eq!(
        planned,
        [
            "lab/no-login.yaml",
            "lab/missing.yaml",
            "lab/telnet.yaml",
            "lab/unknown-key.yaml"
        ],
        "direct and serial hosts are not behind jump hosts"
    );
    assert!(plan
        .hosts
        .iter()
        .all(|p| matches!(p, Planned::Refused { .. })));
    let (tunnels, _) = open_with_ports(
        &ws,
        &store,
        known_hosts,
        &all,
        &ports,
        TunnelOptions::default(),
    )
    .await;
    assert_eq!(bastion.connections(), 0, "nothing was connected");
    assert!(
        tunnels
            .status()
            .iter()
            .all(|status| matches!(status.state, TunnelState::Refused { .. })),
        "no listener for any port: {:?}",
        tunnels.status()
    );

    let reason = refusal(&tunnels, "lab/no-login.yaml");
    assert!(
        reason.contains("jump host open.yaml has no stored credential profile"),
        "{reason}"
    );
    let reason = refusal(&tunnels, "lab/missing.yaml");
    assert!(reason.contains("jump hosts cannot be used"), "{reason}");
    assert!(reason.contains("nowhere.yaml"), "{reason}");
    let reason = refusal(&tunnels, "lab/telnet.yaml");
    assert!(reason.contains("only SSH is tunnelled"), "{reason}");
    let reason = refusal(&tunnels, "lab/unknown-key.yaml");
    assert!(
        reason.contains(&format!(
            "has not trusted its host key (at 127.0.0.1 port {})",
            untrusted.port()
        )),
        "{reason}"
    );
    assert!(reason.contains("connect to it once in the app"), "{reason}");
}

/// An untrusted, then a changed jump-host key: refused, naming the key;
/// nothing is remembered.
#[tokio::test]
async fn a_jump_host_key_not_trusted_is_refused_naming_it() {
    let bastion = TestSshServer::start().await;
    let other = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let known_hosts = KnownHosts::in_memory();
    known_hosts
        .remember("127.0.0.1", router.port(), router.host_key())
        .unwrap();
    let fingerprint = bastion
        .host_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();

    let (tunnels, _) = open(
        &ws,
        &store,
        known_hosts.clone(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let reason = refusal(&tunnels, "r1.yaml");
    assert!(reason.contains("not trusted yet"), "{reason}");
    assert!(reason.contains(&fingerprint), "{reason}");
    assert!(reason.contains("bastion.yaml"), "{reason}");
    assert_eq!(bastion.forwarded(), 0);
    assert_eq!(
        known_hosts
            .trusted("127.0.0.1", bastion.port())
            .unwrap()
            .len(),
        0
    );

    known_hosts
        .remember("127.0.0.1", bastion.port(), other.host_key())
        .unwrap();
    let (tunnels, _) = open(
        &ws,
        &store,
        known_hosts.clone(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let reason = refusal(&tunnels, "r1.yaml");
    assert!(reason.contains("not the one trusted"), "{reason}");
    assert!(reason.contains(&fingerprint), "{reason}");
}

#[tokio::test]
async fn a_jump_host_login_that_asks_questions_is_refused() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        (
            "bastion.yaml",
            host(bastion.port(), "credential_profile: otp\n"),
        ),
        (
            "r1.yaml",
            host(
                router.port(),
                "jump_host: bastion.yaml\ncredential_profile: lab\n",
            ),
        ),
    ]);
    let store = store(&ws);
    store
        .put(&ws.credential_scope(), &profile("otp", OTP_USERNAME))
        .unwrap();
    let (tunnels, _) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let reason = refusal(&tunnels, "r1.yaml");
    assert!(reason.contains("bastion.yaml"), "{reason}");
    assert!(reason.contains(support::CODE_QUESTION.trim()), "{reason}");
}

/// The device behind the jump host is down: the connection is closed, and
/// the reason reported.
#[tokio::test]
async fn a_device_the_jump_host_cannot_reach() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let closed_port = support::closed_port().await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(closed_port, "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let known_hosts = support::known_hosts();
    // As if the app had trusted a device there once.
    known_hosts
        .remember("127.0.0.1", closed_port, router.host_key())
        .unwrap();
    let (tunnels, events) = open(
        &ws,
        &store,
        known_hosts,
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let mut socket = TcpStream::connect(("127.0.0.1", port(&tunnels, "r1.yaml")))
        .await
        .unwrap();
    closed(&mut socket).await;
    event_within(&events, Duration::from_secs(5), |event| {
        matches!(event, TunnelEvent::Failed { reason, .. } if reason.contains("could not reach it"))
    })
    .await;
}

/// The jump host drops every connection: the open one closes with a
/// reason, and the next connects the chain again.
#[tokio::test]
async fn a_dropped_jump_connection() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let port = port(&tunnels, "r1.yaml");
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    first_line(&mut socket, Duration::from_secs(10))
        .await
        .expect("a banner");

    bastion.cut_connections().await;
    let started = Instant::now();
    closed(&mut socket).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    event_within(&events, Duration::from_secs(5), |event| {
        matches!(event, TunnelEvent::Failed { host, reason }
            if host.as_str() == "r1.yaml" && reason.contains("jump host bastion.yaml was lost"))
    })
    .await;

    assert!(banner(port).await.starts_with("SSH-2.0-"));
    assert_eq!(bastion.connections(), 2);
    assert!(events.lock().unwrap().iter().any(
        |event| matches!(event, TunnelEvent::ChainConnected { chain } if chain == &["bastion.yaml"])
    ));
}

#[tokio::test]
async fn dropping_the_tunnels_closes_their_ports() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, _) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;
    let port = port(&tunnels, "r1.yaml");
    banner(port).await;
    drop(tunnels);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
}

/// Opens tunnels to a flooding server and another behind one bastion,
/// and a terminal session through the first that floods and never reads
/// again (paused by flow control). The servers, tunnels and events, and
/// the second tunnel's port.
struct Stalled {
    _servers: Vec<TestSshServer>,
    _temp: TempDir,
    tunnels: Tunnels,
    events: Events,
    _manager: enodix_core::session::SessionManager,
    _session: enodix_core::session::SessionEvents,
    other_port: u16,
}

async fn stalled(options: TunnelOptions) -> Stalled {
    let bastion = TestSshServer::start().await;
    let flooder = TestSshServer::start().await;
    let other = TestSshServer::start().await;
    let (temp, ws) = workspace(&[
        (
            "_folder.yaml",
            "credential_profile: lab
"
            .into(),
        ),
        ("bastion.yaml", host(bastion.port(), "")),
        (
            "flooder.yaml",
            host(
                flooder.port(),
                "jump_host: bastion.yaml
",
            ),
        ),
        (
            "other.yaml",
            host(
                other.port(),
                "jump_host: bastion.yaml
",
            ),
        ),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["flooder.yaml", "other.yaml"],
        options,
    )
    .await;
    let manager = support::manager();
    let (id, mut session) = manager.open(
        SessionTarget::Ssh(support::target_at(
            "127.0.0.1",
            port(&tunnels, "flooder.yaml"),
        )),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(
        support::next_state(&mut session).await,
        SessionState::Connecting
    );
    support::next_host_key_question(&mut session).await;
    manager
        .decide_host_key(id, HostKeyDecision::TrustOnce)
        .unwrap();
    assert_eq!(
        support::next_state(&mut session).await,
        SessionState::Connected
    );
    manager.write(id, b"flood\r".to_vec()).unwrap();
    // Never read again: the session stops reading after its window, and
    // so stops reading the tunnel.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let other_port = port(&tunnels, "other.yaml");
    Stalled {
        _servers: vec![bastion, flooder, other],
        _temp: temp,
        tunnels,
        events,
        _manager: manager,
        _session: session,
        other_port,
    }
}

/// A program that stops reading one tunnel, and a second tunnel through
/// the same jump connection: the second still gets through, as each
/// channel's output is drained into its own buffer (russh would otherwise
/// stall the whole jump connection while one channel's output is not
/// taken).
#[tokio::test]
async fn a_stalled_reader_does_not_block_its_jump_connection() {
    let stalled = stalled(TunnelOptions::default()).await;
    let started = Instant::now();
    let got = try_banner(stalled.other_port, Duration::from_secs(10)).await;
    assert!(
        got.is_some_and(|banner| banner.starts_with("SSH-2.0-")),
        "the second tunnel was blocked"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // The stalled one is still open: what waits is under the limit.
    assert!(
        stalled.events.lock().unwrap().is_empty(),
        "{:?}",
        stalled.events.lock().unwrap()
    );
    drop(stalled.tunnels);
}

/// Past the buffer's limit, the stalled tunnel's connection is closed,
/// saying why.
///
/// The device here is a plain service that sends far more than the kernel's
/// loopback buffers can hold (Linux grows them to several MiB), and the
/// program never reads. A terminal session as the reader would not do: an
/// SSH server sends only what the session's window allows (a few MiB), and
/// whether that is more than the sockets' buffers plus the limit depends on
/// how far the kernel has grown them, which varies with the computer's
/// load: the limit was then never passed and the test waited out its
/// deadline. The flood is a TCP service on the host's SSH port, which the
/// tunnel carries as it does any bytes.
#[tokio::test]
async fn a_reader_that_lets_too_much_wait_loses_its_connection() {
    let bastion = TestSshServer::start().await;
    let other = TestSshServer::start().await;
    let flooder = TcpService::flood(64 * 1024 * 1024).await;
    // Tunnels to a host need its host key to be trusted: the flooder gets
    // another test server's key (its bytes are never parsed as SSH).
    let known = support::known_hosts();
    for key in known.trusted("127.0.0.1", other.port()).unwrap() {
        known.remember("127.0.0.1", flooder.port(), &key).unwrap();
    }
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        (
            "flooder.yaml",
            host(flooder.port(), "jump_host: bastion.yaml\n"),
        ),
        (
            "other.yaml",
            host(other.port(), "jump_host: bastion.yaml\n"),
        ),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open(
        &ws,
        &store,
        support::known_hosts(),
        &["flooder.yaml", "other.yaml"],
        TunnelOptions {
            buffer_limit: 256 * 1024,
        },
    )
    .await;
    // Connected, and never read.
    let _socket = TcpStream::connect(("127.0.0.1", port(&tunnels, "flooder.yaml")))
        .await
        .unwrap();
    event_within(&events, Duration::from_secs(60), |event| {
        matches!(event, TunnelEvent::Failed { host, reason }
            if host.as_str() == "flooder.yaml" && reason.contains("did not read its output"))
    })
    .await;
    assert!(banner(port(&tunnels, "other.yaml"))
        .await
        .starts_with("SSH-2.0-"));
}

/// Sends a line through `socket` and reads it back from the echo service.
async fn echoed(socket: &mut TcpStream, line: &str) {
    socket
        .write_all(format!("{line}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(
        first_line(socket, Duration::from_secs(10)).await.as_deref(),
        Some(line)
    );
}

/// Connects to the extra port's listener at `port` and reads the echo
/// service's banner.
async fn service(port: u16) -> (TcpStream, String) {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let banner = first_line(&mut socket, Duration::from_secs(10))
        .await
        .expect("the service's banner within 10 s");
    (socket, banner)
}

/// An extra port reaches the device's service on that port through the
/// jump host; the SSH port's tunnel is as before. The SSH port itself and
/// a port named twice add nothing.
#[tokio::test]
async fn an_extra_port_through_one_jump_host() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let netconf = TcpService::echo("netconf").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let ports = [netconf.port(), router.port(), netconf.port()];
    let plan = tunnel::plan(
        &ws,
        &Profiles::new(&store, &ws),
        &support::known_hosts(),
        &ids(&["r1.yaml"]),
        &ports,
    );
    let [Planned::Ready(request)] = plan.hosts.as_slice() else {
        panic!("{plan:?}");
    };
    assert_eq!(request.port, router.port());
    assert_eq!(request.ports, vec![netconf.port()]);

    let (tunnels, events) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        &ports,
        TunnelOptions::default(),
    )
    .await;
    // Nothing reaches the device's service until a program connects.
    assert_eq!(netconf.accepted(), 0);
    let extra = extra_ports(&tunnels, "r1.yaml");
    assert_eq!(extra.keys().copied().collect::<Vec<_>>(), [netconf.port()]);
    let listener = extra[&netconf.port()];
    assert_ne!(listener, port(&tunnels, "r1.yaml"));

    assert!(banner(port(&tunnels, "r1.yaml"))
        .await
        .starts_with("SSH-2.0-"));
    let (mut socket, greeting) = service(listener).await;
    assert_eq!(greeting, "netconf");
    echoed(&mut socket, "<hello/>").await;
    assert_eq!(netconf.accepted(), 1);
    assert_eq!(bastion.connections(), 1);
    assert_eq!(bastion.forwarded(), 2);
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_extra_port_through_a_chain_of_two_jump_hosts() {
    let core = TestSshServer::start().await;
    let site = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let api = TcpService::echo("api").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("core.yaml", host(core.port(), "")),
        ("site.yaml", host(site.port(), "jump_host: core.yaml\n")),
        ("r1.yaml", host(router.port(), "jump_host: site.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, _) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        &[api.port()],
        TunnelOptions::default(),
    )
    .await;
    let (mut socket, greeting) = service(extra_port(&tunnels, "r1.yaml", api.port())).await;
    assert_eq!(greeting, "api");
    echoed(&mut socket, "GET /").await;
    // The core forwarded to the site (connecting the chain), the site to
    // the router's service.
    assert_eq!(core.forwarded(), 1);
    assert_eq!(site.forwarded(), 1);
}

#[tokio::test]
async fn hosts_behind_one_jump_host_share_its_connection_whatever_the_port() {
    let bastion = TestSshServer::start().await;
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let netconf = TcpService::echo("netconf").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(r1.port(), "jump_host: bastion.yaml\n")),
        ("r2.yaml", host(r2.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml", "r2.yaml"],
        &[netconf.port()],
        TunnelOptions::default(),
    )
    .await;
    let (a, b, (_c, c), (_d, d)) = tokio::join!(
        banner(port(&tunnels, "r1.yaml")),
        banner(port(&tunnels, "r2.yaml")),
        service(extra_port(&tunnels, "r1.yaml", netconf.port())),
        service(extra_port(&tunnels, "r2.yaml", netconf.port())),
    );
    assert!(a.starts_with("SSH-2.0-") && b.starts_with("SSH-2.0-"));
    assert_eq!((c.as_str(), d.as_str()), ("netconf", "netconf"));
    assert_eq!(bastion.connections(), 1);
    assert_eq!(bastion.forwarded(), 4);
    assert!(events.lock().unwrap().is_empty());
}

/// A port nothing listens on at the device: that connection is closed at
/// once, the reason names the host and the port, and the other listeners
/// and connections go on.
#[tokio::test]
async fn a_port_closed_on_the_device() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let netconf = TcpService::echo("netconf").await;
    let closed_port = support::closed_port().await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        &[netconf.port(), closed_port],
        TunnelOptions::default(),
    )
    .await;
    let (mut open_one, _) = service(extra_port(&tunnels, "r1.yaml", netconf.port())).await;

    let started = Instant::now();
    let mut socket =
        TcpStream::connect(("127.0.0.1", extra_port(&tunnels, "r1.yaml", closed_port)))
            .await
            .unwrap();
    closed(&mut socket).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    event_within(&events, Duration::from_secs(5), |event| {
        matches!(event, TunnelEvent::Failed { host, reason }
            if host.as_str() == "r1.yaml"
                && reason.contains(&format!("port {closed_port}"))
                && reason.contains("could not reach it"))
    })
    .await;
    let shown = events.lock().unwrap()[0].to_string();
    assert!(
        shown.starts_with(&format!("r1.yaml: port {closed_port}: ")),
        "{shown}"
    );

    echoed(&mut open_one, "still here").await;
    let (_, greeting) = service(extra_port(&tunnels, "r1.yaml", netconf.port())).await;
    assert_eq!(greeting, "netconf");
    assert!(banner(port(&tunnels, "r1.yaml"))
        .await
        .starts_with("SSH-2.0-"));
    assert_eq!(bastion.connections(), 1);
}

/// The jump host drops every connection: the extra port's connection
/// closes with today's reason, naming the port, and the next connects the
/// chain again.
#[tokio::test]
async fn a_dropped_jump_connection_on_an_extra_port() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let netconf = TcpService::echo("netconf").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        &[netconf.port()],
        TunnelOptions::default(),
    )
    .await;
    let listener = extra_port(&tunnels, "r1.yaml", netconf.port());
    let (mut socket, _) = service(listener).await;

    bastion.cut_connections().await;
    let started = Instant::now();
    closed(&mut socket).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let wanted = format!(
        "port {}: the connection to the jump host bastion.yaml was lost",
        netconf.port()
    );
    event_within(&events, Duration::from_secs(5), |event| {
        matches!(event, TunnelEvent::Failed { host, reason }
            if host.as_str() == "r1.yaml" && reason == &wanted)
    })
    .await;

    let (_, greeting) = service(listener).await;
    assert_eq!(greeting, "netconf");
    assert_eq!(bastion.connections(), 2);
    assert!(events.lock().unwrap().iter().any(
        |event| matches!(event, TunnelEvent::ChainConnected { chain } if chain == &["bastion.yaml"])
    ));
}

#[tokio::test]
async fn dropping_the_tunnels_closes_every_listener() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let netconf = TcpService::echo("netconf").await;
    let api = TcpService::echo("api").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, _) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml"],
        &[netconf.port(), api.port()],
        TunnelOptions::default(),
    )
    .await;
    let mut listeners = vec![port(&tunnels, "r1.yaml")];
    listeners.extend(extra_ports(&tunnels, "r1.yaml").into_values());
    assert_eq!(listeners.len(), 3);
    let (mut held, _) = service(extra_port(&tunnels, "r1.yaml", api.port())).await;
    drop(tunnels);
    closed(&mut held).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    for listener in listeners {
        assert!(
            TcpStream::connect(("127.0.0.1", listener)).await.is_err(),
            "port {listener} still listens"
        );
    }
}

/// Tunnels to `r1`, whose extra port floods, and to `r2`, behind one
/// bastion, with a connection to r1's extra port that is never read.
struct StalledPort {
    _servers: Vec<TestSshServer>,
    _flooder: TcpService,
    _temp: TempDir,
    _socket: TcpStream,
    tunnels: Tunnels,
    events: Events,
    flood_port: u16,
    /// r2's SSH port's listener.
    other_port: u16,
}

/// Opens [`StalledPort`]: r1's extra port sends `flood` bytes to each
/// connection.
async fn stalled_port(flood: usize, options: TunnelOptions) -> StalledPort {
    let bastion = TestSshServer::start().await;
    let r1 = TestSshServer::start_device("r1").await;
    let r2 = TestSshServer::start_device("r2").await;
    let flooder = TcpService::flood(flood).await;
    let (temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(r1.port(), "jump_host: bastion.yaml\n")),
        ("r2.yaml", host(r2.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let (tunnels, events) = open_with_ports(
        &ws,
        &store,
        support::known_hosts(),
        &["r1.yaml", "r2.yaml"],
        &[flooder.port()],
        options,
    )
    .await;
    let socket = TcpStream::connect(("127.0.0.1", extra_port(&tunnels, "r1.yaml", flooder.port())))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let other_port = port(&tunnels, "r2.yaml");
    StalledPort {
        _servers: vec![bastion, r1, r2],
        flood_port: flooder.port(),
        _flooder: flooder,
        _temp: temp,
        _socket: socket,
        tunnels,
        events,
        other_port,
    }
}

/// A program that stops reading an extra port's tunnel does not block the
/// jump connection the other tunnels share.
#[tokio::test]
async fn a_stalled_reader_on_an_extra_port_does_not_block_its_jump_connection() {
    // Under the default limit, and more than russh's own buffers hold.
    let stalled = stalled_port(6 * 1024 * 1024, TunnelOptions::default()).await;
    let started = Instant::now();
    let got = try_banner(stalled.other_port, Duration::from_secs(10)).await;
    assert!(
        got.is_some_and(|banner| banner.starts_with("SSH-2.0-")),
        "the second tunnel was blocked"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        stalled.events.lock().unwrap().is_empty(),
        "{:?}",
        stalled.events.lock().unwrap()
    );
    drop(stalled.tunnels);
}

/// Past the buffer's limit, the extra port's connection is closed, saying
/// why and naming the port.
#[tokio::test]
async fn a_reader_on_an_extra_port_that_lets_too_much_wait_loses_its_connection() {
    let stalled = stalled_port(
        64 * 1024 * 1024,
        TunnelOptions {
            buffer_limit: 256 * 1024,
        },
    )
    .await;
    let port_named = format!("port {}: ", stalled.flood_port);
    event_within(&stalled.events, Duration::from_secs(20), |event| {
        matches!(event, TunnelEvent::Failed { host, reason }
            if host.as_str() == "r1.yaml"
                && reason.starts_with(&port_named)
                && reason.contains("did not read its output"))
    })
    .await;
    assert!(banner(stalled.other_port).await.starts_with("SSH-2.0-"));
}

/// A device key that was trusted and is revoked since is not handed to the
/// script as a key to accept: the tunnel is refused, saying it is revoked.
#[tokio::test]
async fn a_device_key_revoked_after_it_was_trusted_is_refused_as_revoked() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start_device("r1").await;
    let (_temp, ws) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("bastion.yaml", host(bastion.port(), "")),
        ("r1.yaml", host(router.port(), "jump_host: bastion.yaml\n")),
    ]);
    let store = store(&ws);
    let data = TempDir::new().unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());
    known_hosts
        .remember("127.0.0.1", bastion.port(), bastion.host_key())
        .unwrap();
    known_hosts
        .remember("127.0.0.1", router.port(), router.host_key())
        .unwrap();
    let file = data.path().join(enodix_core::known_hosts::FILE_NAME);
    let mut text = fs::read_to_string(&file).unwrap();
    text.push_str(&format!(
        "@revoked * {}\n",
        router.host_key().to_openssh().unwrap()
    ));
    fs::write(&file, text).unwrap();
    assert!(known_hosts
        .trusted("127.0.0.1", router.port())
        .unwrap()
        .is_empty());

    let (tunnels, _) = open(
        &ws,
        &store,
        known_hosts,
        &["r1.yaml"],
        TunnelOptions::default(),
    )
    .await;

    let reason = refusal(&tunnels, "r1.yaml");
    assert!(reason.contains("is revoked"), "{reason}");
    assert!(
        !reason.contains("connect to it once in the app and trust"),
        "{reason}"
    );
}
