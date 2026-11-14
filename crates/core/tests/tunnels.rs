//! Loopback tunnels through jump chains (`netterm_core::tunnel`), against
//! the in-process test servers. Most checks read the device's SSH banner
//! through the tunnel and count what the jump hosts forwarded.

mod support;

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::{HostKeyDecision, SessionState, SessionTarget, TerminalSize};
use netterm_core::tunnel::{
    self, Planned, TunnelEvent, TunnelOptions, TunnelState, TunnelStatus, Tunnels,
};
use netterm_core::workspace::{HostId, Workspace};
use support::{TestSshServer, OTP_USERNAME, PASSWORD, USERNAME};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
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
    let plan = tunnel::plan(
        workspace,
        &Profiles::new(store, workspace),
        &known_hosts,
        &ids(hosts),
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
    let plan = tunnel::plan(&ws, &Profiles::new(&store, &ws), &known_hosts, &ids(&all));
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
    let (tunnels, _) = open(&ws, &store, known_hosts, &all, TunnelOptions::default()).await;
    assert_eq!(bastion.connections(), 0, "nothing was connected");

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
    _manager: netterm_core::session::SessionManager,
    _session: netterm_core::session::SessionEvents,
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
#[tokio::test]
async fn a_reader_that_lets_too_much_wait_loses_its_connection() {
    let stalled = stalled(TunnelOptions {
        buffer_limit: 256 * 1024,
    })
    .await;
    event_within(&stalled.events, Duration::from_secs(20), |event| {
        matches!(event, TunnelEvent::Failed { host, reason }
            if host.as_str() == "flooder.yaml" && reason.contains("did not read its output"))
    })
    .await;
    assert!(banner(stalled.other_port).await.starts_with("SSH-2.0-"));
}
