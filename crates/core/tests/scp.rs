//! SCP on a file connection against the test SSH server's SCP side: sending
//! and fetching one file, the fall back from SFTP, a connection per
//! transfer, and how devices answer (refusals in their own words, the two
//! hints, a short fetch, a cancelled send).

mod support;

use std::fs;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use netterm_core::files::{
    FileConnection, FileError, Files, NoSftp, Protocol, Scp, ScpError, StartError,
};
use netterm_core::platform::built_in_platform;
use netterm_core::session::{SshAuth, SshTarget};
use support::scp::{ScpQuirk, NOT_FOUND, PLAIN_TEXT_REFUSAL};
use support::{known_hosts, TestSshServer, PASSWORD, USERNAME, WAIT};
use tempfile::TempDir;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::Notify;

/// The IOS hint and the Junos one, as the tests recognise them.
const IOS_HINT: &str = "ip scp server enable";
const JUNOS_HINT: &str = "set system services ssh sftp-server";

/// A server serving a new temporary folder over SCP (and refusing SFTP).
async fn scp_server() -> (TestSshServer, TempDir) {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_scp(folder.path());
    (server, folder)
}

async fn connected(target: SshTarget) -> FileConnection {
    FileConnection::open_unattended(target, known_hosts())
        .await
        .unwrap()
}

/// What SFTP's absence turns a connection into: SCP.
async fn fallen_back(server: &TestSshServer) -> Scp {
    match connected(server.target()).await.start_files(None).await {
        Ok(Files::Scp(scp)) => scp,
        Ok(Files::Sftp { .. }) => panic!("SFTP started"),
        Err(error) => panic!("{error}"),
    }
}

/// Bytes that do not repeat in short runs.
fn content(size: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_u32;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

async fn send(connection: &FileConnection, path: &str, bytes: &[u8]) -> Result<(), ScpError> {
    connection
        .scp_send(
            path,
            bytes.len() as u64,
            bytes,
            std::future::pending::<()>(),
        )
        .await
}

async fn fetch(connection: &FileConnection, path: &str) -> Result<Vec<u8>, ScpError> {
    let mut fetched = Vec::new();
    let size = connection.scp_fetch(path, &mut fetched).await?;
    assert_eq!(size, fetched.len() as u64);
    Ok(fetched)
}

async fn until(what: &str, expected: usize, count: impl Fn() -> usize) {
    let waited = tokio::time::timeout(WAIT, async {
        while count() != expected {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}: {} instead of {expected}", count());
}

/// The device's refusal: its words and the core's hint.
fn refusal(error: ScpError) -> (String, Option<&'static str>) {
    let hint = error.hint();
    match error {
        ScpError::Refused { message, .. } => (message, hint),
        other => panic!("not a refusal: {other:?}"),
    }
}

#[tokio::test]
async fn a_file_sent_and_fetched_back_is_the_same_byte_for_byte() {
    let (server, folder) = scp_server().await;
    let bytes = content(3 * 1024 * 1024 + 17);

    let connection = connected(server.target()).await;
    send(&connection, "/image.bin", &bytes).await.unwrap();
    assert!(fs::read(folder.path().join("image.bin")).unwrap() == bytes);
    let fetched = fetch(&connection, "/image.bin").await.unwrap();
    assert!(fetched == bytes, "fetched {} bytes", fetched.len());
}

#[tokio::test]
async fn a_file_goes_and_comes_back_through_a_jump_host() {
    let bastion = TestSshServer::start().await;
    let (router, folder) = scp_server().await;
    let mut target = router.target();
    target.jump_hosts.push(netterm_core::session::JumpHost {
        name: "bastion".into(),
        host: "127.0.0.1".into(),
        port: bastion.port(),
        username: USERNAME.into(),
        auth: SshAuth::Password(PASSWORD.into()),
        legacy_crypto: false,
        connect_timeout: target.connect_timeout,
    });
    let bytes = content(2 * 1024 * 1024);

    let connection = connected(target).await;
    send(&connection, "/via-jump.bin", &bytes).await.unwrap();
    assert!(fs::read(folder.path().join("via-jump.bin")).unwrap() == bytes);
    assert!(fetch(&connection, "/via-jump.bin").await.unwrap() == bytes);
}

#[tokio::test]
async fn a_device_that_refuses_sftp_falls_back_to_scp_and_holds_no_connection() {
    let (server, folder) = scp_server().await;
    let files = connected(server.target())
        .await
        .start_files(None)
        .await
        .unwrap();
    assert_eq!(files.protocol(), Protocol::Scp);
    let Files::Scp(scp) = files else {
        unreachable!()
    };
    assert_eq!(scp.no_sftp(), NoSftp::Refused);
    assert_eq!(scp.hint(), None);
    until("connections", 0, || server.connections_alive()).await;

    // Each transfer connects again.
    scp.send("/a.txt", 5, &b"hello"[..], std::future::pending::<()>())
        .await
        .unwrap();
    assert_eq!(fs::read(folder.path().join("a.txt")).unwrap(), b"hello");
    let mut fetched = Vec::new();
    scp.fetch("/a.txt", &mut fetched).await.unwrap();
    assert_eq!(fetched, b"hello");
    assert_eq!(server.logins(), 3);
    until("connections", 0, || server.connections_alive()).await;
}

#[tokio::test]
async fn a_device_that_accepts_sftp_then_closes_falls_back_to_scp() {
    let (server, _folder) = scp_server().await;
    server.close_sftp_at_once();
    let scp = fallen_back(&server).await;
    assert_eq!(scp.no_sftp(), NoSftp::ClosedBeforeReply);
}

#[tokio::test]
async fn a_device_that_closes_sftp_and_then_the_connection_falls_back_to_scp() {
    // IOS-XE as it really behaves (#108, seen on the lab's CSR 1000v in a
    // file job): it ends the whole connection when the SFTP channel ends,
    // so the connection is gone by the time the client asks why SFTP did
    // not start. That is still a device without SFTP, not a lost
    // connection: the device closed the channel in order first.
    let (server, folder) = scp_server().await;
    server.close_sftp_and_hang_up();
    for _ in 0..10 {
        let scp = fallen_back(&server).await;
        assert_eq!(scp.no_sftp(), NoSftp::ClosedBeforeReply);
    }
    let scp = fallen_back(&server).await;
    scp.send("/a.txt", 5, &b"hello"[..], std::future::pending::<()>())
        .await
        .unwrap();
    assert_eq!(fs::read(folder.path().join("a.txt")).unwrap(), b"hello");
}

#[tokio::test]
async fn a_device_with_sftp_keeps_sftp_and_its_connection() {
    let (server, folder) = scp_server().await;
    server.serve_sftp(folder.path());
    let files = connected(server.target())
        .await
        .start_files(None)
        .await
        .unwrap();
    assert_eq!(files.protocol(), Protocol::Sftp);
    assert!(files.list("/").await.unwrap().is_empty());
    assert_eq!(server.connections_alive(), 1);
}

#[tokio::test]
async fn a_failed_connection_is_not_a_fall_back() {
    let (server, _folder) = scp_server().await;
    let connection = connected(server.target()).await;
    server.cut_connections().await;
    tokio::time::timeout(WAIT, connection.lost()).await.unwrap();
    let failed = connection.start_files(None).await;
    assert!(
        matches!(failed, Err(StartError::Connection(_))),
        "{failed:?}"
    );
}

#[tokio::test]
async fn two_transfers_in_a_row_work_against_a_device_that_closes_after_each() {
    let (server, folder) = scp_server().await;
    server.scp_quirk(ScpQuirk::CloseConnectionAfter);
    let scp = fallen_back(&server).await;

    scp.send("/one.txt", 3, &b"one"[..], std::future::pending::<()>())
        .await
        .unwrap();
    scp.send("/two.txt", 3, &b"two"[..], std::future::pending::<()>())
        .await
        .unwrap();
    let mut fetched = Vec::new();
    scp.fetch("/one.txt", &mut fetched).await.unwrap();
    assert_eq!(fetched, b"one");
    assert_eq!(fs::read(folder.path().join("two.txt")).unwrap(), b"two");

    // Which is why: on one connection, the second transfer finds it gone.
    let connection = connected(server.target()).await;
    send(&connection, "/three.txt", b"three").await.unwrap();
    tokio::time::timeout(WAIT, connection.lost()).await.unwrap();
    assert_eq!(
        send(&connection, "/four.txt", b"four").await,
        Err(ScpError::ConnectionLost)
    );
}

#[tokio::test]
async fn ios_refusing_scp_gives_its_words_and_the_hint_to_turn_it_on() {
    let (server, _folder) = scp_server().await;
    server.scp_quirk(ScpQuirk::AdministrativelyDisabled);
    let connection = connected(server.target()).await;

    let (message, hint) = refusal(send(&connection, "flash:x.bin", b"x").await.unwrap_err());
    assert_eq!(message, "Administratively disabled.");
    assert!(hint.unwrap().contains(IOS_HINT));
    let (message, hint) = refusal(fetch(&connection, "flash:x.bin").await.unwrap_err());
    assert_eq!(message, "Administratively disabled.");
    assert!(hint.unwrap().contains(IOS_HINT));
}

#[tokio::test]
async fn plain_text_without_an_error_byte_is_the_devices_refusal() {
    let (server, _folder) = scp_server().await;
    server.scp_quirk(ScpQuirk::PlainText);
    let connection = connected(server.target()).await;

    let (message, hint) = refusal(send(&connection, "/x.bin", b"x").await.unwrap_err());
    assert_eq!(message, PLAIN_TEXT_REFUSAL);
    assert_eq!(hint, None);
    let (message, hint) = refusal(fetch(&connection, "/x.bin").await.unwrap_err());
    assert_eq!(message, PLAIN_TEXT_REFUSAL);
    assert_eq!(hint, None);
}

#[tokio::test]
async fn a_host_without_the_scp_program_says_so_through_stderr_and_its_exit_status() {
    let (server, _folder) = scp_server().await;
    server.scp_quirk(ScpQuirk::NoProgram);
    let connection = connected(server.target()).await;

    let (message, hint) = refusal(send(&connection, "/x.bin", b"x").await.unwrap_err());
    assert!(message.contains(NOT_FOUND), "{message}");
    assert!(message.contains("127"), "{message}");
    assert_eq!(hint, None);
    let (message, hint) = refusal(fetch(&connection, "/x.bin").await.unwrap_err());
    assert!(message.contains(NOT_FOUND), "{message}");
    assert!(message.contains("127"), "{message}");
    assert_eq!(hint, None);
}

#[tokio::test]
async fn after_a_fall_back_the_caller_gets_what_scp_answered() {
    // cEOS at privilege 1: SFTP accepted and closed, as on an SCP-only
    // device, then SCP refused in plain text.
    let (server, _folder) = scp_server().await;
    server.close_sftp_at_once();
    server.scp_quirk(ScpQuirk::PlainText);
    let scp = fallen_back(&server).await;

    let failed = scp
        .send("/x.bin", 1, &b"x"[..], std::future::pending::<()>())
        .await
        .unwrap_err();
    let (message, hint) = refusal(failed);
    assert_eq!(message, PLAIN_TEXT_REFUSAL);
    assert_eq!(hint, None);
}

#[tokio::test]
async fn a_write_failure_after_the_data_is_the_devices_refusal() {
    let (server, _folder) = scp_server().await;
    server.scp_quirk(ScpQuirk::WriteFailedAfterData);
    let connection = connected(server.target()).await;

    let (message, hint) = refusal(
        send(&connection, "flash:nope/x.bin", &content(100_000))
            .await
            .unwrap_err(),
    );
    assert_eq!(message, "Write failed");
    assert_eq!(hint, None);
}

#[tokio::test]
async fn a_fetch_whose_header_names_the_file_without_its_folder_succeeds() {
    let (server, folder) = scp_server().await;
    fs::write(folder.path().join("ten.bin"), b"0123456789").unwrap();
    let connection = connected(server.target()).await;
    assert_eq!(
        fetch(&connection, "bootflash:ten.bin").await.unwrap(),
        b"0123456789"
    );
}

#[tokio::test]
async fn a_fetch_of_a_missing_file_fails_with_the_devices_text() {
    let (server, _folder) = scp_server().await;
    let connection = connected(server.target()).await;
    let (message, hint) = refusal(fetch(&connection, "/nope.bin").await.unwrap_err());
    assert_eq!(message, "scp: /nope.bin: No such file or directory");
    assert_eq!(hint, None);
}

#[tokio::test]
async fn a_fetch_that_delivers_fewer_bytes_than_its_header_says_fails() {
    let (server, folder) = scp_server().await;
    server.scp_quirk(ScpQuirk::ShortFetch);
    fs::write(folder.path().join("half.bin"), content(200_000)).unwrap();
    let connection = connected(server.target()).await;

    let failed = fetch(&connection, "/half.bin").await.unwrap_err();
    assert_eq!(
        failed,
        ScpError::Short {
            expected: 200_000,
            received: 100_000
        }
    );
    assert_eq!(failed.hint(), None);
}

#[tokio::test]
async fn a_junos_host_falling_back_to_scp_carries_the_hint_for_a_listing() {
    let (server, _folder) = scp_server().await;
    let junos = built_in_platform("junos").unwrap().definition();
    let files = connected(server.target())
        .await
        .start_files(Some(&junos))
        .await
        .unwrap();
    let Files::Scp(scp) = files else {
        panic!("SFTP started")
    };
    assert!(scp.hint().unwrap().contains(JUNOS_HINT));

    // Another platform falling back gets none.
    let ios = built_in_platform("ios").unwrap().definition();
    let files = connected(server.target())
        .await
        .start_files(Some(&ios))
        .await
        .unwrap();
    let Files::Scp(scp) = files else {
        panic!("SFTP started")
    };
    assert_eq!(scp.hint(), None);
}

/// A source that signals once it has given `after` bytes.
struct Signalling {
    bytes: Vec<u8>,
    given: usize,
    after: usize,
    reached: Arc<Notify>,
}

impl AsyncRead for Signalling {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        let take = buf.remaining().min(this.bytes.len() - this.given);
        buf.put_slice(&this.bytes[this.given..this.given + take]);
        this.given += take;
        if this.given >= this.after {
            this.reached.notify_one();
        }
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn cancelling_a_send_closes_the_channel_and_says_a_partial_file_may_remain() {
    let (server, _folder) = scp_server().await;
    let connection = connected(server.target()).await;
    let size = 8 * 1024 * 1024;
    let reached = Arc::new(Notify::new());
    let source = Signalling {
        bytes: content(size),
        given: 0,
        after: 1024 * 1024,
        reached: reached.clone(),
    };

    let cancelled = connection
        .scp_send("/big.bin", size as u64, source, reached.notified())
        .await
        .unwrap_err();
    assert_eq!(cancelled, ScpError::Cancelled);
    assert!(
        cancelled.to_string().contains("partial file may remain"),
        "{cancelled}"
    );
    until("SCP transfers on the server", 0, || server.scp_running()).await;
    // The connection's channel is free again.
    send(&connection, "/after.txt", b"after").await.unwrap();
}

#[tokio::test]
async fn listing_stat_rename_and_delete_are_refused_on_scp_as_transfer_only() {
    let (server, _folder) = scp_server().await;
    let files = connected(server.target())
        .await
        .start_files(None)
        .await
        .unwrap();

    let refusals = [
        files.list("/").await.map(|_| ()).unwrap_err(),
        files.stat("/a.txt").await.map(|_| ()).unwrap_err(),
        files.rename("/a.txt", "/b.txt").await.unwrap_err(),
        files.delete_file("/a.txt").await.unwrap_err(),
        files.delete_folder("/a").await.unwrap_err(),
        files.make_folder("/a").await.unwrap_err(),
    ];
    for refused in refusals {
        assert!(
            matches!(refused, FileError::TransferOnly { .. }),
            "{refused:?}"
        );
        assert!(refused.to_string().contains("transfer only"), "{refused}");
    }
}
