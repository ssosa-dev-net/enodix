//! File connections: a login to a device that opens no terminal, as the
//! Files tab and file transfers use (`enodix_core::files`). Logging in
//! with a stored profile and through a jump host, questions answered by the
//! caller or refused when nobody can answer, a device that hangs up before
//! its banner, a cut connection, closing, and terminal sessions beside it.

mod support;

use std::fs;
use std::time::Duration;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::files::{ConnectError, FileConnection, Lost, OpeningEvent, StartError};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::{
    HostKeyDecision, OpenError, SessionState, SessionTarget, SshAuth, SshTarget, TerminalSize,
};
use enodix_core::workspace::{HostId, Workspace};
use support::{
    known_hosts, manager, next_state, output_ending_with, target_at, HangUpServer, TestSshServer,
    CODE_QUESTION, ONE_TIME_CODE, OTP_USERNAME, PASSWORD, PROMPT, USERNAME, WAIT,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A workspace made of `files` (relative path, text).
fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("files");
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

/// The target of host `id`, logging in with the profile `lab`.
fn profile_target(workspace: &Workspace, id: &str) -> SshTarget {
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: USERNAME.into(),
                auth: ProfileAuth::Password {
                    password: PASSWORD.into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    Profiles::new(&store, workspace)
        .profile_target(&HostId::new(id))
        .unwrap()
}

async fn connected(target: SshTarget) -> FileConnection {
    tokio::time::timeout(WAIT, FileConnection::open_unattended(target, known_hosts()))
        .await
        .expect("the file connection opened in time")
        .expect("the file connection opened")
}

/// Waits until `count()` is `expected`.
async fn until(what: &str, expected: usize, count: impl Fn() -> usize) {
    let waited = tokio::time::timeout(WAIT, async {
        while count() != expected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}: {} instead of {expected}", count());
}

#[tokio::test]
async fn a_file_connection_logs_in_with_a_stored_profile_and_opens_no_shell() {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    let (_temp, workspace) = workspace(&[(
        "router.yaml",
        host(server.port(), "credential_profile: lab\n"),
    )]);

    let connection = connected(profile_target(&workspace, "router.yaml")).await;
    let sftp = connection.start_sftp().await.unwrap();
    sftp.home().await.unwrap();

    assert_eq!(server.logins(), 1);
    assert_eq!(server.shell_requests(), 0, "no shell was asked for");
    assert!(!connection.algorithms().is_empty());
}

#[tokio::test]
async fn a_file_connection_goes_through_a_jump_host() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    router.serve_sftp(folder.path());
    let (_temp, workspace) = workspace(&[
        (
            "Site/_folder.yaml",
            "jump_host: Site/bastion.yaml\ncredential_profile: lab\n".into(),
        ),
        ("Site/bastion.yaml", host(bastion.port(), "")),
        ("Site/router.yaml", host(router.port(), "")),
    ]);
    let target = profile_target(&workspace, "Site/router.yaml");
    assert_eq!(target.jump_hosts.len(), 1);

    let connection = connected(target).await;
    let sftp = connection.start_sftp().await.unwrap();
    sftp.home().await.unwrap();

    assert_eq!(bastion.forwarded(), 1, "reached through the bastion");
    assert_eq!(router.logins(), 1);
    assert_eq!(bastion.shell_requests() + router.shell_requests(), 0);
    assert_eq!(connection.algorithms().len(), 2, "jump host and device");
}

#[tokio::test]
async fn attended_an_unknown_host_key_reaches_the_caller_and_trusting_it_once_asks_again_next_time()
{
    let server = TestSshServer::start().await;
    let known_hosts = KnownHosts::in_memory();

    for _ in 0..2 {
        let mut opening = FileConnection::open(server.target(), known_hosts.clone());
        let question = match opening.next().await {
            Some(OpeningEvent::HostKey(question)) => question,
            other => panic!("expected a host-key question, got {other:?}"),
        };
        assert_eq!(question.port, server.port());
        assert!(!question.changed());
        opening.decide_host_key(HostKeyDecision::TrustOnce);
        match opening.next().await {
            Some(OpeningEvent::Opened(_)) => {}
            other => panic!("expected the connection, got {other:?}"),
        }
        assert_eq!(
            opening.next().await.map(|_| ()),
            None,
            "the opening is over"
        );
    }
    assert_eq!(server.logins(), 2);
}

#[tokio::test]
async fn attended_a_keyboard_interactive_question_reaches_the_caller_and_its_answer_is_used() {
    let server = TestSshServer::start().await;
    let mut target = server.target();
    target.username = OTP_USERNAME.into();

    let mut opening = FileConnection::open(target, known_hosts());
    let prompt = match opening.next().await {
        Some(OpeningEvent::Prompt(prompt)) => prompt,
        other => panic!("expected a login question, got {other:?}"),
    };
    assert_eq!(prompt.questions[0].text, CODE_QUESTION);
    opening.answer(vec![ONE_TIME_CODE.into()]);
    match opening.next().await {
        Some(OpeningEvent::Opened(_)) => {}
        other => panic!("expected the connection, got {other:?}"),
    }
    assert_eq!(server.logins(), 1);
}

/// A file connection has no terminal: a device's login banner (#134)
/// reaches nobody, and the login goes on as without one, attended (the
/// question is the first thing the caller learns) or not.
#[tokio::test]
async fn a_login_banner_reaches_no_one_and_does_not_stop_the_login() {
    let server = TestSshServer::start().await;
    server
        .quirks()
        .auth_banner("AUTHORIZED ACCESS ONLY\nPassword: ");
    let mut target = server.target();
    target.username = OTP_USERNAME.into();

    let mut opening = FileConnection::open(target, known_hosts());
    let prompt = match opening.next().await {
        Some(OpeningEvent::Prompt(prompt)) => prompt,
        other => panic!("expected the login question, got {other:?}"),
    };
    assert_eq!(prompt.questions[0].text, CODE_QUESTION);
    opening.answer(vec![ONE_TIME_CODE.into()]);
    match opening.next().await {
        Some(OpeningEvent::Opened(_)) => {}
        other => panic!("expected the connection, got {other:?}"),
    }

    connected(server.target()).await;
    assert_eq!(server.logins(), 2);
}

#[tokio::test]
async fn attended_a_wrong_answer_fails_the_login() {
    let server = TestSshServer::start().await;
    let mut target = server.target();
    target.username = OTP_USERNAME.into();

    let mut opening = FileConnection::open(target, known_hosts());
    assert!(matches!(
        opening.next().await,
        Some(OpeningEvent::Prompt(_))
    ));
    opening.answer(vec!["000000".into()]);
    match opening.next().await {
        Some(OpeningEvent::Failed(OpenError::AuthenticationFailed)) => {}
        other => panic!("expected a refused login, got {other:?}"),
    }
}

#[tokio::test]
async fn unattended_an_unknown_host_key_is_refused_with_a_reason_naming_it() {
    let server = TestSshServer::start().await;
    let error = FileConnection::open_unattended(server.target(), KnownHosts::in_memory())
        .await
        .expect_err("refused");
    let question = match &error {
        ConnectError::HostKeyRefused(question) => question,
        other => panic!("expected a refused host key, got {other:?}"),
    };
    assert_eq!(question.port, server.port());
    let reason = error.to_string();
    assert!(reason.contains("host key"), "{reason}");
    assert!(reason.contains(&question.key.fingerprint), "{reason}");
    assert!(reason.contains("not trusted"), "{reason}");
    assert_eq!(server.logins(), 0);
}

#[tokio::test]
async fn unattended_a_login_question_is_refused_with_a_reason_naming_it() {
    let server = TestSshServer::start().await;
    let mut target = server.target();
    target.username = OTP_USERNAME.into();

    let error = FileConnection::open_unattended(target, known_hosts())
        .await
        .expect_err("refused");
    assert!(
        matches!(error, ConnectError::QuestionRefused(_)),
        "{error:?}"
    );
    let reason = error.to_string();
    assert!(reason.contains(CODE_QUESTION.trim()), "{reason}");
    assert_eq!(server.logins(), 0);
}

#[tokio::test]
async fn unattended_a_jump_host_that_asks_for_a_password_is_refused() {
    let bastion = TestSshServer::start().await;
    let device = TestSshServer::start().await;
    let mut target = device.target();
    target.jump_hosts.push(enodix_core::session::JumpHost {
        name: "bastion".into(),
        host: "127.0.0.1".into(),
        port: bastion.port(),
        username: USERNAME.into(),
        auth: SshAuth::Prompt,
        legacy_crypto: false,
        connect_timeout: target.connect_timeout,
    });

    let error = FileConnection::open_unattended(target, known_hosts())
        .await
        .expect_err("refused");
    let reason = error.to_string();
    assert!(reason.contains("Password"), "{reason}");
    assert!(reason.contains("bastion"), "{reason}");
}

#[tokio::test]
async fn a_failed_connection_is_reported_as_such() {
    let port = support::closed_port().await;
    match FileConnection::open_unattended(target_at("127.0.0.1", port), known_hosts()).await {
        Err(ConnectError::Failed(OpenError::ConnectionRefused { .. })) => {}
        other => panic!("expected a refused connection, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn a_device_that_closes_the_connection_before_its_banner_says_its_lines_may_be_in_use() {
    let device = HangUpServer::start().await;
    let error =
        FileConnection::open_unattended(target_at("127.0.0.1", device.port()), known_hosts())
            .await
            .expect_err("failed");
    assert_eq!(error, ConnectError::Failed(OpenError::ClosedBeforeLogin));
    assert_eq!(
        error.to_string(),
        "the device closed the connection before the login began; all its lines may be in use"
    );
}

#[tokio::test]
async fn a_terminal_session_says_so_too_when_the_device_closes_before_its_banner() {
    let device = HangUpServer::start().await;
    let manager = manager();
    let (_id, mut events) = manager.open(
        SessionTarget::Ssh(target_at("127.0.0.1", device.port())),
        SIZE,
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    match next_state(&mut events).await {
        SessionState::Closed(enodix_core::session::CloseReason::Failed(
            OpenError::ClosedBeforeLogin,
        )) => {}
        other => panic!("expected the hang-up, got {other:?}"),
    }
}

#[tokio::test]
async fn a_cut_connection_is_noticed_and_reported_and_the_next_connection_works() {
    let server = TestSshServer::start().await;
    let connection = connected(server.target()).await;
    assert!(connection.is_open());

    server.cut_connections().await;
    let lost = tokio::time::timeout(WAIT, connection.lost())
        .await
        .expect("the loss was noticed");
    match &lost {
        Lost::Closed { message } => assert!(message.contains("going down"), "{message}"),
        other => panic!("expected the device to close it, got {other:?}"),
    }
    assert!(
        lost.to_string()
            .starts_with("the device closed the connection"),
        "{lost}"
    );
    assert!(!connection.is_open());
    match connection.start_sftp().await {
        Err(StartError::Connection(_)) => {}
        other => panic!(
            "expected the connection to have failed, got {:?}",
            other.err()
        ),
    }

    let again = connected(server.target()).await;
    assert!(again.is_open());
    assert_eq!(server.logins(), 2);
}

#[tokio::test]
async fn dropping_a_file_connection_closes_it_and_its_jump_connections() {
    let bastion = TestSshServer::start().await;
    let router = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    router.serve_sftp(folder.path());
    let mut target = router.target();
    target.jump_hosts.push(enodix_core::session::JumpHost {
        name: "bastion".into(),
        host: "127.0.0.1".into(),
        port: bastion.port(),
        username: USERNAME.into(),
        auth: SshAuth::Password(PASSWORD.into()),
        legacy_crypto: false,
        connect_timeout: target.connect_timeout,
    });

    let connection = connected(target).await;
    let sftp = connection.start_sftp().await.unwrap();
    sftp.home().await.unwrap();
    assert_eq!(router.connections_alive(), 1);
    assert_eq!(bastion.connections_alive(), 1);

    drop(sftp);
    drop(connection);
    until("router connections", 0, || router.connections_alive()).await;
    until("bastion connections", 0, || bastion.connections_alive()).await;
}

#[tokio::test]
async fn closing_a_file_connection_closes_it_at_once() {
    let server = TestSshServer::start().await;
    let connection = connected(server.target()).await;
    assert_eq!(server.connections_alive(), 1);
    connection.close().await;
    until("connections", 0, || server.connections_alive()).await;
}

#[tokio::test]
async fn a_file_connection_carries_one_channel_at_a_time() {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    let connection = connected(server.target()).await;

    let first = connection.start_sftp().await.unwrap();
    match connection.start_sftp().await {
        Err(StartError::Busy) => {}
        other => panic!("expected a busy connection, got {:?}", other.err()),
    }
    drop(first);
    let second = connection.start_sftp().await.unwrap();
    second.home().await.unwrap();
}

#[tokio::test]
async fn a_terminal_session_to_the_same_host_is_unaffected_by_a_file_connection() {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    fs::write(folder.path().join("a.txt"), b"hello").unwrap();
    server.serve_sftp(folder.path());
    let manager = manager();
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;

    let connection = connected(server.target()).await;
    let sftp = connection.start_sftp().await.unwrap();
    assert_eq!(sftp.list("/").await.unwrap().len(), 1);
    manager.write(id, b"size\r".to_vec()).unwrap();
    let shown = output_ending_with(&mut events, PROMPT).await;
    assert!(String::from_utf8_lossy(&shown).contains("80x24"));

    drop(sftp);
    connection.close().await;
    manager.write(id, b"size\r".to_vec()).unwrap();
    let shown = output_ending_with(&mut events, PROMPT).await;
    assert!(String::from_utf8_lossy(&shown).contains("80x24"));
    assert_eq!(server.shell_requests(), 1, "the terminal's only");
}
