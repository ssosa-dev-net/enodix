//! Automatic reconnect (#24): the `auto_reconnect` setting, inherited like
//! the others, and which closed sessions may try again by themselves. The
//! tries themselves are the interface's: waits taken from the core.

use std::fs;
use std::time::Duration;

use enodix_core::session::{CloseReason, ConnectStage, OpenError, AUTO_RECONNECT_DELAYS};
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
        failed(OpenError::TimedOut {
            after: Duration::from_secs(15),
            stage: ConnectStage::Login,
        }),
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
