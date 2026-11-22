//! The transfer engine against the test SSH server: files up and down over
//! SFTP and SCP byte for byte, the waiting list per host, progress, cancel
//! and partial files, hashes, the size check, "is it there", the stall
//! rule, a lost connection, and SCP's connection per transfer.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use enodix_core::files::{
    Done, FileConnection, Files, Hashes, Link, Lost, Outcome, Partial, Presence, SizeCheck, Status,
    Transfer, TransferError, TransferWatch, Transfers,
};
use enodix_core::session::SshTarget;
use sha2::Digest;
use support::scp::ScpQuirk;
use support::{known_hosts, TestSshServer, WAIT};
use tempfile::TempDir;

const HOST: &str = "router1";
const MIB: usize = 1024 * 1024;

/// A server serving a new temporary folder over SFTP.
async fn sftp_server() -> (TestSshServer, TempDir) {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    (server, folder)
}

/// A server serving a new temporary folder over SCP only.
async fn scp_server() -> (TestSshServer, TempDir) {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    server.serve_scp(folder.path());
    (server, folder)
}

async fn files_at(target: SshTarget) -> Files {
    FileConnection::open_unattended(target, known_hosts())
        .await
        .unwrap()
        .start_files(None)
        .await
        .unwrap()
}

/// An engine with [`HOST`] linked to `target`'s files.
async fn engine_at(target: SshTarget) -> Transfers {
    let transfers = Transfers::new();
    transfers.connect(HOST, Link::from(files_at(target).await));
    transfers
}

async fn engine(server: &TestSshServer) -> Transfers {
    engine_at(server.target()).await
}

/// `server`'s target with a short connect timeout, so that the stall limit
/// (three connect timeouts) is short.
fn impatient(server: &TestSshServer) -> SshTarget {
    let mut target = server.target();
    target.connect_timeout = Duration::from_secs(1);
    target
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

/// A local file holding `bytes`.
fn local_file(folder: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = folder.path().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

async fn outcome(watch: TransferWatch) -> Outcome {
    tokio::time::timeout(WAIT, watch.outcome())
        .await
        .expect("the transfer did not end")
}

fn done(outcome: Outcome) -> Done {
    match outcome {
        Outcome::Done(done) => done,
        other => panic!("not done: {other:?}"),
    }
}

/// Waits until the transfer has moved at least one byte.
async fn moving(watch: &mut TransferWatch) {
    let waited = tokio::time::timeout(WAIT, async {
        loop {
            if let Status::Running { done, .. } = watch.status() {
                if done > 0 {
                    return;
                }
            }
            if watch.changed().await.is_none() {
                panic!("ended before moving: {:?}", watch.status());
            }
        }
    })
    .await;
    assert!(waited.is_ok(), "never moved: {:?}", watch.status());
}

async fn until(what: &str, check: impl Fn() -> bool) {
    let waited = tokio::time::timeout(WAIT, async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what}");
}

fn hashes_of(bytes: &[u8]) -> Hashes {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect();
    Hashes {
        md5: hex(&md5::compute(bytes).0),
        sha256: hex(&sha2::Sha256::digest(bytes)),
    }
}

fn exists(folder: &TempDir, name: &str) -> bool {
    folder.path().join(name).exists()
}

#[tokio::test]
async fn a_file_goes_up_and_comes_back_byte_for_byte_over_sftp() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let bytes = content(6 * MIB + 17);
    let path = local_file(&local, "image.bin", &bytes);
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::upload(&path, "/image.bin"));
    let sent = done(outcome(watch).await);
    assert_eq!(sent.bytes, bytes.len() as u64);
    assert_eq!(sent.size, SizeCheck::Checked);
    assert_eq!(sent.hashes, Some(hashes_of(&bytes)));
    assert!(fs::read(remote.path().join("image.bin")).unwrap() == bytes);

    let back = local.path().join("back.bin");
    let (_, watch) = transfers.add(HOST, Transfer::download("/image.bin", &back));
    let fetched = done(outcome(watch).await);
    assert_eq!(fetched.bytes, bytes.len() as u64);
    assert_eq!(fetched.size, SizeCheck::Checked);
    assert_eq!(fetched.hashes, None);
    assert!(fs::read(&back).unwrap() == bytes);
}

#[tokio::test]
async fn a_file_goes_up_and_comes_back_byte_for_byte_over_scp() {
    let (server, remote) = scp_server().await;
    let local = TempDir::new().unwrap();
    let bytes = content(6 * MIB + 17);
    let path = local_file(&local, "image.bin", &bytes);
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::upload(&path, "flash:image.bin"));
    let sent = done(outcome(watch).await);
    assert_eq!(sent.bytes, bytes.len() as u64);
    assert_eq!(sent.size, SizeCheck::NotChecked, "SCP cannot check");
    assert_eq!(
        sent.size.to_string(),
        "the size the device wrote could not be checked"
    );
    assert_eq!(sent.hashes, Some(hashes_of(&bytes)));
    assert!(fs::read(remote.path().join("image.bin")).unwrap() == bytes);

    let back = local.path().join("back.bin");
    let (_, watch) = transfers.add(HOST, Transfer::download("flash:image.bin", &back));
    let fetched = done(outcome(watch).await);
    assert_eq!(fetched.size, SizeCheck::Checked, "against the header");
    assert!(fs::read(&back).unwrap() == bytes);
}

#[tokio::test]
async fn an_upload_gives_the_local_files_md5_and_sha256() {
    let (server, _remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "abc.txt", b"abc");
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::upload(&path, "/abc.txt"));
    let hashes = done(outcome(watch).await).hashes.unwrap();
    assert_eq!(hashes.md5, "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        hashes.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[tokio::test]
async fn transfers_for_one_host_run_one_after_the_other_in_order() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let bytes = content(8 * MIB);
    let path = local_file(&local, "image.bin", &bytes);
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (first, mut first_watch) = transfers.add(HOST, Transfer::upload(&path, "/a.bin"));
    let (second, second_watch) = transfers.add(HOST, Transfer::upload(&path, "/b.bin"));
    let (third, mut third_watch) = transfers.add(HOST, Transfer::upload(&path, "/c.bin"));
    moving(&mut first_watch).await;
    assert_eq!(transfers.running(HOST), Some(first));
    assert_eq!(transfers.waiting(HOST), vec![second, third]);
    assert_eq!(second_watch.status(), Status::Waiting);

    // A waiting one is removed.
    assert!(transfers.cancel(second));
    assert_eq!(transfers.waiting(HOST), vec![third]);
    assert_eq!(
        outcome(second_watch).await,
        Outcome::Cancelled {
            partial: Partial::Nothing
        }
    );

    server.sftp_quirks().release();
    // The third starts only once the first has ended.
    while third_watch.status() == Status::Waiting {
        third_watch.changed().await.unwrap();
    }
    assert!(matches!(
        first_watch.status(),
        Status::Finished(Outcome::Done(_))
    ));
    done(outcome(first_watch).await);
    done(outcome(third_watch).await);
    assert!(exists(&remote, "a.bin") && exists(&remote, "c.bin"));
    assert!(!exists(&remote, "b.bin"), "the removed one never ran");
    assert_eq!(transfers.running(HOST), None);
}

#[tokio::test]
async fn transfers_for_two_hosts_run_at_the_same_time() {
    let (first, _first_folder) = sftp_server().await;
    let (second, _second_folder) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(8 * MIB));
    let transfers = Transfers::new();
    transfers.connect("r1", Link::from(files_at(first.target()).await));
    transfers.connect("r2", Link::from(files_at(second.target()).await));
    first.sftp_quirks().hold_from(MIB as u64);
    second.sftp_quirks().hold_from(MIB as u64);

    let (_, mut one) = transfers.add("r1", Transfer::upload(&path, "/image.bin"));
    let (_, mut two) = transfers.add("r2", Transfer::upload(&path, "/image.bin"));
    moving(&mut one).await;
    moving(&mut two).await;
    assert!(matches!(one.status(), Status::Running { .. }));
    assert!(matches!(two.status(), Status::Running { .. }));

    first.sftp_quirks().release();
    second.sftp_quirks().release();
    done(outcome(one).await);
    done(outcome(two).await);
}

#[tokio::test]
async fn progress_arrives_while_a_transfer_runs_and_ends_with_its_outcome() {
    let (server, _remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let size = 8 * MIB as u64;
    let path = local_file(&local, "image.bin", &content(size as usize));
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (_, mut watch) = transfers.add(HOST, Transfer::upload(&path, "/image.bin"));
    moving(&mut watch).await;
    let Status::Running {
        done: so_far,
        total,
    } = watch.status()
    else {
        unreachable!()
    };
    assert_eq!(total, Some(size));
    assert!(so_far < size, "held part way: {so_far}");

    server.sftp_quirks().release();
    let mut last = watch.status();
    let mut before = so_far;
    while let Some(status) = tokio::time::timeout(WAIT, watch.changed()).await.unwrap() {
        if let Status::Running { done, .. } = status {
            assert!(done >= before, "progress went back");
            before = done;
        }
        last = status;
    }
    match last {
        Status::Finished(Outcome::Done(done)) => assert_eq!(done.bytes, size),
        other => panic!("ended with {other:?}"),
    }
}

#[tokio::test]
async fn a_caller_that_never_reads_the_progress_does_not_hold_transfers_up() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let bytes = content(3 * MIB);
    let path = local_file(&local, "image.bin", &bytes);
    let transfers = engine(&server).await;

    let watches: Vec<TransferWatch> = ["/a.bin", "/b.bin", "/c.bin"]
        .into_iter()
        .map(|remote| transfers.add(HOST, Transfer::upload(&path, remote)).1)
        .collect();
    until("the transfers ended", || {
        transfers.running(HOST).is_none() && transfers.waiting(HOST).is_empty()
    })
    .await;
    for watch in watches {
        assert!(matches!(watch.status(), Status::Finished(Outcome::Done(_))));
    }
    for name in ["a.bin", "b.bin", "c.bin"] {
        assert!(fs::read(remote.path().join(name)).unwrap() == bytes);
    }
}

#[tokio::test]
async fn cancelling_the_running_sftp_upload_removes_its_remote_file() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(8 * MIB));
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (id, mut watch) = transfers.add(HOST, Transfer::upload(&path, "/image.bin"));
    moving(&mut watch).await;
    assert!(exists(&remote, "image.bin"), "written to its final name");
    assert!(transfers.cancel(id));
    server.sftp_quirks().release();

    let partial = Partial::RemovedFromDevice;
    assert_eq!(outcome(watch).await, Outcome::Cancelled { partial });
    assert!(!exists(&remote, "image.bin"));
}

#[tokio::test]
async fn when_the_device_refuses_the_remove_the_outcome_says_the_file_remains() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(8 * MIB));
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);
    server.sftp_quirks().refuse_remove();

    let (id, mut watch) = transfers.add(HOST, Transfer::upload(&path, "/image.bin"));
    moving(&mut watch).await;
    transfers.cancel(id);
    server.sftp_quirks().release();

    let Outcome::Cancelled { partial } = outcome(watch).await else {
        panic!("not cancelled")
    };
    assert!(
        matches!(partial, Partial::RemainsOnDevice { .. }),
        "{partial:?}"
    );
    assert_eq!(
        partial.to_string(),
        "a partial file remains on the device; it could not be removed \
         (permission denied: /image.bin)"
    );
    assert!(exists(&remote, "image.bin"));
}

#[tokio::test]
async fn a_cancelled_scp_upload_says_a_partial_file_may_remain() {
    let (server, _remote) = scp_server().await;
    server.scp_quirk(ScpQuirk::StallMidway);
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(4 * MIB));
    let transfers = engine(&server).await;

    let (id, mut watch) = transfers.add(HOST, Transfer::upload(&path, "flash:image.bin"));
    moving(&mut watch).await;
    transfers.cancel(id);

    let partial = outcome(watch).await;
    assert_eq!(
        partial,
        Outcome::Cancelled {
            partial: Partial::MayRemainOnDevice
        }
    );
    let Outcome::Cancelled { partial } = partial else {
        unreachable!()
    };
    assert_eq!(
        partial.to_string(),
        "a partial file may remain on the device (SCP cannot remove it)"
    );
}

#[tokio::test]
async fn a_cancelled_sftp_download_leaves_no_local_file() {
    let (server, remote) = sftp_server().await;
    fs::write(remote.path().join("image.bin"), content(8 * MIB)).unwrap();
    let local = TempDir::new().unwrap();
    let target = local.path().join("image.bin");
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (id, mut watch) = transfers.add(HOST, Transfer::download("/image.bin", &target));
    moving(&mut watch).await;
    assert!(target.exists());
    transfers.cancel(id);

    let partial = Partial::RemovedLocally;
    assert_eq!(outcome(watch).await, Outcome::Cancelled { partial });
    assert!(!target.exists());
    server.sftp_quirks().release();
}

#[tokio::test]
async fn a_cancelled_scp_download_leaves_no_local_file() {
    let (server, remote) = scp_server().await;
    server.scp_quirk(ScpQuirk::StallMidway);
    fs::write(remote.path().join("image.bin"), content(4 * MIB)).unwrap();
    let local = TempDir::new().unwrap();
    let target = local.path().join("image.bin");
    let transfers = engine(&server).await;

    let (id, mut watch) = transfers.add(HOST, Transfer::download("flash:image.bin", &target));
    moving(&mut watch).await;
    let Status::Running { total, .. } = watch.status() else {
        unreachable!()
    };
    assert_eq!(total, Some(4 * MIB as u64), "the header's size");
    transfers.cancel(id);

    let partial = Partial::RemovedLocally;
    assert_eq!(outcome(watch).await, Outcome::Cancelled { partial });
    assert!(!target.exists());
}

#[tokio::test]
async fn a_remote_file_shorter_than_the_local_one_fails_the_sftp_upload() {
    let (server, remote) = sftp_server().await;
    server.sftp_quirks().keep_at_most(100_000);
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(300_000));
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::upload(&path, "/image.bin"));
    let Outcome::Failed { error, partial } = outcome(watch).await else {
        panic!("not failed")
    };
    assert_eq!(
        error,
        TransferError::SizeOnDevice {
            sent: 300_000,
            found: 100_000
        }
    );
    assert_eq!(
        error.to_string(),
        "the device holds 100000 bytes of the 300000 sent"
    );
    assert_eq!(partial, Partial::RemovedFromDevice);
    assert!(!exists(&remote, "image.bin"));
}

#[tokio::test]
async fn is_it_there_answers_with_the_size_or_absent_over_sftp_and_cannot_tell_over_scp() {
    let (server, remote) = sftp_server().await;
    fs::write(remote.path().join("image.bin"), b"twelve bytes").unwrap();
    let files = files_at(server.target()).await;
    assert_eq!(
        files.presence("/image.bin").await.unwrap(),
        Presence::Exists { size: Some(12) }
    );
    assert_eq!(
        files.presence("/missing.bin").await.unwrap(),
        Presence::Absent
    );
    let link = Link::from(files);
    assert_eq!(
        link.presence("/image.bin").await.unwrap(),
        Presence::Exists { size: Some(12) }
    );

    let (scp, _scp_folder) = scp_server().await;
    let files = files_at(scp.target()).await;
    assert_eq!(
        files.presence("flash:image.bin").await.unwrap(),
        Presence::CannotTell
    );
    assert_eq!(
        Link::from(files).presence("flash:x").await.unwrap(),
        Presence::CannotTell
    );
}

/// A transfer against a server that stops answering: it fails as stalled
/// after three connect timeouts (one second each here).
async fn stalls(transfers: &Transfers, transfer: Transfer) -> (TransferError, Partial) {
    let started = Instant::now();
    let (_, watch) = transfers.add(HOST, transfer);
    let Outcome::Failed { error, partial } = outcome(watch).await else {
        panic!("not failed")
    };
    let limit = Duration::from_secs(3);
    assert_eq!(error, TransferError::Stalled { after: limit });
    assert!(started.elapsed() >= limit, "after {:?}", started.elapsed());
    assert_eq!(
        error.to_string(),
        "no byte moved for 3 s: the device stopped answering"
    );
    (error, partial)
}

#[tokio::test]
async fn an_sftp_upload_that_stops_moving_fails_after_the_stall_time() {
    let (server, _remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(8 * MIB));
    let transfers = engine_at(impatient(&server)).await;
    server.sftp_quirks().hold_from(MIB as u64);

    stalls(&transfers, Transfer::upload(&path, "/image.bin")).await;
}

#[tokio::test]
async fn an_sftp_download_that_stops_moving_fails_after_the_stall_time() {
    let (server, remote) = sftp_server().await;
    fs::write(remote.path().join("image.bin"), content(8 * MIB)).unwrap();
    let local = TempDir::new().unwrap();
    let target = local.path().join("image.bin");
    let transfers = engine_at(impatient(&server)).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (_, partial) = stalls(&transfers, Transfer::download("/image.bin", &target)).await;
    assert_eq!(partial, Partial::RemovedLocally);
    assert!(!target.exists());
}

#[tokio::test]
async fn an_scp_fetch_that_stops_moving_fails_after_the_stall_time() {
    let (server, remote) = scp_server().await;
    server.scp_quirk(ScpQuirk::StallMidway);
    fs::write(remote.path().join("image.bin"), content(4 * MIB)).unwrap();
    let local = TempDir::new().unwrap();
    let target = local.path().join("image.bin");
    let transfers = engine_at(impatient(&server)).await;

    let (_, partial) = stalls(&transfers, Transfer::download("flash:image.bin", &target)).await;
    assert_eq!(partial, Partial::RemovedLocally);
}

#[tokio::test]
async fn a_cut_connection_fails_the_running_transfer_at_once_and_the_rest_wait() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(8 * MIB));
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (_, mut first) = transfers.add(HOST, Transfer::upload(&path, "/a.bin"));
    let (second, second_watch) = transfers.add(HOST, Transfer::upload(&path, "/b.bin"));
    moving(&mut first).await;
    let cut = Instant::now();
    server.sever_connections();

    let Outcome::Failed { error, partial } = outcome(first).await else {
        panic!("not failed")
    };
    assert!(
        cut.elapsed() < Duration::from_secs(5),
        "after {:?}",
        cut.elapsed()
    );
    assert!(matches!(error, TransferError::Lost(_)), "{error:?}");
    assert!(
        error.to_string().starts_with("the connection was lost: "),
        "{error}"
    );
    assert!(
        matches!(partial, Partial::RemainsOnDevice { .. }),
        "{partial:?}"
    );

    // The second waits for a new link.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(second_watch.status(), Status::Waiting);
    assert_eq!(transfers.waiting(HOST), vec![second]);
    assert!(!transfers.is_connected(HOST));

    server.sftp_quirks().release();
    transfers.connect(HOST, Link::from(files_at(server.target()).await));
    assert!(transfers.is_connected(HOST));
    done(outcome(second_watch).await);
    assert!(exists(&remote, "b.bin"));
}

#[tokio::test]
async fn over_scp_each_transfer_makes_its_own_connection() {
    let (server, remote) = scp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(MIB));
    let transfers = engine(&server).await;
    let before = server.logins();

    let (_, first) = transfers.add(HOST, Transfer::upload(&path, "flash:a.bin"));
    let (_, second) = transfers.add(HOST, Transfer::upload(&path, "flash:b.bin"));
    done(outcome(first).await);
    done(outcome(second).await);
    assert_eq!(server.logins() - before, 2);
    assert!(exists(&remote, "a.bin") && exists(&remote, "b.bin"));
    until("the connections closed", || server.connections_alive() == 0).await;
}

#[tokio::test]
async fn over_sftp_queued_transfers_share_the_open_connection() {
    let (server, _remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(MIB));
    let transfers = engine(&server).await;
    let before = server.logins();

    let (_, first) = transfers.add(HOST, Transfer::upload(&path, "/a.bin"));
    let (_, second) = transfers.add(HOST, Transfer::upload(&path, "/b.bin"));
    done(outcome(first).await);
    done(outcome(second).await);
    assert_eq!(server.logins(), before, "no new login");
    assert_eq!(server.connections_alive(), 1);
}

#[tokio::test]
async fn transfers_wait_until_the_host_has_a_link() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", b"hello");
    let transfers = Transfers::new();

    let (_, watch) = transfers.add(HOST, Transfer::upload(&path, "/hello.txt"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(watch.status(), Status::Waiting);
    assert!(!transfers.is_connected(HOST));

    transfers.connect(HOST, Link::from(files_at(server.target()).await));
    done(outcome(watch).await);
    assert_eq!(fs::read(remote.path().join("hello.txt")).unwrap(), b"hello");
}

#[tokio::test]
async fn a_missing_local_file_fails_the_upload_and_leaves_nothing() {
    let (server, remote) = sftp_server().await;
    let missing: &Path = Path::new("no such folder/image.bin");
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::upload(missing, "/image.bin"));
    let Outcome::Failed { error, partial } = outcome(watch).await else {
        panic!("not failed")
    };
    assert!(matches!(error, TransferError::Local(_)), "{error:?}");
    assert!(error.to_string().starts_with("could not open "), "{error}");
    assert_eq!(partial, Partial::Nothing);
    assert!(!exists(&remote, "image.bin"));
}

#[tokio::test]
async fn closing_a_host_cancels_its_running_and_waiting_transfers() {
    let (server, remote) = sftp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(8 * MIB));
    let transfers = engine(&server).await;
    server.sftp_quirks().hold_from(MIB as u64);

    let (_, mut first) = transfers.add(HOST, Transfer::upload(&path, "/a.bin"));
    let (_, second) = transfers.add(HOST, Transfer::upload(&path, "/b.bin"));
    moving(&mut first).await;
    transfers.close(HOST);
    server.sftp_quirks().release();

    let removed = Partial::RemovedFromDevice;
    assert_eq!(
        outcome(first).await,
        Outcome::Cancelled { partial: removed }
    );
    let nothing = Partial::Nothing;
    assert_eq!(
        outcome(second).await,
        Outcome::Cancelled { partial: nothing }
    );
    assert!(!transfers.is_connected(HOST));
    assert!(!exists(&remote, "a.bin") && !exists(&remote, "b.bin"));
}

#[tokio::test]
async fn a_refused_scp_fetch_leaves_an_existing_local_file_as_it_was() {
    let (server, _remote) = scp_server().await;
    let local = TempDir::new().unwrap();
    let kept = local_file(&local, "image.bin", b"the user's own file");
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::download("flash:missing.bin", &kept));
    let Outcome::Failed { error, partial } = outcome(watch).await else {
        panic!("not failed")
    };
    assert!(matches!(error, TransferError::Scp(_)), "{error:?}");
    assert_eq!(partial, Partial::Nothing);
    assert_eq!(fs::read(&kept).unwrap(), b"the user's own file");
}

#[tokio::test]
async fn an_empty_file_comes_down_over_scp() {
    let (server, remote) = scp_server().await;
    fs::write(remote.path().join("empty.txt"), b"").unwrap();
    let local = TempDir::new().unwrap();
    let target = local.path().join("empty.txt");
    let transfers = engine(&server).await;

    let (_, watch) = transfers.add(HOST, Transfer::download("flash:empty.txt", &target));
    assert_eq!(done(outcome(watch).await).bytes, 0);
    assert_eq!(fs::read(&target).unwrap(), b"");
}

#[tokio::test]
async fn an_scp_upload_that_stops_moving_fails_after_the_stall_time() {
    let (server, _remote) = scp_server().await;
    server.scp_quirk(ScpQuirk::StallMidway);
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "image.bin", &content(4 * MIB));
    let transfers = engine_at(impatient(&server)).await;

    // The test server's SCP, reading nothing, blocks its whole connection
    // (a channel nobody reads), so its keepalives stop too: the connection
    // is found not answering at the same moment as the stall.
    let started = Instant::now();
    let (_, watch) = transfers.add(HOST, Transfer::upload(&path, "flash:image.bin"));
    let Outcome::Failed { error, partial } = outcome(watch).await else {
        panic!("not failed")
    };
    assert!(
        matches!(
            error,
            TransferError::Stalled { .. } | TransferError::Lost(Lost::NotAnswering)
        ),
        "{error:?}"
    );
    assert!(started.elapsed() >= Duration::from_secs(3));
    assert_eq!(partial, Partial::MayRemainOnDevice);
}

/// A caller's own way of connecting for SCP whose connection fails.
struct Refusing;

impl enodix_core::files::Connect for Refusing {
    fn connect(&self) -> enodix_core::files::Connecting<'_> {
        Box::pin(async { Err("the login was cancelled".to_owned().into()) })
    }
}

/// A caller's own way of connecting for SCP whose user says no to the
/// first question, then connects unattended.
struct DecliningOnce {
    declined: std::sync::atomic::AtomicBool,
    target: SshTarget,
}

impl enodix_core::files::Connect for DecliningOnce {
    fn connect(&self) -> enodix_core::files::Connecting<'_> {
        Box::pin(async move {
            if !self
                .declined
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(enodix_core::files::ConnectFailure::Declined(
                    "the login was cancelled".into(),
                ));
            }
            enodix_core::files::FileConnection::open_unattended(
                self.target.clone(),
                support::known_hosts(),
            )
            .await
            .map_err(|error| error.to_string().into())
        })
    }
}

#[tokio::test]
async fn a_declined_scp_connection_fails_that_transfer_alone_and_the_next_one_connects() {
    let (server, remote) = scp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "hello.txt", b"hello");
    let transfers = Transfers::new();
    transfers.connect(
        HOST,
        Link::scp(DecliningOnce {
            declined: Default::default(),
            target: server.target(),
        }),
    );

    let (_, first) = transfers.add(HOST, Transfer::upload(&path, "flash:a.txt"));
    let (_, second) = transfers.add(HOST, Transfer::upload(&path, "flash:b.txt"));
    let Outcome::Failed { error, partial } = outcome(first).await else {
        panic!("not failed")
    };
    assert_eq!(
        error,
        TransferError::Declined("the login was cancelled".into())
    );
    assert_eq!(error.to_string(), "the login was cancelled");
    assert_eq!(partial, Partial::Nothing);
    done(outcome(second).await);
    assert!(transfers.is_connected(HOST));
    assert!(exists(&remote, "b.txt"));
    assert!(!exists(&remote, "a.txt"));
}

#[tokio::test]
async fn a_failed_scp_connection_fails_the_transfer_and_the_rest_wait() {
    let (server, remote) = scp_server().await;
    let local = TempDir::new().unwrap();
    let path = local_file(&local, "hello.txt", b"hello");
    let transfers = Transfers::new();
    transfers.connect(HOST, Link::scp(Refusing));

    let (_, first) = transfers.add(HOST, Transfer::upload(&path, "flash:a.txt"));
    let (second, second_watch) = transfers.add(HOST, Transfer::upload(&path, "flash:b.txt"));
    let Outcome::Failed { error, partial } = outcome(first).await else {
        panic!("not failed")
    };
    assert_eq!(
        error.to_string(),
        "could not connect for the transfer: the login was cancelled"
    );
    assert_eq!(partial, Partial::Nothing);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(transfers.waiting(HOST), vec![second]);
    assert!(!transfers.is_connected(HOST));

    transfers.connect(HOST, Link::from(files_at(server.target()).await));
    done(outcome(second_watch).await);
    assert!(exists(&remote, "b.txt"));
}
