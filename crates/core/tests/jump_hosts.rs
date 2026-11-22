//! Jump hosts: sessions to a device tunnelled through one or more other
//! hosts of the workspace (in-process test servers that forward
//! `direct-tcpip` channels), each hop with its own credential profile and
//! host-key check, and failures that name the hop.

mod support;

use std::fs;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, ProfileError, Profiles,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::{
    CloseReason, HostKeyDecision, LoginQuestion, OpenError, SessionEvents, SessionManager,
    SessionState, SessionTarget, SshAuth, SshTarget, TerminalSize,
};
use enodix_core::workspace::{HostId, JumpChainError, Workspace};
use support::{
    closed_port, next_host_key_question, next_prompt, next_state, output_ending_with,
    TestSshServer, CODE_NAME, CODE_QUESTION, ONE_TIME_CODE, OTP_USERNAME, PASSWORD, PROMPT,
    USERNAME,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A workspace made of `files` (relative path, text).
fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("jumps");
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

/// A host file for a server on the loopback address.
fn host(port: u16, extra: &str) -> String {
    format!("address: 127.0.0.1\nport: {port}\n{extra}")
}

fn password_profile(name: &str, password: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: USERNAME.into(),
        auth: ProfileAuth::Password {
            password: password.into(),
        },
        enable_secret: None,
    }
}

/// A store holding `profiles` for `workspace`.
fn store(workspace: &Workspace, profiles: &[CredentialProfile]) -> MemoryStore {
    let store = MemoryStore::new();
    for profile in profiles {
        store.put(&workspace.credential_scope(), profile).unwrap();
    }
    store
}

fn profile_target(store: &MemoryStore, workspace: &Workspace, id: &str) -> SshTarget {
    Profiles::new(store, workspace)
        .profile_target(&HostId::new(id))
        .unwrap()
}

async fn open(manager: &SessionManager, target: SshTarget) -> SessionEvents {
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    events
}

/// Waits for the session to connect and its shell to print a prompt.
async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

async fn failure(events: &mut SessionEvents) -> OpenError {
    match next_state(events).await {
        SessionState::Closed(CloseReason::Failed(error)) => error,
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn jump_host_names(target: &SshTarget) -> Vec<&str> {
    target.jump_hosts.iter().map(|j| j.name.as_str()).collect()
}

/// `Site/router` is reached through `Site/bastion`, which the `Site` folder
/// sets, so the bastion itself (inheriting its own name) connects directly.
/// The bastion logs in with a key only its own server accepts; the router
/// with a password.
#[tokio::test]
async fn a_host_with_a_jump_host_connects_through_it() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (temp, workspace) = workspace(&[
        (
            "Site/_folder.yaml",
            "jump_host: Site/bastion.yaml\ncredential_profile: lab\n".into(),
        ),
        (
            "Site/bastion.yaml",
            host(bastion.port(), "credential_profile: bastion-key\n"),
        ),
        ("Site/router.yaml", host(router.port(), "")),
    ]);
    let key_file = temp.path().join("bastion_key");
    bastion.write_client_key(&key_file, None);
    let store = store(
        &workspace,
        &[
            password_profile("lab", PASSWORD),
            CredentialProfile {
                name: "bastion-key".into(),
                username: USERNAME.into(),
                auth: ProfileAuth::PrivateKey {
                    key_file: key_file.clone(),
                    passphrase: None,
                },
                enable_secret: None,
            },
        ],
    );

    let target = profile_target(&store, &workspace, "Site/router.yaml");
    assert_eq!(target.port, router.port());
    assert_eq!(target.auth, SshAuth::Password(PASSWORD.into()));
    assert_eq!(jump_host_names(&target), ["Site/bastion.yaml"]);
    let jump = &target.jump_hosts[0];
    assert_eq!(jump.port, bastion.port());
    assert_eq!(
        jump.auth,
        SshAuth::PrivateKey {
            key_file,
            passphrase: None
        }
    );

    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    connected(&mut events).await;
    assert_eq!(bastion.forwarded(), 1);
    assert_eq!(router.forwarded(), 0);

    // The session is the router's shell, as without a jump host.
    manager.write(id, b"size\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, PROMPT).await;
    assert!(output.windows(5).any(|w| w == b"80x24"), "{output:?}");
    manager.write(id, b"exit 3\r".to_vec()).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::RemoteExited {
            exit_status: Some(3)
        })
    );

    // The bastion, inheriting itself as jump host, connects directly.
    let bastion_target = profile_target(&store, &workspace, "Site/bastion.yaml");
    assert!(bastion_target.jump_hosts.is_empty());
    let mut events = open(&manager, bastion_target).await;
    connected(&mut events).await;
    assert_eq!(bastion.forwarded(), 1);
}

/// The router is reached through the site bastion, which is reached
/// through the core bastion: core → site → router.
#[tokio::test]
async fn a_chain_of_two_jump_hosts_works() {
    let core = TestSshServer::start().await;
    let site = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("Core/bastion.yaml", host(core.port(), "")),
        ("Site/_folder.yaml", "jump_host: Site/bastion.yaml\n".into()),
        (
            "Site/bastion.yaml",
            host(site.port(), "jump_host: Core/bastion.yaml\n"),
        ),
        ("Site/router.yaml", host(router.port(), "")),
    ]);
    let store = store(&workspace, &[password_profile("lab", PASSWORD)]);

    let target = profile_target(&store, &workspace, "Site/router.yaml");
    assert_eq!(
        jump_host_names(&target),
        ["Core/bastion.yaml", "Site/bastion.yaml"]
    );

    let manager = support::manager();
    let mut events = open(&manager, target).await;
    connected(&mut events).await;
    assert_eq!(core.forwarded(), 1);
    assert_eq!(site.forwarded(), 1);
    assert_eq!(router.forwarded(), 0);
}

/// Each hop's host key is checked against its own address and port, and
/// asked about in the order the hops are connected.
#[tokio::test]
async fn each_hop_has_its_host_key_checked() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        (
            "Site/_folder.yaml",
            "jump_host: Site/bastion.yaml\ncredential_profile: lab\n".into(),
        ),
        ("Site/bastion.yaml", host(bastion.port(), "")),
        ("Site/router.yaml", host(router.port(), "")),
    ]);
    let store = store(&workspace, &[password_profile("lab", PASSWORD)]);
    let manager = SessionManager::new(KnownHosts::in_memory());

    let (id, mut events) = manager.open(
        SessionTarget::Ssh(profile_target(&store, &workspace, "Site/router.yaml")),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let question = next_host_key_question(&mut events).await;
    assert_eq!(question.port, bastion.port());
    assert_eq!(
        question.key.fingerprint,
        bastion
            .host_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string()
    );
    manager
        .decide_host_key(id, HostKeyDecision::TrustAndRemember)
        .unwrap();
    let question = next_host_key_question(&mut events).await;
    assert_eq!(question.host, "127.0.0.1");
    assert_eq!(question.port, router.port());
    manager
        .decide_host_key(id, HostKeyDecision::TrustOnce)
        .unwrap();
    connected(&mut events).await;

    // The bastion's key was remembered, so only the router's (trusted
    // once) is asked about again; refusing it refuses the device.
    let (id, mut events) = manager.open(
        SessionTarget::Ssh(profile_target(&store, &workspace, "Site/router.yaml")),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let question = next_host_key_question(&mut events).await;
    assert_eq!(question.port, router.port());
    manager
        .decide_host_key(id, HostKeyDecision::Cancel)
        .unwrap();
    assert_eq!(failure(&mut events).await, OpenError::HostKeyNotTrusted);

    // Refusing a jump host's key names the jump host.
    let manager = SessionManager::new(KnownHosts::in_memory());
    let (id, mut events) = manager.open(
        SessionTarget::Ssh(profile_target(&store, &workspace, "Site/router.yaml")),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let question = next_host_key_question(&mut events).await;
    assert_eq!(question.port, bastion.port());
    manager
        .decide_host_key(id, HostKeyDecision::Cancel)
        .unwrap();
    assert_eq!(
        failure(&mut events).await,
        OpenError::JumpHost {
            name: "Site/bastion.yaml".into(),
            error: Box::new(OpenError::HostKeyNotTrusted),
        }
    );
}

/// A jump host that asks for a one-time code says it is the one asking.
#[tokio::test]
async fn a_jump_host_asking_for_a_code_is_named_in_the_question() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = router_behind_bastion(bastion.port(), router.port());
    let store = store(
        &workspace,
        &[
            password_profile("lab", PASSWORD),
            CredentialProfile {
                username: OTP_USERNAME.into(),
                ..password_profile("bastion", PASSWORD)
            },
        ],
    );

    let manager = support::manager();
    let (id, mut events) = manager.open(
        SessionTarget::Ssh(profile_target(&store, &workspace, "Site/router.yaml")),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let prompt = next_prompt(&mut events).await;
    assert_eq!(
        prompt.name,
        format!("Jump host Site/bastion.yaml: {CODE_NAME}")
    );
    assert_eq!(prompt.questions[0].text, CODE_QUESTION);
    manager.answer(id, vec![ONE_TIME_CODE.into()]).unwrap();
    connected(&mut events).await;
}

fn router_behind_bastion(bastion_port: u16, router_port: u16) -> (TempDir, Workspace) {
    workspace(&[
        (
            "Site/_folder.yaml",
            "jump_host: Site/bastion.yaml\ncredential_profile: lab\n".into(),
        ),
        (
            "Site/bastion.yaml",
            host(bastion_port, "credential_profile: bastion\n"),
        ),
        ("Site/router.yaml", host(router_port, "")),
    ])
}

#[tokio::test]
async fn an_unreachable_jump_host_is_named() {
    let router = TestSshServer::start().await;
    let (_temp, workspace) = router_behind_bastion(closed_port().await, router.port());
    let store = store(
        &workspace,
        &[
            password_profile("lab", PASSWORD),
            password_profile("bastion", PASSWORD),
        ],
    );

    let manager = support::manager();
    let mut events = open(
        &manager,
        profile_target(&store, &workspace, "Site/router.yaml"),
    )
    .await;
    let error = failure(&mut events).await;
    match &error {
        OpenError::JumpHost { name, error } => {
            assert_eq!(name, "Site/bastion.yaml");
            assert!(
                matches!(**error, OpenError::ConnectionRefused { .. }),
                "{error:?}"
            );
        }
        other => panic!("expected a jump host failure, got {other:?}"),
    }
    let message = error.to_string();
    assert!(message.contains("Site/bastion.yaml"), "{message}");
    assert!(message.contains("refused the connection"), "{message}");
}

#[tokio::test]
async fn a_jump_host_refusing_the_login_is_named() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = router_behind_bastion(bastion.port(), router.port());
    let store = store(
        &workspace,
        &[
            password_profile("lab", PASSWORD),
            password_profile("bastion", "wrong"),
        ],
    );

    let manager = support::manager();
    let mut events = open(
        &manager,
        profile_target(&store, &workspace, "Site/router.yaml"),
    )
    .await;
    let error = failure(&mut events).await;
    assert_eq!(
        error,
        OpenError::JumpHost {
            name: "Site/bastion.yaml".into(),
            error: Box::new(OpenError::AuthenticationFailed),
        }
    );
    assert_eq!(
        error.to_string(),
        "jump host Site/bastion.yaml: authentication failed"
    );
    assert_eq!(bastion.forwarded(), 0);
}

/// In a chain, the failing hop is the one named: here the second.
#[tokio::test]
async fn the_failing_hop_of_a_chain_is_named() {
    let core = TestSshServer::start().await;
    let site = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: lab\n".into()),
        ("Core/bastion.yaml", host(core.port(), "")),
        ("Site/_folder.yaml", "jump_host: Site/bastion.yaml\n".into()),
        (
            "Site/bastion.yaml",
            host(
                site.port(),
                "jump_host: Core/bastion.yaml\ncredential_profile: site\n",
            ),
        ),
        ("Site/router.yaml", host(router.port(), "")),
    ]);
    let store = store(
        &workspace,
        &[
            password_profile("lab", PASSWORD),
            password_profile("site", "wrong"),
        ],
    );

    let manager = support::manager();
    let mut events = open(
        &manager,
        profile_target(&store, &workspace, "Site/router.yaml"),
    )
    .await;
    assert_eq!(
        failure(&mut events).await,
        OpenError::JumpHost {
            name: "Site/bastion.yaml".into(),
            error: Box::new(OpenError::AuthenticationFailed),
        }
    );
    assert_eq!(core.forwarded(), 1);
}

/// The bastion is fine but cannot reach the device: the device is the one
/// that is unreachable, through that bastion.
#[tokio::test]
async fn a_device_the_jump_host_cannot_reach_is_reported() {
    let bastion = TestSshServer::start().await;
    let (_temp, workspace) = router_behind_bastion(bastion.port(), closed_port().await);
    let store = store(
        &workspace,
        &[
            password_profile("lab", PASSWORD),
            password_profile("bastion", PASSWORD),
        ],
    );

    let manager = support::manager();
    let mut events = open(
        &manager,
        profile_target(&store, &workspace, "Site/router.yaml"),
    )
    .await;
    match failure(&mut events).await {
        OpenError::Connect(reason) => {
            assert!(reason.contains("Site/bastion.yaml"), "{reason}")
        }
        other => panic!("expected a connect failure, got {other:?}"),
    }
}

#[test]
fn a_jump_host_named_by_its_host_name_is_reported_with_its_id() {
    // As a newcomer writes it: the bastion's name, not its id (#116).
    let (_temp, workspace) = workspace(&[
        ("Site/bastion.yaml", host(22, "")),
        ("Site/router.yaml", host(22, "jump_host: bastion\n")),
    ]);
    let router = workspace
        .resolve_host(&HostId::new("Site/router.yaml"))
        .unwrap();

    let error = workspace.jump_chain(&router).unwrap_err();

    assert_eq!(
        error,
        JumpChainError::Missing {
            from: HostId::new("Site/router.yaml"),
            jump_host: "bastion".into(),
            named: Some(HostId::new("Site/bastion.yaml")),
        }
    );
    assert_eq!(
        error.to_string(),
        "the jump host bastion of Site/router.yaml is named by its host name; name it by its id, Site/bastion.yaml, as the Jump host setting lists it"
    );
}

#[test]
fn a_jump_host_missing_from_the_workspace_is_reported() {
    let (_temp, workspace) = workspace(&[
        ("Site/_folder.yaml", "jump_host: Site/gone.yaml\n".into()),
        ("Site/router.yaml", host(22, "")),
    ]);
    let router = workspace
        .resolve_host(&HostId::new("Site/router.yaml"))
        .unwrap();

    let error = workspace.jump_chain(&router).unwrap_err();
    assert_eq!(
        error,
        JumpChainError::Missing {
            from: HostId::new("Site/router.yaml"),
            jump_host: "Site/gone.yaml".into(),
            named: None,
        }
    );
    assert_eq!(
        error.to_string(),
        "the jump host Site/gone.yaml of Site/router.yaml is not in the workspace; choose one the Jump host setting lists"
    );

    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let id = HostId::new("Site/router.yaml");
    assert_eq!(
        profiles.prompted_target(&id, USERNAME.into(), PASSWORD.into()),
        Err(ProfileError::JumpChain(error))
    );
}

#[test]
fn a_loop_of_jump_hosts_is_reported() {
    let (_temp, workspace) = workspace(&[
        ("a.yaml", host(22, "jump_host: b.yaml\n")),
        ("b.yaml", host(22, "jump_host: c.yaml\n")),
        ("c.yaml", host(22, "jump_host: b.yaml\n")),
        // A host naming itself in its own file is a loop too.
        ("d.yaml", host(22, "jump_host: d.yaml\n")),
    ]);
    let resolve = |id: &str| workspace.resolve_host(&HostId::new(id)).unwrap();

    let error = workspace.jump_chain(&resolve("a.yaml")).unwrap_err();
    assert_eq!(
        error,
        JumpChainError::Loop(vec![
            HostId::new("a.yaml"),
            HostId::new("b.yaml"),
            HostId::new("c.yaml"),
            HostId::new("b.yaml"),
        ])
    );
    assert_eq!(
        error.to_string(),
        "the jump hosts form a loop: a.yaml → b.yaml → c.yaml → b.yaml"
    );
    assert_eq!(
        workspace.jump_chain(&resolve("d.yaml")).unwrap_err(),
        JumpChainError::Loop(vec![HostId::new("d.yaml"), HostId::new("d.yaml")])
    );
}

#[test]
fn a_jump_host_reference_ignores_case() {
    let (_temp, workspace) = workspace(&[
        ("Core/Bastion.yaml", host(22, "")),
        ("router.yaml", host(22, "jump_host: core/bastion.YAML\n")),
    ]);
    let router = workspace.resolve_host(&HostId::new("router.yaml")).unwrap();
    let chain = workspace.jump_chain(&router).unwrap();
    let ids: Vec<_> = chain.iter().map(|hop| hop.host.id.clone()).collect();
    assert_eq!(ids, [HostId::new("Core/Bastion.yaml")]);
}

#[test]
fn a_jump_host_that_is_not_reached_over_ssh_is_reported() {
    let (_temp, workspace) = workspace(&[
        ("console.yaml", host(23, "protocol: telnet\n")),
        ("router.yaml", host(22, "jump_host: console.yaml\n")),
    ]);
    let router = workspace.resolve_host(&HostId::new("router.yaml")).unwrap();
    let error = workspace.jump_chain(&router).unwrap_err();
    assert_eq!(
        error,
        JumpChainError::NotSsh {
            jump_host: HostId::new("console.yaml")
        }
    );
    assert_eq!(
        error.to_string(),
        "the jump host console.yaml is not reached over SSH"
    );
}

/// A jump host without a usable profile (none named, or one not set up
/// here) asks for its username and password while connecting, in a
/// question labelled with its name, as a device without one is prompted.
#[tokio::test]
async fn a_jump_host_without_a_usable_profile_asks_for_its_login() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        ("bastion.yaml", host(bastion.port(), "")),
        (
            "keyed.yaml",
            host(
                bastion.port(),
                "credential_profile: bastion
",
            ),
        ),
        (
            "r1.yaml",
            host(
                router.port(),
                "jump_host: bastion.yaml
credential_profile: lab
",
            ),
        ),
        (
            "r2.yaml",
            host(
                router.port(),
                "jump_host: keyed.yaml
credential_profile: lab
",
            ),
        ),
    ]);
    let store = store(&workspace, &[password_profile("lab", PASSWORD)]);
    let manager = support::manager();

    for (id, jump_host) in [("r1.yaml", "bastion.yaml"), ("r2.yaml", "keyed.yaml")] {
        let target = profile_target(&store, &workspace, id);
        assert_eq!(jump_host_names(&target), [jump_host]);
        assert_eq!(target.jump_hosts[0].auth, SshAuth::Prompt);
        assert_eq!(target.jump_hosts[0].username, "");

        let (session, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
        assert_eq!(next_state(&mut events).await, SessionState::Connecting);
        let prompt = next_prompt(&mut events).await;
        assert_eq!(prompt.name, format!("Jump host {jump_host}"));
        assert_eq!(
            prompt.questions,
            [
                LoginQuestion {
                    text: "Username: ".into(),
                    echo: true
                },
                LoginQuestion {
                    text: "Password: ".into(),
                    echo: false
                },
            ]
        );
        manager
            .answer(session, vec![USERNAME.into(), PASSWORD.into()])
            .unwrap();
        connected(&mut events).await;
    }
    assert_eq!(bastion.forwarded(), 2);

    // A wrong answer is the jump host refusing the login.
    let (session, mut events) = manager.open(
        SessionTarget::Ssh(profile_target(&store, &workspace, "r1.yaml")),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    next_prompt(&mut events).await;
    manager
        .answer(session, vec![USERNAME.into(), "wrong".into()])
        .unwrap();
    assert_eq!(
        failure(&mut events).await,
        OpenError::JumpHost {
            name: "bastion.yaml".into(),
            error: Box::new(OpenError::AuthenticationFailed),
        }
    );
}

/// Logging in to the device by hand still goes through its jump host.
#[tokio::test]
async fn a_prompted_login_goes_through_the_jump_host() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        (
            "bastion.yaml",
            host(bastion.port(), "credential_profile: lab\n"),
        ),
        (
            "router.yaml",
            host(router.port(), "jump_host: bastion.yaml\n"),
        ),
    ]);
    let store = store(&workspace, &[password_profile("lab", PASSWORD)]);

    let target = Profiles::new(&store, &workspace)
        .prompted_target(
            &HostId::new("router.yaml"),
            USERNAME.into(),
            PASSWORD.into(),
        )
        .unwrap();
    assert_eq!(jump_host_names(&target), ["bastion.yaml"]);
    let manager = support::manager();
    let mut events = open(&manager, target).await;
    connected(&mut events).await;
    assert_eq!(bastion.forwarded(), 1);
}
