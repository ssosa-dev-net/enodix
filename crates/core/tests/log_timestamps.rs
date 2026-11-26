//! Timestamps in session logs (#24): an option, inherited like the other
//! settings, that writes the time before each logged line.

mod support;

use std::fs;

use enodix_core::logging::LogFolder;
use enodix_core::session::{CloseReason, SessionEvents, SessionState, SessionTarget, TerminalSize};
use enodix_core::workspace::{Effective, HostId, Origin, Workspace};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("lab");
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

#[test]
fn timestamps_are_off_by_default_and_inherit_from_folders() {
    let (_temp, workspace) = workspace(&[
        ("Audit/_folder.yaml", "log_timestamps: true\n".into()),
        ("Audit/a.yaml", "address: 10.0.0.1\n".into()),
        (
            "Audit/b.yaml",
            "address: 10.0.0.2\nlog_timestamps: false\n".into(),
        ),
        ("c.yaml", "address: 10.0.0.3\n".into()),
    ]);
    let stamps = |id: &str| {
        workspace
            .resolve_host(&HostId::new(id))
            .unwrap()
            .settings
            .log_timestamps
    };

    assert_eq!(
        stamps("Audit/a.yaml"),
        Effective {
            value: true,
            origin: Origin::Folder {
                path: "Audit".into()
            }
        }
    );
    assert_eq!(
        stamps("Audit/b.yaml"),
        Effective {
            value: false,
            origin: Origin::Host
        }
    );
    assert_eq!(
        stamps("c.yaml"),
        Effective {
            value: false,
            origin: Origin::Default
        }
    );
}

/// Whether `line` starts with `[YYYY-MM-DD HH:MM:SS] `.
fn stamped(line: &str) -> bool {
    let Some(stamp) = line.strip_prefix('[').and_then(|rest| rest.get(..19)) else {
        return false;
    };
    stamp.chars().enumerate().all(|(i, c)| match i {
        4 | 7 => c == '-',
        10 => c == ' ',
        13 | 16 => c == ':',
        _ => c.is_ascii_digit(),
    }) && line[20..].starts_with("] ")
}

async fn closed(events: &mut SessionEvents) {
    match next_state(events).await {
        SessionState::Closed(reason) => assert_eq!(reason, CloseReason::ClosedLocally),
        other => panic!("expected the session to close, got {other:?}"),
    }
}

async fn logged_text(extra: &str) -> String {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[(
        "r1.yaml",
        format!("address: 127.0.0.1\nport: {}\n{extra}", server.port()),
    )]);
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path());
    let manager = support::manager();
    let id = HostId::new("r1.yaml");
    let target = workspace
        .resolve_host(&id)
        .unwrap()
        .ssh_target(USERNAME.into(), PASSWORD.into());
    let (session, mut events) = manager.open_logged(
        SessionTarget::Ssh(target),
        SIZE,
        logs.host_log(&workspace, &id),
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    manager.write(session, b"lines 2\r".to_vec()).unwrap();
    output_ending_with(&mut events, PROMPT).await;
    manager.close(session).unwrap();
    closed(&mut events).await;

    let folder = logs.host_folder(&workspace, &id).unwrap();
    let file = fs::read_dir(folder)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::read_to_string(file).unwrap()
}

#[tokio::test]
async fn a_log_with_timestamps_has_the_time_before_each_line() {
    let text = logged_text("log_timestamps: true\n").await;

    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines.iter().all(|line| stamped(line)), "{text}");
    let rest: Vec<&str> = lines.iter().map(|line| &line[22..]).collect();
    assert_eq!(rest, ["test-ssh$ lines 2", "line 1", "line 2", "test-ssh$"]);
}

#[tokio::test]
async fn a_log_without_timestamps_is_the_text_only() {
    assert_eq!(
        logged_text("").await,
        "test-ssh$ lines 2\nline 1\nline 2\ntest-ssh$\n"
    );
}
