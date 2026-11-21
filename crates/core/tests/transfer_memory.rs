//! The transfer engine never holds a file in memory: a file of tens of
//! megabytes goes up and comes back over SFTP and over SCP while the
//! process's resident memory stays far below the file's size.
//!
//! A file of its own, so that no other test shares the process (and its
//! memory) while this one measures.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use netterm_core::files::{FileConnection, Link, Outcome, Transfer, Transfers};
use support::memory::{resident, Peak, MIB};
use support::{known_hosts, TestSshServer};
use tempfile::TempDir;

/// The file's size.
const SIZE: u64 = 40 * MIB;

/// How much the process's memory may grow while files move: well below
/// [`SIZE`] (the SSH window, the SFTP requests in flight, buffers), so a
/// file held whole would not pass. On Linux the growth measured 16 to 18
/// MiB (glibc's allocator keeps an arena per thread of the runtime), so
/// 16 MiB failed there (#114).
const GROWTH: u64 = 24 * MIB;

/// Writes a file of [`SIZE`] bytes a piece at a time.
fn write_large(path: &Path) {
    let mut file = fs::File::create(path).unwrap();
    let mut state = 0x2545_f491_u32;
    let mut piece = vec![0u8; MIB as usize];
    for _ in 0..SIZE / MIB {
        for byte in &mut piece {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        file.write_all(&piece).unwrap();
    }
}

/// Whether two files hold the same bytes, compared a piece at a time.
fn same(one: &Path, two: &Path) -> bool {
    let (mut one, mut two) = (fs::File::open(one).unwrap(), fs::File::open(two).unwrap());
    let (mut a, mut b) = (vec![0u8; MIB as usize], vec![0u8; MIB as usize]);
    loop {
        let n = one.read(&mut a).unwrap();
        let mut m = 0;
        while m < n {
            match two.read(&mut b[m..n]).unwrap() {
                0 => return false,
                read => m += read,
            }
        }
        if n == 0 {
            return two.read(&mut b).unwrap() == 0;
        }
        if a[..n] != b[..n] {
            return false;
        }
    }
}

/// Sends `local` to the server and fetches it back, through an engine
/// linked to `server`'s files; `remote` names the file there.
async fn up_and_down(server: &TestSshServer, local: &Path, remote: &str, back: &Path) {
    let files = FileConnection::open_unattended(server.target(), known_hosts())
        .await
        .unwrap()
        .start_files(None)
        .await
        .unwrap();
    let transfers = Transfers::new();
    transfers.connect("router1", Link::from(files));
    let (_, watch) = transfers.add("router1", Transfer::upload(local, remote));
    let sent = watch.outcome().await;
    assert!(matches!(sent, Outcome::Done(_)), "{sent:?}");
    let (_, watch) = transfers.add("router1", Transfer::download(remote, back));
    let fetched = watch.outcome().await;
    assert!(matches!(fetched, Outcome::Done(_)), "{fetched:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_of_tens_of_megabytes_move_without_memory_growing_with_their_size() {
    let local = TempDir::new().unwrap();
    let image = local.path().join("image.bin");
    write_large(&image);
    let sftp_folder = TempDir::new().unwrap();
    let sftp = TestSshServer::start().await;
    sftp.serve_sftp(sftp_folder.path());
    let scp_folder = TempDir::new().unwrap();
    let scp = TestSshServer::start().await;
    scp.serve_scp(scp_folder.path());

    let baseline = resident();
    let peak = Peak::start();
    let sftp_back = local.path().join("sftp.bin");
    up_and_down(&sftp, &image, "/image.bin", &sftp_back).await;
    let scp_back = local.path().join("scp.bin");
    up_and_down(&scp, &image, "flash:image.bin", &scp_back).await;
    let highest = peak.stop();

    for copy in [
        sftp_folder.path().join("image.bin"),
        sftp_back,
        scp_folder.path().join("image.bin"),
        scp_back,
    ] {
        assert!(same(&image, &copy), "{} differs", copy.display());
    }
    if let Some(baseline) = baseline {
        let growth = highest.saturating_sub(baseline);
        assert!(
            growth < GROWTH,
            "memory grew by {} MiB moving {} MiB files",
            growth / MIB,
            SIZE / MIB
        );
        eprintln!(
            "memory grew by {:.1} MiB moving {} MiB files",
            growth as f64 / MIB as f64,
            SIZE / MIB
        );
    }
}
