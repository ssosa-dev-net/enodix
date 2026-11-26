//! Port forwards over a terminal session's connection (#21): local ones
//! (`ssh -L`) and dynamic ones (SOCKS5, `ssh -D`), against the in-process
//! test server, which carries `direct-tcpip` channels to a TCP service on
//! the loopback address. Also the `forwards` setting of the workspace.

mod support;

use std::fs;
use std::time::Duration;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::session::{
    ForwardKind, ForwardState, ForwardStatus, OpenOptions, PortForward, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshTarget, TerminalSize,
    SOCKS_HANDSHAKE_TIMEOUT,
};
use enodix_core::workspace::{ForwardSetting, HostId, Settings, Workspace};
use support::tcp::TcpService;
use support::{next_state, TestSshServer, PASSWORD, USERNAME};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };
const WAIT: Duration = Duration::from_secs(10);

fn forward(name: &str, text: &str) -> PortForward {
    PortForward::parse(name, text).unwrap()
}

/// Opens a session to `target` with `forwards`, once it has connected.
async fn open(
    manager: &SessionManager,
    target: SshTarget,
    forwards: Vec<PortForward>,
) -> (SessionId, SessionEvents) {
    let options = OpenOptions {
        forwards,
        ..OpenOptions::default()
    };
    let (id, mut events) = manager.open_with(SessionTarget::Ssh(target), SIZE, options);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    (id, events)
}

/// The next report of the forwards, output taken on the way.
async fn next_forwards(events: &mut SessionEvents) -> Vec<ForwardStatus> {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Forwards(forwards)) => return forwards,
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                Some(SessionEvent::State(SessionState::Closed(reason))) => {
                    panic!("the session closed ({reason}) while waiting for its forwards")
                }
                Some(_) => {}
                None => panic!("the events ended while waiting for the forwards"),
            }
        }
    })
    .await
    .expect("timed out waiting for the forwards")
}

/// The port a forward listens on, which must be listening.
fn listening(status: &ForwardStatus) -> u16 {
    match &status.state {
        ForwardState::Listening { address, port } => {
            assert_eq!(address, "127.0.0.1");
            *port
        }
        other => panic!("{} is not listening: {other:?}", status.name),
    }
}

/// The forwards of `id`, by the manager.
fn statuses(manager: &SessionManager, id: SessionId) -> Vec<ForwardStatus> {
    manager.forwards(id).unwrap()
}

/// Reads the banner line an echo service sends, through `socket`.
async fn banner(socket: &mut TcpStream) -> String {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    tokio::time::timeout(WAIT, async {
        while socket.read(&mut byte).await.unwrap() == 1 {
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
        }
    })
    .await
    .expect("timed out reading the banner");
    String::from_utf8(line).unwrap()
}

/// What `socket` reads until the other side closes it.
async fn until_closed(socket: &mut TcpStream) -> Vec<u8> {
    let mut rest = Vec::new();
    tokio::time::timeout(WAIT, socket.read_to_end(&mut rest))
        .await
        .expect("the connection stayed open")
        .ok();
    rest
}

/// Echoes `text` through `socket`.
async fn echoes(socket: &mut TcpStream, text: &[u8]) {
    socket.write_all(text).await.unwrap();
    let mut back = vec![0; text.len()];
    tokio::time::timeout(WAIT, socket.read_exact(&mut back))
        .await
        .expect("timed out waiting for the echo")
        .unwrap();
    assert_eq!(back, text);
}

/// A SOCKS5 connection through `port` to `host` (a name, or an IPv4
/// address) and `to`: the reply code.
async fn socks(port: u16, host: &str, to: u16) -> (TcpStream, u8) {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(&[5, 1, 0]).await.unwrap();
    let mut chosen = [0u8; 2];
    socket.read_exact(&mut chosen).await.unwrap();
    assert_eq!(chosen, [5, 0], "no authentication");
    let mut request = vec![5, 1, 0];
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(address) => {
            request.push(1);
            request.extend_from_slice(&address.octets());
        }
        Err(_) => {
            request.push(3);
            request.push(host.len() as u8);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&to.to_be_bytes());
    socket.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(WAIT, socket.read_exact(&mut reply))
        .await
        .expect("timed out waiting for the SOCKS reply")
        .unwrap();
    assert_eq!(reply[0], 5);
    (socket, reply[1])
}

#[tokio::test]
async fn a_local_forward_reaches_a_service_as_the_device_does() {
    let server = TestSshServer::start().await;
    let service = TcpService::echo("web gui").await;
    let manager = support::manager();
    let spec = format!("L 0:127.0.0.1:{}", service.port());
    let (id, _events) = open(&manager, server.target(), vec![forward("gui", &spec)]).await;

    // Listening before the session counts as connected.
    let reported = statuses(&manager, id);
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].name, "gui");
    assert_eq!(reported[0].spec, spec);
    let port = listening(&reported[0]);

    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert_eq!(banner(&mut socket).await, "web gui");
    echoes(&mut socket, b"GET / HTTP/1.0\r\n").await;
    // A second connection at once, on the same forward.
    let mut second = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert_eq!(banner(&mut second).await, "web gui");
    assert_eq!(server.forwarded(), 2);
    // Through the session's connection: nothing logged in again.
    assert_eq!(server.logins(), 1);
    assert_eq!(server.connections_open(), 1);
}

#[tokio::test]
async fn a_dynamic_forward_is_a_socks5_proxy_through_the_device() {
    let server = TestSshServer::start().await;
    let service = TcpService::echo("api").await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target(), vec![forward("proxy", "D 0")]).await;
    let reported = statuses(&manager, id);
    assert_eq!(reported[0].spec, "D 0");
    let port = listening(&reported[0]);

    // By address, and by name (resolved by the device).
    for host in ["127.0.0.1", "localhost"] {
        let (mut socket, reply) = socks(port, host, service.port()).await;
        assert_eq!(reply, 0, "succeeded for {host}");
        assert_eq!(banner(&mut socket).await, "api");
        echoes(&mut socket, b"ping").await;
    }
    assert_eq!(server.forwarded(), 2);

    // A destination the device cannot reach: refused, and said so.
    let closed = support::closed_port().await;
    let (mut socket, reply) = socks(port, "127.0.0.1", closed).await;
    assert_eq!(reply, 5, "connection refused");
    assert!(until_closed(&mut socket).await.is_empty());
    let reported = next_forwards(&mut events).await;
    let failure = reported[0].last_failure.as_deref().unwrap();
    assert!(
        failure.contains(&format!("could not reach 127.0.0.1 port {closed}")),
        "{failure}"
    );
    // The proxy goes on.
    assert!(reported[0].is_listening());
}

#[tokio::test]
async fn a_socks_client_that_asks_for_something_else_is_answered_no() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, _events) = open(&manager, server.target(), vec![forward("proxy", "D 0")]).await;
    let port = listening(&statuses(&manager, id)[0]);

    // A BIND request (2): not supported.
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(&[5, 1, 0]).await.unwrap();
    let mut chosen = [0u8; 2];
    socket.read_exact(&mut chosen).await.unwrap();
    socket
        .write_all(&[5, 2, 0, 1, 127, 0, 0, 1, 0, 80])
        .await
        .unwrap();
    let rest = until_closed(&mut socket).await;
    assert_eq!(rest[..2], [5, 7]);

    // Only a username and password on offer: no acceptable method.
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket.write_all(&[5, 1, 2]).await.unwrap();
    assert_eq!(until_closed(&mut socket).await, [5, 0xff]);
    assert_eq!(server.forwarded(), 0);
}

#[tokio::test]
async fn a_socks_client_that_says_nothing_is_closed_after_a_while() {
    let server = TestSshServer::start().await;
    let service = TcpService::echo("api").await;
    let manager = support::manager();
    let (id, _events) = open(&manager, server.target(), vec![forward("proxy", "D 0")]).await;
    let port = listening(&statuses(&manager, id)[0]);

    // One silent, one that stops after a byte of its greeting.
    let mut silent = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut halfway = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    halfway.write_all(&[5]).await.unwrap();
    let started = std::time::Instant::now();
    for socket in [&mut silent, &mut halfway] {
        let mut rest = Vec::new();
        tokio::time::timeout(
            SOCKS_HANDSHAKE_TIMEOUT + WAIT,
            socket.read_to_end(&mut rest),
        )
        .await
        .expect("still open after the handshake timeout")
        .ok();
        assert!(rest.is_empty());
    }
    assert!(started.elapsed() >= SOCKS_HANDSHAKE_TIMEOUT - Duration::from_millis(500));
    // A client that does speak still gets through.
    let (mut socket, reply) = socks(port, "127.0.0.1", service.port()).await;
    assert_eq!(reply, 0);
    assert_eq!(banner(&mut socket).await, "api");
}

#[tokio::test]
async fn a_device_that_refuses_forwarding_says_so_in_words() {
    let server = TestSshServer::start().await;
    server.quirks().refuse_forwarding();
    let service = TcpService::echo("web gui").await;
    let manager = support::manager();
    let spec = format!("L 0:127.0.0.1:{}", service.port());
    let forwards = vec![forward("gui", &spec), forward("proxy", "D 0")];
    let (id, mut events) = open(&manager, server.target(), forwards).await;
    let reported = statuses(&manager, id);
    let (gui, proxy) = (listening(&reported[0]), listening(&reported[1]));

    let mut socket = TcpStream::connect(("127.0.0.1", gui)).await.unwrap();
    assert!(until_closed(&mut socket).await.is_empty());
    let reported = next_forwards(&mut events).await;
    assert_eq!(
        reported[0].last_failure.as_deref(),
        Some(
            format!(
                "the device could not reach 127.0.0.1 port {} (it does not allow forwarding)",
                service.port()
            )
            .as_str()
        )
    );
    assert_eq!(reported[1].last_failure, None);

    let (_socket, reply) = socks(proxy, "127.0.0.1", service.port()).await;
    assert_eq!(reply, 2, "not allowed by the ruleset");
    let reported = next_forwards(&mut events).await;
    assert!(reported[1]
        .last_failure
        .as_deref()
        .unwrap()
        .ends_with("(it does not allow forwarding)"));
    assert_eq!(service.accepted(), 0);
}

#[tokio::test]
async fn a_forward_can_be_stopped_and_the_session_goes_on() {
    let server = TestSshServer::start().await;
    let service = TcpService::echo("web gui").await;
    let manager = support::manager();
    let spec = format!("L 0:127.0.0.1:{}", service.port());
    let forwards = vec![forward("gui", &spec), forward("proxy", "D 0")];
    let (id, mut events) = open(&manager, server.target(), forwards).await;
    let reported = statuses(&manager, id);
    let gui = listening(&reported[0]);
    let mut carried = TcpStream::connect(("127.0.0.1", gui)).await.unwrap();
    assert_eq!(banner(&mut carried).await, "web gui");
    assert_eq!(
        manager.all_forwards(),
        vec![(id, statuses(&manager, id))],
        "every open forward, by session"
    );

    assert_eq!(manager.stop_forward(id, "gui").await, Ok(true));
    let reported = next_forwards(&mut events).await;
    assert_eq!(reported[0].state, ForwardState::Stopped);
    assert!(reported[1].is_listening());
    // It listens no more, and what it carried has ended.
    assert!(TcpStream::connect(("127.0.0.1", gui)).await.is_err());
    assert!(until_closed(&mut carried).await.is_empty());
    // Once only; an unknown one is not there to stop.
    assert_eq!(manager.stop_forward(id, "gui").await, Ok(false));
    assert_eq!(manager.stop_forward(id, "nothing").await, Ok(false));

    // The terminal is still there.
    manager.write(id, b"show clock\r".to_vec()).unwrap();
    support::output_ending_with(&mut events, &server.prompt()).await;
}

#[tokio::test]
async fn forwards_end_with_the_session_and_open_again_on_reconnect() {
    let server = TestSshServer::start().await;
    let service = TcpService::echo("web gui").await;
    let manager = support::manager();
    let spec = format!("L 0:127.0.0.1:{}", service.port());
    let (id, mut events) = open(&manager, server.target(), vec![forward("gui", &spec)]).await;
    let port = listening(&statuses(&manager, id)[0]);

    manager.close(id).unwrap();
    assert_eq!(next_forwards(&mut events).await, Vec::new());
    assert!(matches!(
        next_state(&mut events).await,
        SessionState::Closed(_)
    ));
    assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    assert!(manager.all_forwards().is_empty());

    let mut events = manager.reconnect(id, SIZE).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    let port = listening(&statuses(&manager, id)[0]);
    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert_eq!(banner(&mut socket).await, "web gui");
}

#[tokio::test]
async fn a_port_in_use_fails_that_forward_alone() {
    let server = TestSshServer::start().await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let manager = support::manager();
    let forwards = vec![
        forward("taken", &format!("D {port}")),
        forward("free", "D 0"),
    ];
    let (id, _events) = open(&manager, server.target(), forwards).await;
    let reported = statuses(&manager, id);
    assert_eq!(
        reported[0].state,
        ForwardState::Failed {
            reason: format!("cannot listen on 127.0.0.1:{port}: the port is already in use")
        }
    );
    assert!(reported[1].is_listening());
}

#[tokio::test]
async fn a_duplicate_and_a_second_channel_forward_nothing() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, _events) = open(&manager, server.target(), vec![forward("proxy", "D 0")]).await;
    assert!(statuses(&manager, id)[0].is_listening());

    let (copy, mut copied) = manager.duplicate(id, SIZE).unwrap();
    assert_eq!(next_state(&mut copied).await, SessionState::Connecting);
    assert_eq!(next_state(&mut copied).await, SessionState::Connected);
    assert!(statuses(&manager, copy).is_empty());

    let (channel, mut second) = manager.open_channel(id, SIZE).unwrap();
    assert_eq!(next_state(&mut second).await, SessionState::Connecting);
    assert_eq!(next_state(&mut second).await, SessionState::Connected);
    assert!(statuses(&manager, channel).is_empty());
    assert_eq!(manager.all_forwards().len(), 1);
}

// Through jump hosts, from the workspace.

fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("forwards");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    (temp, workspace)
}

fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    let profile = CredentialProfile {
        name: "lab".into(),
        username: USERNAME.into(),
        auth: ProfileAuth::Password {
            password: PASSWORD.into(),
        },
        enable_secret: None,
    };
    store.put(&workspace.credential_scope(), &profile).unwrap();
    store
}

#[tokio::test]
async fn a_host_behind_a_jump_host_forwards_from_the_device() {
    let bastion = TestSshServer::start().await;
    let device = TestSshServer::start_device("leaf1").await;
    let service = TcpService::echo("leaf1 gui").await;
    let (_temp, workspace) = workspace(&[
        (
            "_folder.yaml",
            format!(
                "credential_profile: lab\nforwards:\n  gui: L 0:127.0.0.1:{}\n",
                service.port()
            ),
        ),
        (
            "bastion.yaml",
            format!("address: 127.0.0.1\nport: {}\n", bastion.port()),
        ),
        (
            "leaf1.yaml",
            format!(
                "address: 127.0.0.1\nport: {}\njump_host: bastion.yaml\n",
                device.port()
            ),
        ),
    ]);
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    let store = store(&workspace);
    let id = HostId::new("leaf1.yaml");
    let target = Profiles::new(&store, &workspace)
        .profile_target(&id)
        .unwrap();
    let forwards = workspace.resolve_host(&id).unwrap().port_forwards();
    let manager = support::manager();
    let (id, _events) = open(&manager, target, forwards).await;
    let port = listening(&statuses(&manager, id)[0]);

    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    assert_eq!(banner(&mut socket).await, "leaf1 gui");
    // The bastion carried the session; the device carried the forward.
    assert_eq!(bastion.forwarded(), 1);
    assert_eq!(device.forwarded(), 1);
}

#[test]
fn forwards_are_inherited_by_name_and_can_be_turned_off() {
    let (_temp, workspace) = workspace(&[
        (
            "_folder.yaml",
            "forwards:\n  gui: L 8443:10.0.0.1:443\n  proxy: D 1080\n".into(),
        ),
        (
            "site/_folder.yaml",
            "forwards:\n  gui: L 9443:10.0.0.2:443\n".into(),
        ),
        ("site/r1.yaml", "address: 10.0.0.2\n".into()),
        (
            "site/r2.yaml",
            "address: 10.0.0.3\nforwards:\n  proxy: \"off\"\n  netconf: \"-L [::1]:8830:localhost:830\"\n".into(),
        ),
    ]);
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    let specs = |id: &str| -> Vec<(String, String)> {
        workspace
            .resolve_host(&HostId::new(id))
            .unwrap()
            .port_forwards()
            .iter()
            .map(|f| (f.name.clone(), f.spec()))
            .collect()
    };
    assert_eq!(
        specs("site/r1.yaml"),
        [
            ("proxy".to_string(), "D 1080".to_string()),
            ("gui".to_string(), "L 9443:10.0.0.2:443".to_string()),
        ]
    );
    assert_eq!(
        specs("site/r2.yaml"),
        [
            ("gui".to_string(), "L 9443:10.0.0.2:443".to_string()),
            (
                "netconf".to_string(),
                "L [::1]:8830:localhost:830".to_string()
            ),
        ]
    );
    let netconf = &workspace
        .resolve_host(&HostId::new("site/r2.yaml"))
        .unwrap()
        .port_forwards()[1];
    assert_eq!(netconf.bind.to_string(), "::1");
    assert_eq!(
        netconf.kind,
        ForwardKind::Local {
            host: "localhost".into(),
            port: 830
        }
    );
}

#[test]
fn a_forward_that_cannot_be_read_is_a_problem_of_its_file() {
    let (_temp, workspace) = workspace(&[(
        "r1.yaml",
        "address: 10.0.0.1\nforwards:\n  gui: L 8443:10.0.0.1\n".into(),
    )]);
    let problems = format!("{:?}", workspace.problems());
    assert!(problems.contains("forward `gui`"), "{problems}");
    assert!(problems.contains("is not a local forward"), "{problems}");
}

#[test]
fn a_forward_from_the_workspace_listens_on_loopback_only() {
    for (bind, says) in [
        ("*", "every address (`*`)"),
        ("0.0.0.0", "every address (`*`)"),
        ("192.168.1.10", "192.168.1.10"),
        ("[::]", "::"),
    ] {
        let (_temp, loaded) = workspace(&[(
            "r1.yaml",
            format!("address: 10.0.0.1\nforwards:\n  proxy: D {bind}:1080\n"),
        )]);
        let problems = format!("{:?}", loaded.problems());
        assert!(
            problems.contains(&format!("would listen on {says}")),
            "{bind}: {problems}"
        );
        assert!(problems.contains("127.0.0.1 or ::1 only"), "{problems}");
    }
    // Loopback, written any way, is fine.
    let (_temp, loaded) = workspace(&[(
        "r1.yaml",
        "address: 10.0.0.1\nforwards:\n  a: D 1080\n  b: D localhost:1081\n  c: D 127.0.0.1:1082\n  d: D [::1]:1083\n"
            .into(),
    )]);
    assert!(loaded.problems().is_empty(), "{:?}", loaded.problems());
    let forwards = loaded
        .resolve_host(&HostId::new("r1.yaml"))
        .unwrap()
        .port_forwards();
    assert_eq!(forwards.len(), 4);

    // Nor can the app write one.
    let (_temp, mut editing) = workspace(&[("r1.yaml", "address: 10.0.0.1\n".into())]);
    let settings = Settings {
        forwards: vec![ForwardSetting::new("proxy", "D *:1080")],
        ..Settings::default()
    };
    let error = editing
        .write_folder_settings("", &settings)
        .unwrap_err()
        .to_string();
    assert!(error.contains("would listen on every address"), "{error}");
}

#[test]
fn forwards_are_edited_like_web_links() {
    let (temp, mut workspace) = workspace(&[("r1.yaml", "address: 10.0.0.1\n".into())]);
    let file = temp.path().join("forwards/_folder.yaml");
    let settings = Settings {
        forwards: vec![
            ForwardSetting::new(" gui ", " L 8443:10.0.0.1:443 "),
            ForwardSetting::new("", ""),
        ],
        ..Settings::default()
    };
    workspace.write_folder_settings("", &settings).unwrap();
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("forwards:\n  gui: L 8443:10.0.0.1:443\n"),
        "{text}"
    );
    let r1 = workspace.resolve_host(&HostId::new("r1.yaml")).unwrap();
    assert_eq!(r1.port_forwards()[0].spec(), "L 8443:10.0.0.1:443");

    for (forwards, says) in [
        (vec![ForwardSetting::new("gui", "L 1")], "forward `gui`"),
        (
            vec![
                ForwardSetting::new("a", "D 1080"),
                ForwardSetting::new("a", "D 1081"),
            ],
            "appears twice",
        ),
        (vec![ForwardSetting::new("", "D 1080")], "has no name"),
    ] {
        let settings = Settings {
            forwards,
            ..Settings::default()
        };
        let error = workspace
            .write_folder_settings("", &settings)
            .unwrap_err()
            .to_string();
        assert!(error.contains(says), "{error}");
    }

    workspace
        .write_folder_settings("", &Settings::default())
        .unwrap();
    let text = fs::read_to_string(&file).unwrap_or_default();
    assert!(!text.contains("forwards"), "{text}");
}
