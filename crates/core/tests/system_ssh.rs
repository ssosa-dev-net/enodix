//! The system ssh fallback (#27): the command it starts for a device the SSH
//! library cannot negotiate with. A stand-in script prints the arguments it
//! gets, so that what the real client would be told is observed through a
//! real local session. Unix only (the script is a shell script).

#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use enodix_core::session::{
    system_ssh_target, SessionEvent, SessionManager, SessionState, SessionTarget, SystemSshHost,
    TerminalSize, SYSTEM_SSH_PROGRAM,
};
use support::{known_hosts, next_state};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize {
    cols: 200,
    rows: 24,
};

fn host(username: Option<&str>) -> SystemSshHost {
    SystemSshHost {
        host: "10.0.0.7".into(),
        port: 2222,
        username: username.map(Into::into),
    }
}

#[test]
fn it_runs_the_systems_ssh_by_default() {
    let target = system_ssh_target(None, &host(Some("admin")), None, Vec::new());
    assert_eq!(target.program, SYSTEM_SSH_PROGRAM);
}

#[test]
fn it_allows_the_outdated_algorithms_and_names_the_device_last() {
    let target = system_ssh_target(None, &host(Some("admin")), None, Vec::new());
    let line = target.args.join(" ");
    for expected in [
        "KexAlgorithms=+diffie-hellman-group1-sha1",
        "HostKeyAlgorithms=+ssh-rsa,ssh-dss",
        "Ciphers=+aes128-cbc",
        "MACs=+hmac-sha1",
        "hmac-md5",
        "-p 2222",
        "-l admin",
    ] {
        assert!(line.contains(expected), "{expected}: {line}");
    }
    assert_eq!(&target.args[target.args.len() - 2..], ["--", "10.0.0.7"]);
}

#[test]
fn it_leaves_the_user_to_the_client_when_none_is_given() {
    let target = system_ssh_target(None, &host(None), None, Vec::new());
    assert!(!target.args.iter().any(|arg| arg == "-l"));
}

#[tokio::test]
async fn the_client_is_started_in_a_local_session_with_those_arguments() {
    let folder = TempDir::new().unwrap();
    let script = folder.path().join("fake-ssh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'ARGS:%s\\n' \"$*\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let target = system_ssh_target(
        script.to_str(),
        &host(Some("admin")),
        Some(folder.path().into()),
        Vec::new(),
    );
    let manager = SessionManager::new(known_hosts());
    let (_id, mut events) = manager.open(SessionTarget::Local(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);

    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !String::from_utf8_lossy(&received).contains("-- 10.0.0.7") {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    received.extend_from_slice(&bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await
    .expect("the client's arguments never arrived");
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("ARGS:-o KexAlgorithms=+"), "{text}");
    assert!(text.contains("-p 2222 -l admin -- 10.0.0.7"), "{text}");
}
