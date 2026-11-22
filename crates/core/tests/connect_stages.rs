//! The connect timeout says which stage did not answer (#140): no TCP
//! connection, no SSH identification line, no key exchange, no answer to
//! the login, no answer to the channel request, no shell; each hop of a
//! jump chain named with its stage; telnet; file connections. Each device
//! here goes silent at one stage (`support`), with a connect timeout of
//! 1 s, and the test reads the failure as the user does.

mod support;

use std::time::Duration;

use enodix_core::files::{ConnectError, FileConnection, StartError};
use enodix_core::session::{
    CloseReason, JumpHost, OpenError, SessionManager, SessionTarget, SshAuth, SshTarget,
    TelnetTarget, TerminalSize,
};
use support::raw::RawDevice;
use support::{
    known_hosts, output_until_closed, Misbehaviour, SilentServer, Stage, TestSshServer,
    UnansweringPort, PASSWORD, USERNAME, WAIT,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

const SHORT: Duration = Duration::from_secs(1);

fn short(target: SshTarget) -> SshTarget {
    SshTarget {
        connect_timeout: SHORT,
        ..target
    }
}

/// Why a session to `target` never connected.
async fn failure(manager: &SessionManager, target: SessionTarget) -> OpenError {
    let (_id, mut events) = manager.open(target, SIZE);
    match output_until_closed(&mut events, WAIT).await.1 {
        CloseReason::Failed(error) => error,
        other => panic!("expected the session to fail, got {other:?}"),
    }
}

async fn ssh_failure(target: SshTarget) -> OpenError {
    failure(&support::manager(), SessionTarget::Ssh(short(target))).await
}

/// A device that goes silent at `stage` of the login or the shell start.
async fn silent_at(stage: Stage) -> TestSshServer {
    let server = TestSshServer::start().await;
    server.quirks().misbehave_at(stage, Misbehaviour::Stall);
    server
}

fn jump_host(name: &str, port: u16) -> JumpHost {
    JumpHost {
        name: name.into(),
        host: "127.0.0.1".into(),
        port,
        username: USERNAME.into(),
        auth: SshAuth::Password(PASSWORD.into()),
        legacy_crypto: false,
        connect_timeout: SHORT,
    }
}

#[tokio::test]
async fn no_tcp_connection_names_the_address_and_port() {
    let port = UnansweringPort::start().await;

    let error = ssh_failure(support::target_at("127.0.0.1", port.port())).await;

    assert_eq!(
        error.to_string(),
        format!(
            "no TCP connection to 127.0.0.1 port {} within 1 s",
            port.port()
        )
    );
    let advice = error.advice().unwrap();
    assert!(advice.contains("firewall"), "{advice}");
}

#[tokio::test]
async fn a_port_that_accepts_but_sends_no_ssh_line_says_so() {
    let silent = SilentServer::start().await;

    let error = ssh_failure(support::target_at("127.0.0.1", silent.port())).await;

    assert_eq!(
        error.to_string(),
        "connected, but no SSH identification line came within 1 s"
    );
    let advice = error.advice().unwrap();
    assert!(advice.contains("Port setting"), "{advice}");
}

#[tokio::test]
async fn a_device_that_sends_its_ssh_line_and_no_key_exchange_says_so() {
    let device = RawDevice::sending(vec![(Duration::ZERO, b"SSH-2.0-Device\r\n".to_vec())]).await;

    let error = ssh_failure(support::target_at("127.0.0.1", device.port())).await;

    assert_eq!(
        error.to_string(),
        "the SSH identification line came, but the key exchange did not finish within 1 s"
    );
}

#[tokio::test]
async fn lines_before_the_ssh_line_are_not_taken_for_it() {
    // RFC 4253 lets a server send lines before its identification.
    let device = RawDevice::sending(vec![(Duration::ZERO, b"Welcome\r\nSSH-2.0".to_vec())]).await;

    let error = ssh_failure(support::target_at("127.0.0.1", device.port())).await;

    assert_eq!(
        error.to_string(),
        "connected, but no SSH identification line came within 1 s"
    );
}

#[tokio::test]
async fn a_login_that_gets_no_answer_says_so() {
    let server = silent_at(Stage::Login).await;

    let error = ssh_failure(server.target()).await;

    assert_eq!(
        error.to_string(),
        "the key exchange finished, but the login got no answer within 1 s"
    );
    let advice = error.advice().unwrap();
    assert!(advice.contains("RADIUS"), "{advice}");
}

#[tokio::test]
async fn a_channel_request_that_gets_no_answer_says_so() {
    let server = silent_at(Stage::ChannelOpen).await;

    let error = ssh_failure(server.target()).await;

    assert_eq!(
        error.to_string(),
        "logged in, but the request for a session channel got no answer within 1 s"
    );
}

#[tokio::test]
async fn a_shell_that_never_starts_says_so() {
    let server = silent_at(Stage::ShellRequest).await;

    let error = ssh_failure(server.target()).await;

    assert_eq!(
        error.to_string(),
        "logged in, but the shell did not start within 1 s"
    );
    let advice = error.advice().unwrap();
    assert!(advice.contains("accepted the login"), "{advice}");
}

#[tokio::test]
async fn a_jump_host_that_does_not_answer_its_login_is_named_with_the_stage() {
    let bastion = silent_at(Stage::Login).await;
    let device = TestSshServer::start().await;
    let target = SshTarget {
        jump_hosts: vec![jump_host("bastion", bastion.port())],
        ..device.target()
    };

    let error = ssh_failure(target).await;

    assert_eq!(
        error.to_string(),
        "jump host bastion: the key exchange finished, but the login got no answer within 1 s"
    );
    let advice = error.advice().unwrap();
    assert!(
        advice.starts_with("For the jump host bastion: "),
        "{advice}"
    );
}

#[tokio::test]
async fn the_second_jump_host_is_named_with_its_stage() {
    let core = TestSshServer::start().await;
    let site = RawDevice::sending(vec![(Duration::ZERO, b"SSH-2.0-Site\r\n".to_vec())]).await;
    let device = TestSshServer::start().await;
    let target = SshTarget {
        jump_hosts: vec![
            jump_host("core", core.port()),
            jump_host("site", site.port()),
        ],
        ..device.target()
    };

    let error = ssh_failure(target).await;

    assert_eq!(
        error.to_string(),
        "jump host site: the SSH identification line came, but the key exchange did not \
         finish within 1 s"
    );
}

#[tokio::test]
async fn a_device_the_jump_host_gets_no_tcp_connection_to_says_through_which() {
    let bastion = TestSshServer::start().await;
    let unanswering = UnansweringPort::start().await;
    let target = SshTarget {
        jump_hosts: vec![jump_host("bastion", bastion.port())],
        ..support::target_at("127.0.0.1", unanswering.port())
    };

    let error = ssh_failure(target).await;

    assert_eq!(
        error.to_string(),
        format!(
            "no TCP connection to 127.0.0.1 port {} through the jump host bastion within 1 s",
            unanswering.port()
        )
    );
    let advice = error.advice().unwrap();
    assert!(advice.contains("bastion can reach it"), "{advice}");
}

#[tokio::test]
async fn a_telnet_device_that_gets_no_tcp_connection_says_so() {
    let port = UnansweringPort::start().await;
    let target = SessionTarget::Telnet(TelnetTarget {
        host: "127.0.0.1".into(),
        port: port.port(),
        connect_timeout: SHORT,
    });

    let error = failure(&support::manager(), target).await;

    assert_eq!(
        error.to_string(),
        format!(
            "no TCP connection to 127.0.0.1 port {} within 1 s",
            port.port()
        )
    );
}

#[tokio::test]
async fn a_file_connection_says_which_stage_did_not_answer() {
    let server = silent_at(Stage::Login).await;

    let opened = FileConnection::open_unattended(short(server.target()), known_hosts()).await;

    match opened {
        Err(ConnectError::Failed(error)) => assert_eq!(
            error.to_string(),
            "the key exchange finished, but the login got no answer within 1 s"
        ),
        other => panic!("expected a timeout, got {:?}", other.err()),
    }
}

/// What starting SFTP on a file connection to `server` reports.
async fn sftp_failure(server: &TestSshServer) -> String {
    let connection = FileConnection::open_unattended(short(server.target()), known_hosts())
        .await
        .expect("the file connection opened");
    match tokio::time::timeout(WAIT, connection.start_sftp()).await {
        Ok(Err(StartError::Connection(reason))) => reason,
        Ok(other) => panic!("expected SFTP not to start, got {:?}", other.err()),
        Err(_) => panic!("SFTP's start did not end in time"),
    }
}

#[tokio::test]
async fn starting_sftp_says_which_request_got_no_answer() {
    let channel = silent_at(Stage::ChannelOpen).await;
    let subsystem = silent_at(Stage::Subsystem).await;

    assert_eq!(
        sftp_failure(&channel).await,
        "logged in, but the request for a session channel got no answer within 1 s"
    );
    assert_eq!(
        sftp_failure(&subsystem).await,
        "logged in, but the request for the sftp subsystem got no answer within 1 s"
    );
}
