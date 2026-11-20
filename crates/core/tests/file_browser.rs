//! The Files tab's browsing (`netterm_core::files::FileBrowsers`), against
//! the test SSH server's SFTP side: opening (stored login, host-key and
//! login questions, cancelling), the starting folder, the listing's order,
//! the path box, going up, rename, delete, new folder, a cut connection,
//! closing, and a device without SFTP.

mod support;

use std::fs;

use netterm_core::files::{
    join_path, parent_folder, resolve_path, BrowseError, BrowserEvent, BrowserId, EntryKind,
    FileBrowsers, FileError, Listing,
};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::{HostKeyDecision, SessionState, SessionTarget, TerminalSize};
use support::{
    known_hosts, manager, next_state, output_ending_with, TestSshServer, CODE_QUESTION,
    ONE_TIME_CODE, OTP_USERNAME, PROMPT, WAIT,
};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

/// The host the browsers belong to.
const HOST: &str = "r1";

struct Opened {
    server: TestSshServer,
    folder: TempDir,
    browsers: FileBrowsers,
    id: BrowserId,
    events: UnboundedReceiver<BrowserEvent>,
    listing: Listing,
}

impl Opened {
    /// A server serving a new temporary folder (holding `files`, a path
    /// ending in `/` being a folder) and a browser connected to it,
    /// starting in `start`.
    async fn start(files: &[&str], start: Option<&str>) -> Self {
        let server = TestSshServer::start().await;
        let folder = TempDir::new().unwrap();
        for file in files {
            let path = folder.path().join(file.trim_end_matches('/'));
            if file.ends_with('/') {
                fs::create_dir_all(path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"hostname r1\n").unwrap();
            }
        }
        server.serve_sftp(folder.path());
        let browsers = FileBrowsers::new();
        let (id, mut events) = browsers.open(
            HOST,
            server.target(),
            known_hosts(),
            start.map(str::to_owned),
            None,
        );
        let listing = match next(&mut events).await {
            BrowserEvent::Connected { listing, .. } => listing,
            other => panic!("expected a listing, got {other:?}"),
        };
        Self {
            server,
            folder,
            browsers,
            id,
            events,
            listing,
        }
    }

    fn exists(&self, path: &str) -> bool {
        self.folder.path().join(path).exists()
    }
}

async fn next(events: &mut UnboundedReceiver<BrowserEvent>) -> BrowserEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("an event in time")
        .expect("an event")
}

fn names(listing: &Listing) -> Vec<&str> {
    listing.entries.iter().map(|e| e.name.as_str()).collect()
}

/// Waits until `count()` is `expected`.
async fn until(what: &str, expected: usize, count: impl Fn() -> usize) {
    let waited = tokio::time::timeout(WAIT, async {
        while count() != expected {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}: {} instead of {expected}", count());
}

#[tokio::test]
async fn with_a_stored_login_it_lists_the_login_folder_folders_first_then_by_name() {
    let opened = Opened::start(
        &["b.txt", "A.cfg", ".hidden", "zeta/", "Alpha/", "logs/x.log"],
        None,
    )
    .await;
    assert_eq!(opened.listing.folder, "/");
    assert_eq!(opened.listing.parent, None, "the top has no way up");
    assert_eq!(
        names(&opened.listing),
        ["Alpha", "logs", "zeta", ".hidden", "A.cfg", "b.txt"]
    );
    let file = &opened.listing.entries[4];
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.size, Some(12));
    assert!(file.modified.is_some());
    assert!(file.permissions.is_some());
    assert_eq!(opened.server.shell_requests(), 0);
}

#[tokio::test]
async fn it_starts_in_the_folder_given_and_falls_back_to_the_login_folder_with_a_notice() {
    let opened = Opened::start(&["flash/boot.bin"], Some("/flash")).await;
    assert_eq!(opened.listing.folder, "/flash");
    assert_eq!(opened.listing.parent.as_deref(), Some("/"));
    assert_eq!(names(&opened.listing), ["boot.bin"]);

    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    let browsers = FileBrowsers::new();
    let (_, mut events) = browsers.open(
        HOST,
        server.target(),
        known_hosts(),
        Some("/nowhere".into()),
        None,
    );
    match next(&mut events).await {
        BrowserEvent::Connected { listing, notice } => {
            assert_eq!(listing.folder, "/");
            let notice = notice.expect("a notice");
            assert!(notice.contains("/nowhere"), "{notice}");
        }
        other => panic!("expected a listing, got {other:?}"),
    }
}

#[tokio::test]
async fn the_path_box_goes_to_absolute_and_relative_paths_and_a_missing_one_says_so() {
    let opened = Opened::start(&["logs/old/a.log", "logs/b.log"], None).await;
    let b = &opened.browsers;

    let logs = b.list(opened.id, "/", "logs").await.unwrap();
    assert_eq!(logs.folder, "/logs");
    assert_eq!(names(&logs), ["old", "b.log"]);

    let old = b.list(opened.id, "/logs", "old/").await.unwrap();
    assert_eq!(old.folder, "/logs/old");
    let up = b.list(opened.id, &old.folder, "..").await.unwrap();
    assert_eq!(up.folder, "/logs");
    let absolute = b.list(opened.id, "/logs", "/logs/old").await.unwrap();
    assert_eq!(absolute.folder, "/logs/old");
    let refreshed = b.list(opened.id, "/logs", "").await.unwrap();
    assert_eq!(refreshed.folder, "/logs");

    match b.list(opened.id, "/", "/missing").await {
        Err(BrowseError::File(FileError::NotFound { path })) => assert!(path.contains("missing")),
        other => panic!("expected not found, got {other:?}"),
    }
}

#[test]
fn paths_go_up_and_join_as_the_device_writes_them() {
    assert_eq!(parent_folder("/"), None);
    assert_eq!(parent_folder("/var"), Some("/".into()));
    assert_eq!(parent_folder("/var/tmp/"), Some("/var".into()));
    assert_eq!(parent_folder("flash:"), None);
    assert_eq!(parent_folder("flash:/"), None);
    assert_eq!(parent_folder("flash:/dir"), Some("flash:/".into()));
    assert_eq!(parent_folder("flash:dir"), Some("flash:".into()));
    assert_eq!(parent_folder("bootflash:/a/b"), Some("bootflash:/a".into()));

    assert_eq!(join_path("/", "a"), "/a");
    assert_eq!(join_path("/var", "a"), "/var/a");
    assert_eq!(join_path("flash:", "a"), "flash:a");
    assert_eq!(join_path("flash:/", "a"), "flash:/a");

    assert_eq!(resolve_path("/var", ""), "/var");
    assert_eq!(resolve_path("/var", " tmp "), "/var/tmp");
    assert_eq!(resolve_path("/var", "/etc"), "/etc");
    assert_eq!(resolve_path("/var", "flash:"), "flash:");
}

#[tokio::test]
async fn rename_delete_and_new_folder_change_the_device_and_return_the_new_listing() {
    let opened = Opened::start(&["a.txt", "b.txt", "empty/", "full/x"], None).await;
    let (b, id) = (&opened.browsers, opened.id);

    let listing = b.rename(id, "/", "a.txt", "c.txt").await.unwrap();
    assert_eq!(names(&listing), ["empty", "full", "b.txt", "c.txt"]);
    assert!(opened.exists("c.txt") && !opened.exists("a.txt"));

    let listing = b.make_folder(id, "/", "new").await.unwrap();
    assert!(names(&listing).contains(&"new"));
    assert!(opened.folder.path().join("new").is_dir());

    let listing = b.delete(id, "/", "b.txt", EntryKind::File).await.unwrap();
    assert!(!names(&listing).contains(&"b.txt"));
    let listing = b.delete(id, "/", "empty", EntryKind::Folder).await.unwrap();
    assert!(!names(&listing).contains(&"empty"));

    match b.delete(id, "/", "full", EntryKind::Folder).await {
        Err(BrowseError::File(FileError::NotEmpty { .. })) => {}
        other => panic!("expected not empty, got {other:?}"),
    }
    let error = b.rename(id, "/", "c.txt", "new").await.unwrap_err();
    assert!(
        matches!(error, BrowseError::File(FileError::AlreadyExists { .. })),
        "{error:?}"
    );
    assert!(error.to_string().contains("exists already"), "{error}");
    assert!(opened.exists("c.txt"), "nothing was replaced");
}

#[tokio::test]
async fn a_name_with_a_slash_or_no_name_is_refused_before_asking_the_device() {
    let opened = Opened::start(&["a.txt"], None).await;
    let (b, id) = (&opened.browsers, opened.id);
    for name in ["", "  ", "x/y", ".", ".."] {
        assert!(
            matches!(
                b.make_folder(id, "/", name).await,
                Err(BrowseError::BadName { .. })
            ),
            "{name:?}"
        );
        assert!(matches!(
            b.rename(id, "/", "a.txt", name).await,
            Err(BrowseError::BadName { .. })
        ));
    }
    assert!(opened.exists("a.txt"));
}

#[tokio::test]
async fn an_unknown_host_key_is_asked_and_cancelling_it_says_why() {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    let browsers = FileBrowsers::new();

    let (id, mut events) =
        browsers.open(HOST, server.target(), KnownHosts::in_memory(), None, None);
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::HostKey { .. }
    ));
    browsers.decide_host_key(id, HostKeyDecision::Cancel);
    match next(&mut events).await {
        BrowserEvent::Failed { reason } => assert!(reason.contains("host key"), "{reason}"),
        other => panic!("expected a failure, got {other:?}"),
    }

    let (id, mut events) =
        browsers.open(HOST, server.target(), KnownHosts::in_memory(), None, None);
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::HostKey { .. }
    ));
    browsers.decide_host_key(id, HostKeyDecision::TrustOnce);
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::Connected { .. }
    ));
}

#[tokio::test]
async fn a_login_question_is_asked_and_answered_or_cancelled() {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    let browsers = FileBrowsers::new();
    let mut target = server.target();
    target.username = OTP_USERNAME.into();

    let (id, mut events) = browsers.open(HOST, target.clone(), known_hosts(), None, None);
    match next(&mut events).await {
        BrowserEvent::Prompt { prompt } => assert_eq!(prompt.questions[0].text, CODE_QUESTION),
        other => panic!("expected a question, got {other:?}"),
    }
    browsers.answer(id, vec![ONE_TIME_CODE.into()]);
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::Connected { .. }
    ));

    let (id, mut events) = browsers.open(HOST, target, known_hosts(), None, None);
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::Prompt { .. }
    ));
    browsers.cancel(id);
    match next(&mut events).await {
        BrowserEvent::Failed { reason } => assert!(reason.contains("cancelled"), "{reason}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(server.logins(), 1, "only the answered login got in");
}

#[tokio::test]
async fn a_cut_connection_is_reported_with_its_reason_and_a_new_one_lists_again() {
    let mut opened = Opened::start(&["a.txt"], None).await;
    opened.server.cut_connections().await;
    match next(&mut opened.events).await {
        BrowserEvent::Lost { reason } => {
            assert!(reason.contains("closed the connection"), "{reason}")
        }
        other => panic!("expected the loss, got {other:?}"),
    }
    assert!(matches!(
        opened.browsers.list(opened.id, "/", "").await,
        Err(BrowseError::NotConnected)
    ));

    opened.browsers.close(opened.id);
    let (_, mut events) = opened.browsers.open(
        HOST,
        opened.server.target(),
        known_hosts(),
        Some("/".into()),
        None,
    );
    match next(&mut events).await {
        BrowserEvent::Connected { listing, .. } => assert_eq!(names(&listing), ["a.txt"]),
        other => panic!("expected a listing, got {other:?}"),
    }
}

#[tokio::test]
async fn closing_closes_the_connection_and_a_terminal_session_beside_it_goes_on() {
    let opened = Opened::start(&["a.txt"], None).await;
    let manager = manager();
    let (session, mut session_events) = manager.open(
        SessionTarget::Ssh(opened.server.target()),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(
        next_state(&mut session_events).await,
        SessionState::Connecting
    );
    assert_eq!(
        next_state(&mut session_events).await,
        SessionState::Connected
    );
    until("connections", 2, || opened.server.connections_alive()).await;

    opened.browsers.close(opened.id);
    until("connections", 1, || opened.server.connections_alive()).await;
    assert!(matches!(
        opened.browsers.list(opened.id, "/", "").await,
        Err(BrowseError::NotConnected)
    ));
    manager.write(session, b"x".to_vec()).unwrap();
    assert_eq!(
        opened.server.connections_alive(),
        1,
        "the session is still up"
    );
    manager.close(session).unwrap();
}

#[tokio::test]
async fn a_device_without_sftp_says_so_and_offers_transfer_only() {
    let server = TestSshServer::start().await;
    server.refuse_sftp();
    let browsers = FileBrowsers::new();
    let (_, mut events) = browsers.open(HOST, server.target(), known_hosts(), None, None);
    match next(&mut events).await {
        BrowserEvent::TransferOnly { reason, .. } => assert!(reason.contains("SFTP"), "{reason}"),
        other => panic!("expected transfer only, got {other:?}"),
    }
}

#[tokio::test]
async fn the_test_shell_counts_and_cuts_sftp_connections_leaving_itself_up() {
    let mut opened = Opened::start(&["a.txt"], None).await;
    let manager = manager();
    let (session, mut shell) = manager.open(
        SessionTarget::Ssh(opened.server.target()),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(next_state(&mut shell).await, SessionState::Connecting);
    assert_eq!(next_state(&mut shell).await, SessionState::Connected);
    output_ending_with(&mut shell, PROMPT).await;

    manager.write(session, b"show sftp\r".to_vec()).unwrap();
    let shown = output_ending_with(&mut shell, PROMPT).await;
    assert!(String::from_utf8_lossy(&shown).contains("sftp connections: 1"));

    manager.write(session, b"cut sftp\r".to_vec()).unwrap();
    output_ending_with(&mut shell, PROMPT).await;
    match next(&mut opened.events).await {
        BrowserEvent::Lost { reason } => assert!(reason.contains("going down"), "{reason}"),
        other => panic!("expected the loss, got {other:?}"),
    }
    manager.write(session, b"show sftp\r".to_vec()).unwrap();
    let shown = output_ending_with(&mut shell, PROMPT).await;
    assert!(String::from_utf8_lossy(&shown).contains("sftp connections: 0"));
    manager.close(session).unwrap();
}
