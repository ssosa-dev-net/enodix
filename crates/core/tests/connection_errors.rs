//! Why a connection failed, each kind of failure in its own words, and the
//! connect timeout.

mod support;

use std::time::{Duration, Instant};

use netterm_core::session::{
    AgentError, CloseReason, ConnectStage, JumpHost, OpenError, SessionManager, SessionState,
    SessionTarget, SshAuth, SshTarget, TerminalSize, DEFAULT_CONNECT_TIMEOUT,
};
use russh::MethodKind;
use support::{
    closed_port, next_prompt, next_state, output_ending_with, target_at, SilentServer,
    TestSshServer, CODE_QUESTION, ONE_TIME_CODE, OTP_USERNAME, PASSWORD, PASSWORD_QUESTION, PROMPT,
    STALLING_USERNAME,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Opens a session to `target` and returns why it failed.
async fn failure(manager: &SessionManager, target: SshTarget) -> OpenError {
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    match next_state(&mut events).await {
        SessionState::Closed(CloseReason::Failed(error)) => error,
        other => panic!("expected the session to fail, got {other:?}"),
    }
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_reported() {
    let manager = support::manager();

    // `.invalid` is reserved: it never resolves.
    let error = failure(&manager, target_at("no-such-device.invalid", 22)).await;

    assert_eq!(
        error,
        OpenError::NameNotResolved {
            host: "no-such-device.invalid".into()
        }
    );
    assert_eq!(
        error.to_string(),
        "the name no-such-device.invalid could not be resolved"
    );
}

#[tokio::test]
async fn a_port_nobody_listens_on_is_reported_as_refused() {
    let manager = support::manager();
    let port = closed_port().await;

    let error = failure(&manager, target_at("127.0.0.1", port)).await;

    assert_eq!(
        error,
        OpenError::ConnectionRefused {
            host: "127.0.0.1".into(),
            port
        }
    );
    assert_eq!(
        error.to_string(),
        format!("127.0.0.1 refused the connection on port {port}")
    );
}

// 240.0.0.0/4 is reserved: Windows reports the network unreachable at
// once. Elsewhere the result depends on the machine's routes (a default
// route may send the SYN and time out), so the test runs on Windows only;
// the classification itself is portable.
#[cfg_attr(not(windows), ignore = "needs Windows' handling of 240.0.0.0/4")]
#[tokio::test]
async fn an_unreachable_network_is_reported() {
    let manager = support::manager();

    let error = failure(&manager, target_at("240.0.0.1", 22)).await;

    match &error {
        OpenError::Unreachable { host, .. } => assert_eq!(host, "240.0.0.1"),
        other => panic!("expected the host to be unreachable, got {other:?}"),
    }
    assert!(
        error
            .to_string()
            .starts_with("240.0.0.1 cannot be reached ("),
        "{error}"
    );
}

#[tokio::test]
async fn a_device_that_never_answers_times_out_after_the_configured_timeout() {
    let manager = support::manager();
    let silent = SilentServer::start().await;
    let target = SshTarget {
        connect_timeout: Duration::from_secs(1),
        ..target_at("127.0.0.1", silent.port())
    };

    let started = Instant::now();
    let error = failure(&manager, target).await;
    let took = started.elapsed();

    assert_eq!(
        error,
        OpenError::TimedOut {
            after: Duration::from_secs(1),
            stage: ConnectStage::Identification,
        }
    );
    assert_eq!(
        error.to_string(),
        "connected, but no SSH identification line came within 1 s"
    );
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(5),
        "took {took:?}"
    );
}

#[tokio::test]
async fn a_login_the_device_never_answers_times_out() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let target = SshTarget {
        username: STALLING_USERNAME.into(),
        connect_timeout: Duration::from_secs(1),
        ..server.target()
    };

    let error = failure(&manager, target).await;

    assert_eq!(
        error,
        OpenError::TimedOut {
            after: Duration::from_secs(1),
            stage: ConnectStage::Login,
        }
    );
}

#[tokio::test]
async fn the_time_the_user_takes_to_answer_login_questions_does_not_count() {
    let server = TestSshServer::start_offering(&[MethodKind::KeyboardInteractive]).await;
    let manager = support::manager();
    let target = SshTarget {
        username: OTP_USERNAME.into(),
        auth: SshAuth::Prompt,
        connect_timeout: Duration::from_secs(1),
        ..server.target()
    };

    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    // The password, asked before logging in (the device's own password
    // question then gets it), and the device's one-time code: each answer
    // takes longer than the timeout.
    let asked = next_prompt(&mut events).await;
    assert_eq!(asked.questions[0].text, PASSWORD_QUESTION);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    manager.answer(id, vec![PASSWORD.into()]).unwrap();
    let asked = next_prompt(&mut events).await;
    assert_eq!(asked.questions[0].text, CODE_QUESTION);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    manager.answer(id, vec![ONE_TIME_CODE.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn a_jump_host_failure_says_which_kind_it_was() {
    let device = TestSshServer::start().await;
    let manager = support::manager();
    let port = closed_port().await;
    let target = SshTarget {
        jump_hosts: vec![JumpHost {
            name: "bastion".into(),
            host: "127.0.0.1".into(),
            port,
            username: support::USERNAME.into(),
            auth: SshAuth::Password(PASSWORD.into()),
            legacy_crypto: false,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }],
        ..device.target()
    };

    let error = failure(&manager, target).await;

    assert_eq!(
        error,
        OpenError::JumpHost {
            name: "bastion".into(),
            error: Box::new(OpenError::ConnectionRefused {
                host: "127.0.0.1".into(),
                port
            })
        }
    );
}

#[tokio::test]
async fn each_kind_of_failure_has_its_own_message() {
    let messages = [
        CloseReason::Failed(OpenError::NameNotResolved {
            host: "core1".into(),
        }),
        CloseReason::Failed(OpenError::Unreachable {
            host: "core1".into(),
            reason: "no route".into(),
        }),
        CloseReason::Failed(OpenError::ConnectionRefused {
            host: "core1".into(),
            port: 22,
        }),
        CloseReason::Failed(OpenError::TimedOut {
            after: Duration::from_secs(15),
            stage: ConnectStage::Tcp {
                host: "core1".into(),
                port: 22,
                via: None,
            },
        }),
        CloseReason::Failed(OpenError::AuthenticationFailed),
        CloseReason::ConnectionLost,
    ]
    .map(|reason| reason.to_string());

    assert_eq!(
        messages,
        [
            "the name core1 could not be resolved",
            "core1 cannot be reached (no route)",
            "core1 refused the connection on port 22",
            "no TCP connection to core1 port 22 within 15 s",
            "authentication failed",
            "connection lost",
        ]
    );
}

#[tokio::test]
async fn a_failure_says_what_to_check_next() {
    // Each failure a newcomer meets first says what to check and where it
    // is set, besides what happened (#116). A lost connection needs none:
    // reconnecting is the way on.
    let advice = [
        CloseReason::Failed(OpenError::NameNotResolved {
            host: "core1".into(),
        }),
        CloseReason::Failed(OpenError::Unreachable {
            host: "core1".into(),
            reason: "no route".into(),
        }),
        CloseReason::Failed(OpenError::ConnectionRefused {
            host: "core1".into(),
            port: 2222,
        }),
        CloseReason::Failed(OpenError::TimedOut {
            after: Duration::from_secs(15),
            stage: ConnectStage::Tcp {
                host: "core1".into(),
                port: 22,
                via: None,
            },
        }),
        CloseReason::Failed(OpenError::AuthenticationFailed),
        CloseReason::Failed(OpenError::Agent(AgentError::NotRunning("no agent".into()))),
        CloseReason::Failed(OpenError::Agent(AgentError::NoKeys)),
        CloseReason::Failed(OpenError::Agent(AgentError::KeysRefused)),
        CloseReason::Failed(OpenError::HostKeyChanged),
        CloseReason::Failed(OpenError::JumpHost {
            name: "Site/bastion.yaml".into(),
            error: Box::new(OpenError::ConnectionRefused {
                host: "10.0.0.1".into(),
                port: 22,
            }),
        }),
        CloseReason::ConnectionLost,
        CloseReason::ClosedLocally,
    ]
    .map(|reason| reason.advice());

    assert_eq!(
        advice,
        [
            Some("Check the address: a name your DNS knows, or an IP address.".into()),
            Some(
                "Check the address, and that this computer has a route to it (a VPN, say).".into()
            ),
            Some(
                "Nothing accepts connections on port 2222: check the Port setting, and that \
                 the device has SSH (or telnet) turned on."
                    .into()
            ),
            Some(
                "Check the address and that the device is up; a firewall may be dropping the \
                 connection. A slow device may need a longer Connect timeout."
                    .into()
            ),
            Some(
                "Check the username and password (or key). For a host with a credential \
                 profile, change them in the Credentials view, then reconnect."
                    .into()
            ),
            Some(
                "Start your SSH agent with your key in it and connect again, or log in with a \
                 password."
                    .into()
            ),
            Some(
                "Add your key to the SSH agent (ssh-add) and connect again, or log in with a \
                 password."
                    .into()
            ),
            Some(
                "Check the username, and that the device has your public key; or log in with a \
                 password."
                    .into()
            ),
            Some(
                "If the device was replaced or reinstalled, check its new fingerprint with the \
                 device, then connect again and replace the key."
                    .into()
            ),
            Some(
                "For the jump host Site/bastion.yaml: nothing accepts connections on port 22: \
                 check the Port setting, and that the device has SSH (or telnet) turned on."
                    .into()
            ),
            None,
            None,
        ]
    );
}

#[tokio::test]
async fn a_refused_port_comes_with_its_advice() {
    // The advice travels with the failure the session ends with.
    let manager = support::manager();
    let port = closed_port().await;
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target_at("127.0.0.1", port)), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let SessionState::Closed(reason) = next_state(&mut events).await else {
        panic!("expected the session to fail");
    };

    assert_eq!(
        reason.advice(),
        Some(format!(
            "Nothing accepts connections on port {port}: check the Port setting, and that \
             the device has SSH (or telnet) turned on."
        ))
    );
}
