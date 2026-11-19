//! Session logging: every session to a workspace host is written to a plain
//! text file under the per-user log folder (here, always a temp folder),
//! organised by workspace, tree path, host and start time, unless the
//! host's resolved `logging` setting is off. A log that cannot be written
//! is reported and the session goes on.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use netterm_core::logging::{LogFolder, PlainText, SessionLog};
use netterm_core::session::{
    CloseReason, SessionEvent, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, TerminalSize,
};
use netterm_core::workspace::{HostId, Workspace};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME, WAIT};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A workspace folder called `lab` made of `files` (relative path, text).
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

/// A host file for the test server.
fn host(server: &TestSshServer, extra: &str) -> String {
    format!("address: 127.0.0.1\nport: {}\n{extra}", server.port())
}

/// A session to the workspace host `id` with the password login, logged as
/// its settings say.
fn open(
    manager: &SessionManager,
    logs: &LogFolder,
    workspace: &Workspace,
    id: &str,
) -> (SessionId, SessionEvents) {
    let id = HostId::new(id);
    let target = workspace
        .resolve_host(&id)
        .unwrap()
        .ssh_target(USERNAME.into(), PASSWORD.into());
    manager.open_logged(
        SessionTarget::Ssh(target),
        SIZE,
        logs.host_log(workspace, &id),
    )
}

async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

/// Types `line` and Enter, and waits for the next prompt.
async fn run(manager: &SessionManager, id: SessionId, events: &mut SessionEvents, line: &str) {
    manager.write(id, format!("{line}\r").into_bytes()).unwrap();
    output_ending_with(events, PROMPT).await;
}

/// Every file below `folder`, as paths relative to it with `/`.
fn files_below(folder: &Path) -> Vec<String> {
    fn walk(base: &Path, folder: &Path, found: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(folder) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, found);
            } else {
                let relative = path.strip_prefix(base).unwrap();
                let parts: Vec<_> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                found.push(parts.join("/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(folder, folder, &mut found);
    found.sort();
    found
}

/// The log files in `folder`, by name.
fn logs_in(folder: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    files
}

/// Whether `name` is a start time as log files are named:
/// `YYYY-MM-DD_HH-MM-SS.log`, maybe with `_<n>` before `.log`.
fn is_start_time_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".log") else {
        return false;
    };
    let time = stem.get(..19).unwrap_or("");
    let shape = time.chars().enumerate().all(|(i, c)| match i {
        4 | 7 | 13 | 16 => c == '-',
        10 => c == '_',
        _ => c.is_ascii_digit(),
    });
    let rest = &stem[time.len()..];
    time.len() == 19
        && shape
        && (rest.is_empty()
            || rest
                .strip_prefix('_')
                .is_some_and(|n| n.parse::<u32>().is_ok()))
}

async fn closed(events: &mut SessionEvents) -> CloseReason {
    match next_state(events).await {
        SessionState::Closed(reason) => reason,
        other => panic!("expected the session to close, got {other:?}"),
    }
}

#[tokio::test]
async fn an_ssh_session_is_logged_by_workspace_tree_path_host_and_start_time() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[("Europe/Madrid/r1.yaml", host(&server, ""))]);
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path());
    let manager = support::manager();

    let before = now();
    let (id, mut events) = open(&manager, &logs, &workspace, "Europe/Madrid/r1.yaml");
    connected(&mut events).await;
    let after = now();
    run(&manager, id, &mut events, "colors").await;
    run(&manager, id, &mut events, "lines 2").await;
    manager.close(id).unwrap();
    assert_eq!(closed(&mut events).await, CloseReason::ClosedLocally);

    let folder = log_root
        .path()
        .join("workspaces")
        .join("lab")
        .join("Europe")
        .join("Madrid")
        .join("r1");
    assert_eq!(
        logs.host_folder(&workspace, &HostId::new("Europe/Madrid/r1.yaml")),
        Some(folder.clone())
    );
    let files = logs_in(&folder);
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(is_start_time_name(&name), "{name}");
    // Named after the local time the session connected.
    assert!(
        (before.as_str()..=after.as_str()).contains(&&name[..19]),
        "{name} should be between {before} and {after}"
    );
    // Plain text: the colour sequences are gone, line ends are `\n`, and the
    // unfinished prompt line is written when the session closes.
    assert_eq!(
        fs::read_to_string(&files[0]).unwrap(),
        "test-ssh$ colors\ndown up\ntest-ssh$ lines 2\nline 1\nline 2\ntest-ssh$\n"
    );
    // Nothing else was written anywhere under the log folder.
    assert_eq!(files_below(log_root.path()).len(), 1);
}

/// The local time now as log file names write it.
fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

#[tokio::test]
async fn hosts_beneath_a_folder_with_logging_off_are_not_logged() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        ("Secure/_folder.yaml", "logging: false\n".into()),
        ("Secure/fw1.yaml", host(&server, "")),
        ("Secure/Vault/fw2.yaml", host(&server, "")),
        ("Secure/audited.yaml", host(&server, "logging: true\n")),
        ("Branch/sw1.yaml", host(&server, "")),
    ]);
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path());
    let manager = support::manager();

    // What the resolved setting gives.
    let host_log = |id: &str| logs.host_log(&workspace, &HostId::new(id));
    assert_eq!(host_log("Secure/fw1.yaml"), None);
    assert_eq!(host_log("Secure/Vault/fw2.yaml"), None);
    assert!(host_log("Secure/audited.yaml").is_some());
    assert!(host_log("Branch/sw1.yaml").is_some());

    // A whole session to a host beneath the folder writes nothing.
    let (id, mut events) = open(&manager, &logs, &workspace, "Secure/Vault/fw2.yaml");
    connected(&mut events).await;
    run(&manager, id, &mut events, "lines 3").await;
    manager.write(id, b"exit\r".to_vec()).unwrap();
    assert_eq!(
        closed(&mut events).await,
        CloseReason::RemoteExited {
            exit_status: Some(0)
        }
    );
    assert_eq!(files_below(log_root.path()), Vec::<String>::new());

    // The host that turns it back on for itself is logged.
    let (id, mut events) = open(&manager, &logs, &workspace, "Secure/audited.yaml");
    connected(&mut events).await;
    manager.write(id, b"exit\r".to_vec()).unwrap();
    closed(&mut events).await;
    let files = files_below(log_root.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(
        files[0].starts_with("workspaces/lab/Secure/audited/"),
        "{files:?}"
    );
    assert_eq!(
        fs::read_to_string(log_root.path().join(&files[0])).unwrap(),
        "test-ssh$ exit\n"
    );
}

#[tokio::test]
async fn each_connection_of_a_session_gets_its_own_log() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[("r1.yaml", host(&server, ""))]);
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path());
    let manager = support::manager();

    let (id, mut events) = open(&manager, &logs, &workspace, "r1.yaml");
    connected(&mut events).await;
    run(&manager, id, &mut events, "lines 1").await;
    manager.close(id).unwrap();
    closed(&mut events).await;

    let mut events = manager.reconnect(id, SIZE).unwrap();
    connected(&mut events).await;
    run(&manager, id, &mut events, "lines 2").await;
    manager.close(id).unwrap();
    closed(&mut events).await;

    let (copy, mut events) = manager.duplicate(id, SIZE).unwrap();
    connected(&mut events).await;
    run(&manager, copy, &mut events, "lines 3").await;
    manager.close(copy).unwrap();
    closed(&mut events).await;

    let folder = log_root.path().join("workspaces").join("lab").join("r1");
    let files = logs_in(&folder);
    assert_eq!(files.len(), 3, "{files:?}");
    let mut contents: Vec<_> = files
        .iter()
        .map(|file| {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            assert!(is_start_time_name(&name), "{name}");
            fs::read_to_string(file).unwrap()
        })
        .collect();
    contents.sort();
    assert_eq!(
        contents,
        [
            "test-ssh$ lines 1\nline 1\ntest-ssh$\n",
            "test-ssh$ lines 2\nline 1\nline 2\ntest-ssh$\n",
            "test-ssh$ lines 3\nline 1\nline 2\nline 3\ntest-ssh$\n",
        ]
    );
}

#[tokio::test]
async fn a_log_that_cannot_be_written_is_reported_and_the_session_goes_on() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[("r1.yaml", host(&server, ""))]);
    // The log folder is in the way: a file stands where it should be, as
    // good as a full or read-only disk for the logger.
    let temp = TempDir::new().unwrap();
    let blocked = temp.path().join("logs");
    fs::write(&blocked, "not a folder").unwrap();
    let logs = LogFolder::new(&blocked);
    let manager = support::manager();

    let (id, mut events) = open(&manager, &logs, &workspace, "r1.yaml");
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    let failure = tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::LogFailed(failure)) => return failure,
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                other => panic!("expected the log failure, got {other:?}"),
            }
        }
    })
    .await
    .expect("timed out waiting for the log failure");
    assert!(
        failure.path.starts_with(&blocked),
        "{} should be under {}",
        failure.path.display(),
        blocked.display()
    );
    assert!(failure.to_string().contains("not logged"), "{failure}");

    // The session works as before.
    output_ending_with(&mut events, PROMPT).await;
    manager.write(id, b"lines 2\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, PROMPT).await;
    assert!(String::from_utf8_lossy(&output).contains("line 2"));
    manager.write(id, b"exit\r".to_vec()).unwrap();
    assert_eq!(
        closed(&mut events).await,
        CloseReason::RemoteExited {
            exit_status: Some(0)
        }
    );
    assert_eq!(fs::read_to_string(&blocked).unwrap(), "not a folder");
}

#[tokio::test]
async fn quick_connections_are_logged_by_their_address() {
    let server = TestSshServer::start().await;
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path());
    let manager = support::manager();

    let log = logs.quick_log("127.0.0.1");
    assert_eq!(
        log.folder(),
        log_root.path().join("quick connect").join("127.0.0.1")
    );
    let (id, mut events) =
        manager.open_logged(SessionTarget::Ssh(server.target()), SIZE, Some(log.clone()));
    connected(&mut events).await;
    manager.write(id, b"exit\r".to_vec()).unwrap();
    closed(&mut events).await;
    let files = logs_in(&log.folder());
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(fs::read_to_string(&files[0]).unwrap(), "test-ssh$ exit\n");
}

#[test]
fn quick_connection_folders_have_portable_names() {
    let logs = LogFolder::new("logs");
    let folder = |name: &str| -> SessionLog { logs.quick_log(name) };
    let quick = Path::new("logs").join("quick connect");
    assert_eq!(folder("fe80::1").folder(), quick.join("fe80__1"));
    // A reserved device name on Windows.
    assert_eq!(folder("COM3").folder(), quick.join("_COM3"));
    assert_eq!(folder("").folder(), quick.join("_"));
    assert_eq!(folder("host.").folder(), quick.join("host_"));
}

#[test]
fn the_host_folder_can_be_created_before_any_session() {
    let (_temp, workspace) = workspace(&[("Core/r1.yaml", "address: 10.0.0.1\n".into())]);
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path().join("logs"));
    let id = HostId::new("Core/r1.yaml");
    let folder = logs.create_host_folder(&workspace, &id).unwrap();
    assert_eq!(
        folder,
        log_root
            .path()
            .join("logs")
            .join("workspaces")
            .join("lab")
            .join("Core")
            .join("r1")
    );
    assert!(folder.is_dir());
    assert!(logs
        .create_host_folder(&workspace, &HostId::new("missing.yaml"))
        .is_err());
}

/// Feeds `chunks` through a [`PlainText`] and returns all it gave.
fn plain(chunks: &[&[u8]]) -> String {
    let mut text = PlainText::new();
    let mut out = String::new();
    for chunk in chunks {
        out.push_str(&text.push(chunk));
    }
    out.push_str(&text.finish());
    out
}

#[test]
fn plain_text_drops_control_sequences() {
    // Colours, cursor addressing, private modes, a window title (ended by
    // BEL and by ST), a charset switch and keypad mode.
    assert_eq!(
        plain(&[
            b"\x1b[1;31mdown\x1b[0m \x1b[38;5;46mup\x1b[0m\r\n",
            b"\x1b[?1049h\x1b[2J\x1b[H\x1b]0;router\x07\x1b]2;r1\x1b\\\x1b(B\x1b=ok\x1b[?25h\r\n",
            b"\x07bell\x00\r\n",
        ]),
        "down up\nok\nbell\n"
    );
}

#[test]
fn plain_text_keeps_what_the_line_shows_after_edits() {
    // Backspace echo, a carriage return rewriting a progress line, a pager
    // prompt erased with backspaces and spaces, erase to end of line.
    assert_eq!(plain(&[b"r1# shw\x08 \x08ow ver\r\n"]), "r1# show ver\n");
    assert_eq!(plain(&[b"copy  10%\rcopy 100%\r\n"]), "copy 100%\n");
    assert_eq!(
        plain(&[
            b" --More-- ",
            b"\x08\x08\x08\x08\x08\x08\x08\x08\x08\x08          \x08\x08\x08\x08\x08\x08\x08\x08\x08\x08",
            b"interface Gi0/1\r\n"
        ]),
        "interface Gi0/1\n"
    );
    assert_eq!(plain(&[b"abcdef\x1b[3D\x1b[K\r\n"]), "abc\n");
    assert_eq!(plain(&[b"abcdef\r\x1b[2K\x1b[Cxy\r\n"]), " xy\n");
    // Tabs stay; trailing blanks go.
    assert_eq!(plain(&[b"a\tb   \r\n"]), "a\tb\n");
}

#[test]
fn plain_text_applies_character_erase_delete_and_insert() {
    // Line editors redraw with these (readline, Junos).
    assert_eq!(plain(&[b"abcdef\x1b[4D\x1b[2X\r\n"]), "ab  ef\n");
    assert_eq!(plain(&[b"abcdef\x1b[4D\x1b[2P\r\n"]), "abef\n");
    assert_eq!(plain(&[b"abef\x1b[2D\x1b[2@cd\r\n"]), "abcdef\n");
    // The 8-bit CSI (U+009B) is a sequence too, and a control character
    // inside a sequence still acts.
    assert_eq!(plain(&["\u{9b}1mbold\r\n".as_bytes()]), "bold\n");
    assert_eq!(plain(&[b"one\x1b[1\n;2mtwo\x1b[0m\r\n"]), "one\ntwo\n");
}

#[test]
fn plain_text_survives_huge_cursor_moves_and_endless_lines() {
    // Device output decides the counts; none may blow up the line.
    let text = plain(&[b"\x1b[999999999Cx\x1b[999999999Gy\x1b[999999999@z\r\n"]);
    assert!(text.len() < 20_000, "{} bytes", text.len());
    assert!(text.ends_with("z\n"), "{:?}", &text[text.len() - 10..]);
    // A line longer than the longest kept goes on in the next.
    let long = "a".repeat(10_000);
    let text = plain(&[format!("{long}\r\n").as_bytes()]);
    assert_eq!(text.replace('\n', ""), long);
    assert!(text.lines().all(|line| line.len() <= 4096));
}

#[test]
fn plain_text_handles_sequences_and_characters_split_across_reads() {
    assert_eq!(
        plain(&[
            b"\x1b[1;3",
            b"1mred\x1b",
            b"[0m \xe2\x94",
            b"\x82 box\r",
            b"\n"
        ]),
        "red \u{2502} box\n"
    );
    // Bytes that are not UTF-8 show as the replacement character.
    assert_eq!(plain(&[b"bad \xff byte\n"]), "bad \u{fffd} byte\n");
}

#[test]
fn plain_text_gives_complete_lines_and_the_rest_at_the_end() {
    let mut text = PlainText::new();
    assert_eq!(text.push(b"one\r\ntw"), "one\n");
    assert_eq!(text.push(b"o\r\nthree"), "two\n");
    assert_eq!(text.finish(), "three\n");
    assert_eq!(text.finish(), "");
    // Blank lines are kept.
    assert_eq!(plain(&[b"a\r\n\r\nb\r\n"]), "a\n\nb\n");
}
