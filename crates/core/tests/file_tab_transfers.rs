//! The Files tab's transfers through `enodix_core::files::FileBrowsers`
//! (#106), against the test SSH server: uploads into the folder shown and
//! downloads of named files into a local folder, which names are already
//! there (on the device and locally), the transfer states the interface
//! shows, cancelling, transfers waiting across a reconnect, closing a host,
//! and a device without SFTP, whose browser sends and fetches by path over
//! SCP and shows a refusal with its hint. Over SCP each transfer connects
//! and asks over the tab what its login asks (#109): a one-time code, a
//! host key trusted once.

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use enodix_core::files::{
    existing_locally, follow, Added, BrowseError, BrowserEvent, BrowserId, Direction, FileBrowsers,
    Listing, TransferState, TransferWatch,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::{HostKeyDecision, SshTarget};
use support::scp::ScpQuirk;
use support::{known_hosts, TestSshServer, CODE_QUESTION, ONE_TIME_CODE, OTP_USERNAME, WAIT};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

const HOST: &str = "r1";

async fn next(events: &mut UnboundedReceiver<BrowserEvent>) -> BrowserEvent {
    tokio::time::timeout(WAIT, events.recv())
        .await
        .expect("an event in time")
        .expect("an event")
}

/// A server serving `remote` over SFTP and a browser on it at `/`.
async fn connected(
    remote: &Path,
) -> (
    TestSshServer,
    FileBrowsers,
    BrowserId,
    UnboundedReceiver<BrowserEvent>,
) {
    let server = TestSshServer::start().await;
    server.serve_sftp(remote);
    let browsers = FileBrowsers::new();
    let (id, mut events) =
        browsers.open(HOST, server.target(), known_hosts(), Some("/".into()), None);
    match next(&mut events).await {
        BrowserEvent::Connected { .. } => {}
        other => panic!("expected a listing, got {other:?}"),
    }
    (server, browsers, id, events)
}

/// Every state `watch` goes through (as `follow` gives them), up to the
/// last.
async fn states(watch: TransferWatch) -> Vec<TransferState> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    tokio::time::timeout(
        WAIT,
        follow(watch, Duration::from_millis(10), move |state| {
            sender.send(state).is_ok()
        }),
    )
    .await
    .expect("the transfer ends in time");
    let mut all = Vec::new();
    while let Ok(state) = receiver.try_recv() {
        all.push(state);
    }
    all
}

async fn last(watch: TransferWatch) -> TransferState {
    states(watch).await.pop().expect("a state")
}

fn names(listing: &Listing) -> Vec<&str> {
    listing.entries.iter().map(|e| e.name.as_str()).collect()
}

#[tokio::test]
async fn uploads_go_into_the_folder_shown_one_after_the_other_and_the_listing_shows_them() {
    let remote = TempDir::new().unwrap();
    fs::create_dir(remote.path().join("flash")).unwrap();
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("a.cfg"), b"hostname a\n").unwrap();
    fs::write(local.path().join("b.bin"), vec![7u8; 300_000]).unwrap();
    let (_server, browsers, id, _events) = connected(remote.path()).await;

    let added = browsers
        .upload(
            id,
            "/flash",
            vec![local.path().join("a.cfg"), local.path().join("b.bin")],
        )
        .unwrap();
    let infos: Vec<&Added> = added.iter().map(|(added, _)| added).collect();
    assert_eq!(infos[0].direction, Direction::Upload);
    assert_eq!(infos[0].name, "a.cfg");
    assert_eq!(infos[0].remote, "/flash/a.cfg");
    assert_eq!(infos[1].remote, "/flash/b.bin");
    let mut watches = added.into_iter().map(|(_, watch)| watch);
    let first = watches.next().unwrap();
    let second = watches.next().unwrap();

    let first = states(first).await;
    match first.last().unwrap() {
        TransferState::Done {
            bytes,
            hashes,
            checked,
            ..
        } => {
            assert_eq!(*bytes, 11);
            let hashes = hashes.as_ref().expect("an upload's hashes");
            assert_eq!(hashes.md5.len(), 32);
            assert_eq!(hashes.sha256.len(), 64);
            assert!(*checked);
        }
        other => panic!("expected done, got {other:?}"),
    }
    match last(second).await {
        TransferState::Done { bytes, .. } => assert_eq!(bytes, 300_000),
        other => panic!("expected done, got {other:?}"),
    }
    assert_eq!(
        fs::read(remote.path().join("flash/a.cfg")).unwrap(),
        b"hostname a\n"
    );
    let listing = browsers.list(id, "/flash", "").await.unwrap();
    assert_eq!(names(&listing), ["a.cfg", "b.bin"]);
}

#[tokio::test]
async fn downloads_put_the_named_files_in_the_local_folder() {
    let remote = TempDir::new().unwrap();
    fs::write(remote.path().join("running.cfg"), b"hostname r1\n").unwrap();
    fs::write(remote.path().join("boot.bin"), vec![1u8; 100_000]).unwrap();
    let local = TempDir::new().unwrap();
    let (_server, browsers, id, _events) = connected(remote.path()).await;

    let added = browsers
        .download(
            id,
            "/",
            vec!["running.cfg".into(), "boot.bin".into()],
            local.path(),
        )
        .unwrap();
    assert_eq!(added[0].0.direction, Direction::Download);
    assert_eq!(added[0].0.local, local.path().join("running.cfg"));
    for (_, watch) in added {
        match last(watch).await {
            TransferState::Done { hashes, .. } => assert!(hashes.is_none()),
            other => panic!("expected done, got {other:?}"),
        }
    }
    assert_eq!(
        fs::read(local.path().join("running.cfg")).unwrap(),
        b"hostname r1\n"
    );
    assert_eq!(
        fs::read(local.path().join("boot.bin")).unwrap().len(),
        100_000
    );
}

#[tokio::test]
async fn the_names_already_there_are_told_on_the_device_and_locally() {
    let remote = TempDir::new().unwrap();
    fs::write(remote.path().join("a.cfg"), b"old").unwrap();
    let (_server, browsers, id, _events) = connected(remote.path()).await;
    let present = browsers
        .present(id, "/", vec!["a.cfg".into(), "new.cfg".into()])
        .await
        .unwrap();
    assert_eq!(present, ["a.cfg"]);

    let local = TempDir::new().unwrap();
    fs::write(local.path().join("x.txt"), b"").unwrap();
    assert_eq!(
        existing_locally(local.path(), &["x.txt".into(), "y.txt".into()]),
        ["x.txt"]
    );
}

#[tokio::test]
async fn a_running_upload_and_a_waiting_one_can_be_cancelled_and_say_what_was_left() {
    let remote = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("big.bin"), vec![3u8; 2 * 1024 * 1024]).unwrap();
    fs::write(local.path().join("small.txt"), b"x").unwrap();
    let (server, browsers, id, _events) = connected(remote.path()).await;
    server.sftp_quirks().hold_from(512 * 1024);

    let mut added = browsers
        .upload(
            id,
            "/",
            vec![local.path().join("big.bin"), local.path().join("small.txt")],
        )
        .unwrap()
        .into_iter();
    let (big, mut big_watch) = added.next().unwrap();
    let (small, small_watch) = added.next().unwrap();
    tokio::time::timeout(WAIT, async {
        while !matches!(
            TransferState::from(&big_watch.status()),
            TransferState::Running { done, .. } if done > 0
        ) {
            big_watch.changed().await;
        }
    })
    .await
    .expect("the upload runs");
    assert_eq!(
        TransferState::from(&small_watch.status()),
        TransferState::Waiting
    );

    assert!(browsers.cancel_transfer(small.id));
    match last(small_watch).await {
        TransferState::Cancelled { partial } => {
            assert_eq!(partial, "no partial file was left")
        }
        other => panic!("expected cancelled, got {other:?}"),
    }
    assert!(browsers.cancel_transfer(big.id));
    server.sftp_quirks().release();
    match last(big_watch).await {
        TransferState::Cancelled { partial } => {
            assert_eq!(partial, "the partial file was removed from the device")
        }
        other => panic!("expected cancelled, got {other:?}"),
    }
    assert!(!remote.path().join("big.bin").exists());
    assert!(!browsers.cancel_transfer(big.id), "it has ended");
}

#[tokio::test]
async fn transfers_waiting_when_the_connection_is_lost_run_once_the_tab_reconnects() {
    let remote = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("big.bin"), vec![3u8; 2 * 1024 * 1024]).unwrap();
    fs::write(local.path().join("next.txt"), b"next").unwrap();
    let (server, browsers, id, mut events) = connected(remote.path()).await;
    server.sftp_quirks().hold_from(512 * 1024);
    let mut added = browsers
        .upload(
            id,
            "/",
            vec![local.path().join("big.bin"), local.path().join("next.txt")],
        )
        .unwrap()
        .into_iter();
    let (_, big) = added.next().unwrap();
    let (_, waiting) = added.next().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.sever_connections();
    assert!(matches!(next(&mut events).await, BrowserEvent::Lost { .. }));
    match last(big).await {
        TransferState::Failed { reason, .. } => {
            assert!(reason.contains("connection"), "{reason}")
        }
        other => panic!("expected failed, got {other:?}"),
    }
    server.sftp_quirks().release();
    assert_eq!(
        TransferState::from(&waiting.status()),
        TransferState::Waiting
    );

    browsers.close(id);
    let (_, mut events) =
        browsers.open(HOST, server.target(), known_hosts(), Some("/".into()), None);
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::Connected { .. }
    ));
    assert!(matches!(last(waiting).await, TransferState::Done { .. }));
    assert_eq!(fs::read(remote.path().join("next.txt")).unwrap(), b"next");
}

#[tokio::test]
async fn closing_a_host_cancels_its_transfers_removes_the_partial_file_and_closes_its_browser() {
    let remote = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("big.bin"), vec![3u8; 2 * 1024 * 1024]).unwrap();
    let (server, browsers, id, _events) = connected(remote.path()).await;
    server.sftp_quirks().hold_from(512 * 1024);
    let (_, mut watch) = browsers
        .upload(id, "/", vec![local.path().join("big.bin")])
        .unwrap()
        .pop()
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while !matches!(
            TransferState::from(&watch.status()),
            TransferState::Running { done, .. } if done > 0
        ) {
            watch.changed().await;
        }
    })
    .await
    .expect("the upload runs");
    browsers.close_host(HOST);
    server.sftp_quirks().release();
    // The connection stays until the cancelled upload removed its file.
    match last(watch).await {
        TransferState::Cancelled { partial } => {
            assert_eq!(partial, "the partial file was removed from the device")
        }
        other => panic!("expected cancelled, got {other:?}"),
    }
    assert!(!remote.path().join("big.bin").exists());
    assert!(matches!(
        browsers.list(id, "/", "").await,
        Err(BrowseError::NotConnected)
    ));
}

#[tokio::test]
async fn a_finished_transfer_is_followed_to_its_end_even_when_slow_to_read() {
    let remote = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("a.bin"), vec![9u8; 1024 * 1024]).unwrap();
    let (_server, browsers, id, _events) = connected(remote.path()).await;
    let (_, watch) = browsers
        .upload(id, "/", vec![local.path().join("a.bin")])
        .unwrap()
        .pop()
        .unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    // Far slower than the transfer: the states in between are skipped,
    // never the last.
    follow(watch, Duration::from_millis(500), move |state| {
        sender.send(state).is_ok()
    })
    .await;
    let mut seen = Vec::new();
    while let Ok(state) = receiver.try_recv() {
        seen.push(state);
    }
    assert!(seen.len() <= 3, "{seen:?}");
    assert!(matches!(seen.last(), Some(TransferState::Done { .. })));
}

#[tokio::test]
async fn names_that_cannot_be_files_are_refused_before_anything_is_added() {
    let remote = TempDir::new().unwrap();
    let (_server, browsers, id, _events) = connected(remote.path()).await;
    let local = TempDir::new().unwrap();
    assert!(matches!(
        browsers.download(id, "/", vec!["../x".into()], local.path()),
        Err(BrowseError::BadName { .. })
    ));
    assert!(matches!(
        browsers.fetch(id, "flash:", local.path()),
        Err(BrowseError::BadName { .. })
    ));
    // Names a device may list (a backslash is an ordinary character on
    // Unix) that would leave the folder, or name another place, as a local
    // file on Windows: refused on every system, before anything is added.
    for name in [
        r"..\..\AppData\Roaming\x.bat",
        r"\Windows\x",
        r"C:\x",
        "C:x",
        r"\\server\share\x",
        "a:stream",
        "CON",
        "nul.txt",
        "bell\u{7}",
        "ends with a dot.",
    ] {
        let refused = browsers.download(id, "/", vec![name.into()], local.path());
        assert!(
            matches!(refused, Err(BrowseError::BadName { .. })),
            "{name:?}: {refused:?}"
        );
    }
    for remote in [r"/up/..\..\x.bat", r"/x\y"] {
        let refused = browsers.fetch(id, remote, local.path());
        assert!(
            matches!(refused, Err(BrowseError::BadName { .. })),
            "{remote:?}: {refused:?}"
        );
    }
    assert_eq!(fs::read_dir(local.path()).unwrap().count(), 0);
}

/// A server with SCP only, serving a new folder, and a browser on it.
async fn scp_only() -> (
    TestSshServer,
    TempDir,
    FileBrowsers,
    BrowserId,
    Option<String>,
) {
    let server = TestSshServer::start().await;
    let remote = TempDir::new().unwrap();
    server.serve_scp(remote.path());
    let browsers = FileBrowsers::new();
    let (id, mut events) = browsers.open(
        HOST,
        server.target(),
        known_hosts(),
        Some("/flash".into()),
        None,
    );
    let hint = match next(&mut events).await {
        BrowserEvent::TransferOnly {
            reason,
            hint,
            folder,
        } => {
            assert!(reason.contains("SFTP"), "{reason}");
            assert_eq!(folder.as_deref(), Some("/flash"));
            hint
        }
        other => panic!("expected transfer only, got {other:?}"),
    };
    (server, remote, browsers, id, hint)
}

#[tokio::test]
async fn without_sftp_the_browser_sends_and_fetches_by_path_holding_no_connection() {
    let (server, remote, browsers, id, hint) = scp_only().await;
    assert_eq!(hint, None, "no hint without a Junos platform");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.scp_running(), 0);
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("new.cfg"), b"hostname n\n").unwrap();
    fs::write(remote.path().join("old.cfg"), b"old").unwrap();

    let (added, watch) = browsers
        .send(id, local.path().join("new.cfg"), "/new.cfg")
        .unwrap();
    assert_eq!(added.name, "new.cfg");
    match last(watch).await {
        TransferState::Done { checked, size, .. } => {
            assert!(!checked);
            assert_eq!(size, "the size the device wrote could not be checked");
        }
        other => panic!("expected done, got {other:?}"),
    }
    assert_eq!(
        fs::read(remote.path().join("new.cfg")).unwrap(),
        b"hostname n\n"
    );
    // Over SCP nothing can be asked: no name is told as there.
    assert!(browsers
        .present(id, "/", vec!["old.cfg".into()])
        .await
        .unwrap()
        .is_empty());

    let (added, watch) = browsers.fetch(id, "/old.cfg", local.path()).unwrap();
    assert_eq!(added.local, local.path().join("old.cfg"));
    assert!(matches!(last(watch).await, TransferState::Done { .. }));
    assert_eq!(fs::read(local.path().join("old.cfg")).unwrap(), b"old");
    assert!(matches!(
        browsers.list(id, "/", "").await,
        Err(BrowseError::NotConnected)
    ));
}

#[tokio::test]
async fn a_refusal_over_scp_is_in_the_devices_words_with_the_hint_under_it() {
    let (server, _remote, browsers, id, _) = scp_only().await;
    server.scp_quirk(ScpQuirk::AdministrativelyDisabled);
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("a.cfg"), b"a").unwrap();
    let (_, watch) = browsers
        .send(id, local.path().join("a.cfg"), "/a.cfg")
        .unwrap();
    match last(watch).await {
        TransferState::Failed { reason, hint, .. } => {
            assert!(reason.contains("Administratively disabled"), "{reason}");
            assert!(
                hint.as_deref()
                    .is_some_and(|hint| hint.contains("ip scp server enable")),
                "{hint:?}"
            );
        }
        other => panic!("expected failed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_scp_connection_does_not_leave_the_next_send_waiting() {
    let (server, _remote, browsers, id, _) = scp_only().await;
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("a.cfg"), b"a").unwrap();
    // The server goes; the transfer cannot connect.
    let target = server.target();
    drop(server);
    let (_, watch) = browsers
        .send(id, local.path().join("a.cfg"), "/a.cfg")
        .unwrap();
    assert!(matches!(last(watch).await, TransferState::Failed { .. }));
    let (_, watch) = browsers
        .send(id, local.path().join("a.cfg"), "/a.cfg")
        .unwrap();
    assert!(
        matches!(last(watch).await, TransferState::Failed { .. }),
        "tried again rather than left waiting"
    );
    drop(target);
}

#[tokio::test]
async fn the_test_shell_turns_scp_off_and_on_for_the_smoke_test() {
    let (server, _remote, browsers, id, _) = scp_only().await;
    let manager = support::manager();
    let (session, mut shell) = manager.open(
        enodix_core::session::SessionTarget::Ssh(server.target()),
        enodix_core::session::TerminalSize { cols: 80, rows: 24 },
    );
    support::next_state(&mut shell).await;
    support::next_state(&mut shell).await;
    support::output_ending_with(&mut shell, support::PROMPT).await;
    let local = TempDir::new().unwrap();
    fs::write(local.path().join("a.cfg"), b"a").unwrap();

    manager.write(session, b"scp off\r".to_vec()).unwrap();
    support::output_ending_with(&mut shell, support::PROMPT).await;
    let (_, watch) = browsers
        .send(id, local.path().join("a.cfg"), "/a.cfg")
        .unwrap();
    assert!(matches!(
        last(watch).await,
        TransferState::Failed { hint: Some(_), .. }
    ));

    manager.write(session, b"scp on\r".to_vec()).unwrap();
    support::output_ending_with(&mut shell, support::PROMPT).await;
    let (_, watch) = browsers
        .send(id, local.path().join("a.cfg"), "/a.cfg")
        .unwrap();
    assert!(matches!(last(watch).await, TransferState::Done { .. }));
    manager.close(session).unwrap();
}

/// A server with SCP only, serving a new folder.
async fn scp_server() -> (TestSshServer, TempDir) {
    let server = TestSshServer::start().await;
    let remote = TempDir::new().unwrap();
    server.serve_scp(remote.path());
    (server, remote)
}

/// The one-time-code user's login at `server` (a password, then a code).
fn otp_target(server: &TestSshServer) -> SshTarget {
    let mut target = server.target();
    target.username = OTP_USERNAME.into();
    target
}

/// Opens a browser on an SCP-only device as the tab does, answering its
/// first login's questions: a host key with `decision`, the one-time code.
async fn open_scp(
    browsers: &FileBrowsers,
    target: SshTarget,
    known_hosts: KnownHosts,
    decision: HostKeyDecision,
) -> (BrowserId, UnboundedReceiver<BrowserEvent>) {
    let (id, mut events) = browsers.open(HOST, target, known_hosts, None, None);
    loop {
        match next(&mut events).await {
            BrowserEvent::HostKey { .. } => browsers.decide_host_key(id, decision),
            BrowserEvent::Prompt { .. } => browsers.answer(id, vec![ONE_TIME_CODE.into()]),
            BrowserEvent::TransferOnly { .. } => return (id, events),
            other => panic!("expected transfer only, got {other:?}"),
        }
    }
}

/// A local file `name` holding `bytes`, in `folder`.
fn local_file(folder: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = folder.path().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

/// Waits for the transfer to show as connecting.
async fn until_connecting(watch: &mut TransferWatch) {
    tokio::time::timeout(WAIT, async {
        while TransferState::from(&watch.status()) != TransferState::Connecting {
            watch.changed().await;
        }
    })
    .await
    .expect("the transfer connects");
}

/// The tab's next event, which must be the one-time code's question (the
/// password typed is not asked again).
async fn code_asked(events: &mut UnboundedReceiver<BrowserEvent>) {
    match next(events).await {
        BrowserEvent::Prompt { prompt } => {
            let asked: Vec<&str> = prompt.questions.iter().map(|q| q.text.as_str()).collect();
            assert_eq!(asked, [CODE_QUESTION]);
        }
        other => panic!("expected the code's question, got {other:?}"),
    }
}

/// No event comes for a while.
async fn nothing_asked(events: &mut UnboundedReceiver<BrowserEvent>) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    if let Ok(event) = events.try_recv() {
        panic!("nothing should be asked, got {event:?}");
    }
}

#[tokio::test]
async fn over_scp_each_send_asks_the_one_time_code_and_goes_through_once_answered() {
    let (server, remote) = scp_server().await;
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        otp_target(&server),
        known_hosts(),
        HostKeyDecision::Cancel,
    )
    .await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "a.cfg", b"hostname a\n");

    for round in 1..=2 {
        let (_, mut watch) = browsers.send(id, path.clone(), "/a.cfg").unwrap();
        until_connecting(&mut watch).await;
        code_asked(&mut events).await;
        assert_eq!(
            TransferState::from(&watch.status()),
            TransferState::Connecting,
            "it waits for the answer"
        );
        browsers.answer(id, vec![ONE_TIME_CODE.into()]);
        assert!(
            matches!(last(watch).await, TransferState::Done { .. }),
            "send {round}"
        );
    }
    assert_eq!(
        fs::read(remote.path().join("a.cfg")).unwrap(),
        b"hostname a\n"
    );
    // The tab's login and one for each send.
    assert_eq!(server.logins(), 3);
}

#[tokio::test]
async fn a_cancelled_question_fails_that_transfer_alone_and_the_next_one_asks_again() {
    let (server, remote) = scp_server().await;
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        otp_target(&server),
        known_hosts(),
        HostKeyDecision::Cancel,
    )
    .await;
    let local = TempDir::new().unwrap();
    let a = local_file(&local, "a.cfg", b"a");
    let b = local_file(&local, "b.cfg", b"b");

    let (_, first) = browsers.send(id, a, "/a.cfg").unwrap();
    let (_, second) = browsers.send(id, b, "/b.cfg").unwrap();
    code_asked(&mut events).await;
    browsers.cancel(id);
    match last(first).await {
        TransferState::Failed {
            reason, partial, ..
        } => {
            assert_eq!(reason, "the login was cancelled");
            assert_eq!(partial, "no partial file was left");
        }
        other => panic!("expected failed, got {other:?}"),
    }
    code_asked(&mut events).await;
    browsers.answer(id, vec![ONE_TIME_CODE.into()]);
    assert!(matches!(last(second).await, TransferState::Done { .. }));
    assert!(!remote.path().join("a.cfg").exists());
    assert_eq!(fs::read(remote.path().join("b.cfg")).unwrap(), b"b");
}

#[tokio::test]
async fn a_host_key_trusted_once_is_asked_about_for_each_transfer() {
    let (server, remote) = scp_server().await;
    fs::write(remote.path().join("old.cfg"), b"old").unwrap();
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        server.target(),
        KnownHosts::in_memory(),
        HostKeyDecision::TrustOnce,
    )
    .await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "a.cfg", b"a");

    let (_, send) = browsers.send(id, path.clone(), "/a.cfg").unwrap();
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::HostKey { .. }
    ));
    browsers.decide_host_key(id, HostKeyDecision::TrustOnce);
    assert!(matches!(last(send).await, TransferState::Done { .. }));

    let (_, fetch) = browsers.fetch(id, "/old.cfg", local.path()).unwrap();
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::HostKey { .. }
    ));
    browsers.decide_host_key(id, HostKeyDecision::TrustOnce);
    assert!(matches!(last(fetch).await, TransferState::Done { .. }));

    // Refused, the key fails that transfer with the refusal's reason; the
    // next one asks again.
    let (_, refused) = browsers.send(id, path.clone(), "/b.cfg").unwrap();
    let (_, next_send) = browsers.send(id, path, "/c.cfg").unwrap();
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::HostKey { .. }
    ));
    browsers.decide_host_key(id, HostKeyDecision::Cancel);
    match last(refused).await {
        TransferState::Failed { reason, .. } => assert!(reason.contains("host key"), "{reason}"),
        other => panic!("expected failed, got {other:?}"),
    }
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::HostKey { .. }
    ));
    browsers.decide_host_key(id, HostKeyDecision::TrustOnce);
    assert!(matches!(last(next_send).await, TransferState::Done { .. }));
    assert!(!remote.path().join("b.cfg").exists());
    assert!(remote.path().join("c.cfg").exists());
}

#[tokio::test]
async fn a_stored_profile_with_remembered_keys_asks_nothing_per_transfer() {
    let (server, _remote) = scp_server().await;
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        server.target(),
        known_hosts(),
        HostKeyDecision::Cancel,
    )
    .await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "a.cfg", b"a");
    for _ in 0..2 {
        let (_, watch) = browsers.send(id, path.clone(), "/a.cfg").unwrap();
        assert!(matches!(last(watch).await, TransferState::Done { .. }));
    }
    nothing_asked(&mut events).await;
}

#[tokio::test]
async fn an_unknown_key_trusted_and_remembered_is_asked_only_once() {
    let (server, _remote) = scp_server().await;
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        server.target(),
        KnownHosts::in_memory(),
        HostKeyDecision::TrustAndRemember,
    )
    .await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "a.cfg", b"a");
    for _ in 0..2 {
        let (_, watch) = browsers.send(id, path.clone(), "/a.cfg").unwrap();
        assert!(matches!(last(watch).await, TransferState::Done { .. }));
    }
    nothing_asked(&mut events).await;
}

#[tokio::test]
async fn a_question_left_open_longer_than_the_stall_limit_does_not_fail_the_transfer() {
    let (server, remote) = scp_server().await;
    let mut target = otp_target(&server);
    // The stall limit is three connect timeouts: 3 s.
    target.connect_timeout = Duration::from_secs(1);
    let browsers = FileBrowsers::new();
    let (id, mut events) =
        open_scp(&browsers, target, known_hosts(), HostKeyDecision::Cancel).await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "a.cfg", b"a");

    let (_, mut watch) = browsers.send(id, path, "/a.cfg").unwrap();
    until_connecting(&mut watch).await;
    code_asked(&mut events).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        TransferState::from(&watch.status()),
        TransferState::Connecting
    );
    browsers.answer(id, vec![ONE_TIME_CODE.into()]);
    assert!(matches!(last(watch).await, TransferState::Done { .. }));
    assert!(remote.path().join("a.cfg").exists());
}

#[tokio::test]
async fn a_transfer_cancelled_while_it_asks_takes_its_question_off_the_tab() {
    let (server, _remote) = scp_server().await;
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        otp_target(&server),
        known_hosts(),
        HostKeyDecision::Cancel,
    )
    .await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "a.cfg", b"a");
    let (added, watch) = browsers.send(id, path, "/a.cfg").unwrap();
    code_asked(&mut events).await;
    assert!(browsers.cancel_transfer(added.id));
    assert!(matches!(last(watch).await, TransferState::Cancelled { .. }));
    assert!(matches!(
        next(&mut events).await,
        BrowserEvent::QuestionClosed
    ));
}

#[tokio::test]
async fn closing_the_browser_with_a_question_open_cancels_the_transfer_and_the_next_asks_the_new_one(
) {
    let (server, remote) = scp_server().await;
    let browsers = FileBrowsers::new();
    let (id, mut events) = open_scp(
        &browsers,
        otp_target(&server),
        known_hosts(),
        HostKeyDecision::Cancel,
    )
    .await;
    let local = TempDir::new().unwrap();
    let a = local_file(&local, "a.cfg", b"a");
    let b = local_file(&local, "b.cfg", b"b");
    let (_, first) = browsers.send(id, a, "/a.cfg").unwrap();
    let (_, second) = browsers.send(id, b, "/b.cfg").unwrap();
    code_asked(&mut events).await;

    // A Reconnect: the browser closes and a new one opens.
    browsers.close(id);
    assert!(matches!(last(first).await, TransferState::Cancelled { .. }));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        TransferState::from(&second.status()),
        TransferState::Waiting,
        "it waits for the new browser"
    );
    let (id, mut events) = open_scp(
        &browsers,
        otp_target(&server),
        known_hosts(),
        HostKeyDecision::Cancel,
    )
    .await;
    code_asked(&mut events).await;
    browsers.answer(id, vec![ONE_TIME_CODE.into()]);
    assert!(matches!(last(second).await, TransferState::Done { .. }));
    assert_eq!(fs::read(remote.path().join("b.cfg")).unwrap(), b"b");

    // Closing the tab gives a question up too.
    let (_, third) = browsers
        .send(id, local.path().join("a.cfg"), "/a.cfg")
        .unwrap();
    code_asked(&mut events).await;
    browsers.close_host(HOST);
    assert!(matches!(last(third).await, TransferState::Cancelled { .. }));
}
