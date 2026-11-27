//! Automatic reconnect (#24): the `auto_reconnect` setting, inherited like
//! the others, which closed sessions may try again by themselves, and the
//! waits, count, reset and stop (`AutoReconnect`). The interface only runs
//! the countdown the core hands it.
//!
//! The time is passed in, so the count is driven without sleeping; the
//! sessions against the in-process servers reconnect at once, skipping
//! each wait, as the shell would after it.

mod support;

use std::fs;
use std::time::{Duration, Instant};

use enodix_core::session::{
    AutoReconnect, AutoReconnects, CloseReason, ConnectStage, JumpHost, OpenError, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAuth, SshTarget,
    TerminalSize, AUTO_RECONNECT_DELAYS, AUTO_RECONNECT_STABLE,
};
use enodix_core::workspace::{Effective, HostId, Origin, Workspace};
use support::proxy::LinkProxy;
use support::{HangUpServer, Misbehaviour, Stage, TestSshServer, PASSWORD, USERNAME};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Long enough for any of these sessions to open or fail.
const WAIT: Duration = Duration::from_secs(20);

fn now() -> Instant {
    Instant::now()
}

fn workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let write = |path: &str, text: &str| {
        let path = temp.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("Core/_folder.yaml", "auto_reconnect: true\n");
    write("Core/a.yaml", "address: 10.0.0.1\n");
    write("Core/b.yaml", "address: 10.0.0.2\nauto_reconnect: false\n");
    write("c.yaml", "address: 10.0.0.3\n");
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[test]
fn the_setting_is_off_by_default_and_inherits() {
    let (_temp, workspace) = workspace();
    let setting = |id: &str| {
        workspace
            .resolve_host(&HostId::new(id))
            .unwrap()
            .settings
            .auto_reconnect
    };

    assert_eq!(
        setting("Core/a.yaml"),
        Effective {
            value: true,
            origin: Origin::Folder {
                path: "Core".into()
            }
        }
    );
    assert_eq!(
        setting("Core/b.yaml"),
        Effective {
            value: false,
            origin: Origin::Host
        }
    );
    assert_eq!(
        setting("c.yaml"),
        Effective {
            value: false,
            origin: Origin::Default
        }
    );
}

#[test]
fn the_hosts_that_reconnect_by_themselves_are_listed() {
    let (_temp, workspace) = workspace();

    let hosts: Vec<_> = workspace
        .hosts_reconnecting_by_themselves()
        .into_iter()
        .map(|id| id.as_str().to_string())
        .collect();

    assert_eq!(hosts, ["Core/a.yaml"]);
}

fn failed(error: OpenError) -> CloseReason {
    CloseReason::Failed(error)
}

fn timed_out(stage: ConnectStage) -> CloseReason {
    failed(OpenError::TimedOut {
        after: Duration::from_secs(15),
        stage,
    })
}

#[test]
fn a_lost_connection_or_an_unreached_device_may_be_tried_again() {
    let unreached = [
        CloseReason::ConnectionLost,
        failed(OpenError::NameNotResolved { host: "h".into() }),
        failed(OpenError::Unreachable {
            host: "h".into(),
            reason: "no route".into(),
        }),
        failed(OpenError::ConnectionRefused {
            host: "h".into(),
            port: 22,
        }),
        timed_out(ConnectStage::Tcp {
            host: "h".into(),
            port: 22,
            via: None,
        }),
        timed_out(ConnectStage::Identification),
        timed_out(ConnectStage::KeyExchange),
        failed(OpenError::Connect("reset".into())),
        failed(OpenError::ClosedBeforeLogin),
        failed(OpenError::JumpHost {
            name: "jump".into(),
            error: Box::new(OpenError::ConnectionRefused {
                host: "j".into(),
                port: 22,
            }),
        }),
    ];
    for reason in unreached {
        assert!(reason.allows_auto_reconnect(), "{reason:?}");
    }
}

#[test]
fn a_refused_login_a_host_key_question_or_the_users_own_close_is_never_tried_again() {
    let never = [
        failed(OpenError::AuthenticationFailed),
        // From the login on, the credentials may have reached an AAA server
        // that counts each try against the account.
        timed_out(ConnectStage::Login),
        timed_out(ConnectStage::Channel),
        timed_out(ConnectStage::Shell),
        timed_out(ConnectStage::Subsystem {
            name: "sftp".into(),
        }),
        failed(OpenError::JumpHost {
            name: "jump".into(),
            error: Box::new(OpenError::TimedOut {
                after: Duration::from_secs(15),
                stage: ConnectStage::Login,
            }),
        }),
        failed(OpenError::HostKeyNotTrusted),
        failed(OpenError::HostKeyChanged),
        failed(OpenError::PrivateKey {
            file: "k".into(),
            reason: "bad".into(),
        }),
        failed(OpenError::JumpHost {
            name: "jump".into(),
            error: Box::new(OpenError::AuthenticationFailed),
        }),
        CloseReason::ClosedLocally,
        CloseReason::RemoteExited { exit_status: None },
        CloseReason::LocalExited { exit_status: None },
    ];
    for reason in never {
        assert!(!reason.allows_auto_reconnect(), "{reason:?}");
    }
}

#[test]
fn the_waits_grow_and_end() {
    assert!(AUTO_RECONNECT_DELAYS
        .windows(2)
        .all(|pair| pair[0] <= pair[1]));
    assert!(AUTO_RECONNECT_DELAYS[0] < *AUTO_RECONNECT_DELAYS.last().unwrap());
}

#[test]
fn each_close_gets_the_next_wait_and_the_count() {
    let mut reconnect = AutoReconnect::new();

    for (index, delay) in AUTO_RECONNECT_DELAYS.iter().enumerate() {
        let retry = reconnect
            .after_close(&CloseReason::ConnectionLost, now())
            .unwrap();
        assert_eq!(retry.after, *delay);
        assert_eq!(retry.attempt, index + 1);
        assert_eq!(retry.of, AUTO_RECONNECT_DELAYS.len());
    }
}

#[test]
fn it_gives_up_after_the_last_wait() {
    let mut reconnect = AutoReconnect::new();
    for _ in AUTO_RECONNECT_DELAYS {
        assert!(reconnect
            .after_close(&CloseReason::ConnectionLost, now())
            .is_some());
    }
    assert!(!reconnect.gave_up());

    assert_eq!(
        reconnect.after_close(&CloseReason::ConnectionLost, now()),
        None
    );
    assert!(reconnect.gave_up());
}

#[test]
fn staying_connected_starts_the_count_again() {
    let start = now();
    let mut reconnect = AutoReconnect::new();
    for _ in 0..3 {
        reconnect.after_close(&CloseReason::ConnectionLost, start);
    }

    reconnect.connected(start);

    let retry = reconnect
        .after_close(&CloseReason::ConnectionLost, start + AUTO_RECONNECT_STABLE)
        .unwrap();
    assert_eq!((retry.after, retry.attempt), (AUTO_RECONNECT_DELAYS[0], 1));
}

#[test]
fn a_session_that_connects_and_drops_at_once_keeps_counting_and_gives_up() {
    // A peer that accepts and then resets, a device that hangs up right
    // after the login: each try connects, then drops a second later.
    let mut at = now();
    let mut reconnect = AutoReconnect::new();
    let mut attempts = Vec::new();
    loop {
        reconnect.connected(at);
        at += Duration::from_secs(1);
        let Some(retry) = reconnect.after_close(&CloseReason::ConnectionLost, at) else {
            break;
        };
        attempts.push(retry.attempt);
        at += retry.after;
    }

    let all: Vec<usize> = (1..=AUTO_RECONNECT_DELAYS.len()).collect();
    assert_eq!(attempts, all);
    assert!(reconnect.gave_up());
}

#[test]
fn a_session_that_drops_just_short_of_staying_connected_keeps_counting() {
    let start = now();
    let mut reconnect = AutoReconnect::new();
    reconnect.after_close(&CloseReason::ConnectionLost, start);

    reconnect.connected(start);
    let short = start + AUTO_RECONNECT_STABLE - Duration::from_millis(1);
    let retry = reconnect
        .after_close(&CloseReason::ConnectionLost, short)
        .unwrap();

    assert_eq!(retry.attempt, 2);
}

#[test]
fn reconnecting_by_hand_starts_the_count_again() {
    let mut reconnect = AutoReconnect::new();
    for _ in 0..3 {
        reconnect.after_close(&CloseReason::ConnectionLost, now());
    }

    reconnect.restart();

    let retry = reconnect
        .after_close(&CloseReason::ConnectionLost, now())
        .unwrap();
    assert_eq!(retry.attempt, 1);
}

#[test]
fn a_key_stops_it_until_the_session_connects_again() {
    let mut reconnect = AutoReconnect::new();
    assert!(reconnect
        .after_close(&CloseReason::ConnectionLost, now())
        .is_some());

    reconnect.stop();
    assert_eq!(
        reconnect.after_close(&CloseReason::ConnectionLost, now()),
        None
    );
    assert!(!reconnect.gave_up());

    reconnect.connected(now());
    assert!(reconnect
        .after_close(&CloseReason::ConnectionLost, now())
        .is_some());
}

#[test]
fn reconnecting_by_hand_starts_it_afresh() {
    let mut reconnect = AutoReconnect::new();
    reconnect.stop();

    reconnect.restart();

    assert!(reconnect
        .after_close(&CloseReason::ConnectionLost, now())
        .is_some());
}

#[test]
fn a_reason_that_does_not_allow_it_gets_no_wait_and_costs_no_try() {
    let mut reconnect = AutoReconnect::new();

    assert_eq!(
        reconnect.after_close(&failed(OpenError::AuthenticationFailed), now()),
        None
    );
    assert_eq!(
        reconnect.after_close(&timed_out(ConnectStage::Login), now()),
        None
    );

    let retry = reconnect
        .after_close(&CloseReason::ConnectionLost, now())
        .unwrap();
    assert_eq!(retry.attempt, 1);
}

#[test]
fn sessions_count_on_their_own_and_only_when_enabled() {
    let all = AutoReconnects::new();
    let (a, b) = (SessionId(1), SessionId(2));
    all.enable(a, true);

    assert!(all
        .after_close(a, &CloseReason::ConnectionLost, now())
        .is_some());
    assert_eq!(
        all.after_close(b, &CloseReason::ConnectionLost, now()),
        None
    );
    all.stop(a);
    assert_eq!(
        all.after_close(a, &CloseReason::ConnectionLost, now()),
        None
    );
    all.connected(a, now());
    assert!(all
        .after_close(a, &CloseReason::ConnectionLost, now())
        .is_some());
    all.forget(a);
    assert_eq!(
        all.after_close(a, &CloseReason::ConnectionLost, now()),
        None
    );
}

async fn wait_until(done: impl Fn() -> bool, what: &str) {
    tokio::time::timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} within {WAIT:?}"));
}

/// Waits for the session to close, feeding its states to `auto` as the
/// shell does.
async fn closed(events: &mut SessionEvents, auto: &AutoReconnects, id: SessionId) -> CloseReason {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                }
                Some(SessionEvent::State(SessionState::Connected)) => {
                    auto.connected(id, now());
                }
                Some(SessionEvent::State(SessionState::Closed(reason))) => {
                    return reason;
                }
                Some(_) => {}
                None => panic!("the session's events ended before it closed"),
            }
        }
    })
    .await
    .expect("the session closes")
}

/// Runs a session as the shell does for a host with `auto_reconnect` on:
/// opens it and, after each close the core gives a try for, reconnects at
/// once (the wait skipped). Every close in order, and whether it gave up.
async fn as_the_shell(manager: &SessionManager, target: SshTarget) -> (Vec<CloseReason>, bool) {
    let auto = AutoReconnects::new();
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    auto.enable(id, true);
    let mut closes = Vec::new();
    loop {
        let reason = closed(&mut events, &auto, id).await;
        let retry = auto.after_close(id, &reason, now());
        closes.push(reason);
        if retry.is_none() {
            return (closes, auto.gave_up(id));
        }
        events = manager.reconnect(id, SIZE).unwrap();
    }
}

fn jump_host(port: u16) -> JumpHost {
    JumpHost {
        name: "bastion".into(),
        host: "127.0.0.1".into(),
        port,
        username: USERNAME.into(),
        auth: SshAuth::Password(PASSWORD.into()),
        legacy_crypto: false,
        connect_timeout: Duration::from_secs(5),
    }
}

#[test]
fn a_connection_that_ended_once_the_login_began_is_never_tried_again() {
    let ended = OpenError::EndedDuringLogin("the device closed the connection".into());
    assert!(!failed(ended.clone()).allows_auto_reconnect());
    assert!(!failed(OpenError::JumpHost {
        name: "jump".into(),
        error: Box::new(ended),
    })
    .allows_auto_reconnect());
}

#[tokio::test]
async fn a_device_that_closes_during_the_login_is_not_tried_again() {
    let server = TestSshServer::start().await;
    server
        .quirks()
        .misbehave_at(Stage::Login, Misbehaviour::Close);
    let manager = support::manager();

    let (closes, gave_up) = as_the_shell(&manager, server.target()).await;

    let [CloseReason::Failed(error)] = closes.as_slice() else {
        panic!("expected one failure, got {closes:?}");
    };
    assert!(matches!(error, OpenError::EndedDuringLogin(_)), "{error:?}");
    // What the user reads is as before.
    assert_eq!(
        error.to_string(),
        "could not connect: the device closed the connection during the login"
    );
    assert!(!gave_up);
    // One login only reached the device.
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn a_jump_host_that_closes_during_its_login_is_not_tried_again() {
    let bastion = TestSshServer::start().await;
    bastion
        .quirks()
        .misbehave_at(Stage::Login, Misbehaviour::Close);
    let device = TestSshServer::start().await;
    let target = SshTarget {
        jump_hosts: vec![jump_host(bastion.port())],
        ..device.target()
    };
    let manager = support::manager();

    let (closes, _) = as_the_shell(&manager, target).await;

    let [CloseReason::Failed(OpenError::JumpHost { name, error })] = closes.as_slice() else {
        panic!("expected one jump host failure, got {closes:?}");
    };
    assert_eq!(name, "bastion");
    assert!(
        matches!(error.as_ref(), OpenError::EndedDuringLogin(_)),
        "{error:?}"
    );
    assert_eq!(bastion.connections(), 1);
    assert_eq!(device.connections(), 0);
}

#[tokio::test]
async fn a_jump_host_lost_while_the_device_logs_in_is_not_tried_again() {
    let bastion = TestSshServer::start().await;
    let link = LinkProxy::to_ssh(&bastion).await;
    let device = TestSshServer::start().await;
    device
        .quirks()
        .pause_at(Stage::Login, Duration::from_secs(30));
    let target = SshTarget {
        jump_hosts: vec![jump_host(link.port())],
        ..device.target()
    };
    let manager = support::manager();

    let cut = async {
        // The device has been reached and is being logged in to.
        wait_until(|| device.connections_alive() == 1, "the device's login").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        link.cut();
    };
    let ((closes, _), ()) = tokio::join!(as_the_shell(&manager, target), cut);

    let [CloseReason::Failed(error)] = closes.as_slice() else {
        panic!("expected one failure, got {closes:?}");
    };
    eprintln!("the jump host lost during the device's login: {error}");
    assert!(!closes[0].allows_auto_reconnect(), "{error:?}");
    assert_eq!(device.connections(), 1);
}

#[tokio::test]
async fn a_failure_before_the_login_is_tried_again_until_the_tries_run_out() {
    // A device with all its lines in use hangs up before the login.
    let device = HangUpServer::start().await;
    let target = support::target_at("127.0.0.1", device.port());
    let manager = support::manager();

    let (closes, gave_up) = as_the_shell(&manager, target).await;

    assert_eq!(closes.len(), AUTO_RECONNECT_DELAYS.len() + 1);
    for reason in &closes {
        assert!(
            matches!(reason, CloseReason::Failed(OpenError::ClosedBeforeLogin)),
            "{reason:?}"
        );
    }
    assert!(gave_up);
}
