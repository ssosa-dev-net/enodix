//! Automatic reconnect (#24): the `auto_reconnect` setting, inherited like
//! the others, which closed sessions may try again by themselves, and the
//! waits, count, reset and stop (`AutoReconnect`). The interface only runs
//! the countdown the core hands it.

use std::fs;
use std::time::Duration;

use enodix_core::session::{
    AutoReconnect, AutoReconnects, CloseReason, ConnectStage, OpenError, SessionId,
    AUTO_RECONNECT_DELAYS,
};
use enodix_core::workspace::{Effective, HostId, Origin, Workspace};
use tempfile::TempDir;

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
        let retry = reconnect.after_close(&CloseReason::ConnectionLost).unwrap();
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
            .after_close(&CloseReason::ConnectionLost)
            .is_some());
    }
    assert!(!reconnect.gave_up());

    assert_eq!(reconnect.after_close(&CloseReason::ConnectionLost), None);
    assert!(reconnect.gave_up());
}

#[test]
fn connecting_starts_the_count_again() {
    let mut reconnect = AutoReconnect::new();
    for _ in 0..3 {
        reconnect.after_close(&CloseReason::ConnectionLost);
    }

    reconnect.connected();

    let retry = reconnect.after_close(&CloseReason::ConnectionLost).unwrap();
    assert_eq!((retry.after, retry.attempt), (AUTO_RECONNECT_DELAYS[0], 1));
}

#[test]
fn a_key_stops_it_until_the_session_connects_again() {
    let mut reconnect = AutoReconnect::new();
    assert!(reconnect
        .after_close(&CloseReason::ConnectionLost)
        .is_some());

    reconnect.stop();
    assert_eq!(reconnect.after_close(&CloseReason::ConnectionLost), None);
    assert!(!reconnect.gave_up());

    reconnect.connected();
    assert!(reconnect
        .after_close(&CloseReason::ConnectionLost)
        .is_some());
}

#[test]
fn reconnecting_by_hand_starts_it_afresh() {
    let mut reconnect = AutoReconnect::new();
    reconnect.stop();

    reconnect.restart();

    assert!(reconnect
        .after_close(&CloseReason::ConnectionLost)
        .is_some());
}

#[test]
fn a_reason_that_does_not_allow_it_gets_no_wait_and_costs_no_try() {
    let mut reconnect = AutoReconnect::new();

    assert_eq!(
        reconnect.after_close(&failed(OpenError::AuthenticationFailed)),
        None
    );
    assert_eq!(reconnect.after_close(&timed_out(ConnectStage::Login)), None);

    let retry = reconnect.after_close(&CloseReason::ConnectionLost).unwrap();
    assert_eq!(retry.attempt, 1);
}

#[test]
fn sessions_count_on_their_own_and_only_when_enabled() {
    let all = AutoReconnects::new();
    let (a, b) = (SessionId(1), SessionId(2));
    all.enable(a, true);

    assert!(all.after_close(a, &CloseReason::ConnectionLost).is_some());
    assert_eq!(all.after_close(b, &CloseReason::ConnectionLost), None);
    all.stop(a);
    assert_eq!(all.after_close(a, &CloseReason::ConnectionLost), None);
    all.connected(a);
    assert!(all.after_close(a, &CloseReason::ConnectionLost).is_some());
    all.forget(a);
    assert_eq!(all.after_close(a, &CloseReason::ConnectionLost), None);
}
