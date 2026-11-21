//! SFTP on a file connection (`netterm_core::files::Sftp`) against the test
//! SSH server's SFTP side, which serves a temporary folder: starting it (and
//! telling a device without SFTP from a failed connection), the operations
//! the Files tab needs, the read and write streams, and failures in the
//! app's own words.

mod support;

use std::fs;

use netterm_core::files::{EntryKind, FileConnection, FileError, NoSftp, Sftp, StartError};
use support::sftp::{SFTP_HOME, SFTP_LOCKED};
use support::{known_hosts, TestSshServer, WAIT};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A server serving a new temporary folder, a file connection to it, and
/// SFTP on that connection.
struct Served {
    server: TestSshServer,
    folder: TempDir,
    connection: FileConnection,
    sftp: Sftp,
}

impl Served {
    async fn start() -> Self {
        let server = TestSshServer::start().await;
        let folder = TempDir::new().unwrap();
        server.serve_sftp(folder.path());
        let connection = FileConnection::open_unattended(server.target(), known_hosts())
            .await
            .unwrap();
        let sftp = connection.start_sftp().await.unwrap();
        Self {
            server,
            folder,
            connection,
            sftp,
        }
    }

    fn local(&self, path: &str) -> std::path::PathBuf {
        self.folder.path().join(path.trim_start_matches('/'))
    }
}

/// No message shows SFTP's bare codes.
fn assert_own_words(error: &FileError) {
    let message = error.to_string();
    assert!(!message.contains("Failure"), "{message}");
    assert!(!message.contains("No such file:"), "{message}");
    assert!(!message.contains("sender dropped"), "{message}");
}

#[tokio::test]
async fn the_home_folder_is_where_the_login_lands_as_an_absolute_path() {
    let served = Served::start().await;
    assert_eq!(served.sftp.home().await.unwrap(), SFTP_HOME);
}

#[tokio::test]
async fn a_listing_has_files_folders_and_hidden_files_and_no_dot_entries() {
    let served = Served::start().await;
    fs::write(served.local("config.txt"), b"hostname r1\n").unwrap();
    fs::write(served.local(".hidden"), b"x").unwrap();
    fs::create_dir(served.local("logs")).unwrap();

    let mut entries = served.sftp.list("/").await.unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, [".hidden", "config.txt", "logs"]);

    let file = &entries[1];
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.size, Some(12));
    assert!(file.modified.is_some());
    let permissions = file.permissions.as_deref().unwrap();
    assert_eq!(permissions.len(), 9, "{permissions}");
    assert!(permissions.starts_with("rw"), "{permissions}");
    assert_eq!(entries[2].kind, EntryKind::Folder);
}

#[cfg(unix)]
#[tokio::test]
async fn a_listing_shows_a_link_as_a_link() {
    let served = Served::start().await;
    fs::write(served.local("target.txt"), b"x").unwrap();
    std::os::unix::fs::symlink(served.local("target.txt"), served.local("link")).unwrap();
    let entries = served.sftp.list("/").await.unwrap();
    let link = entries.iter().find(|e| e.name == "link").unwrap();
    assert_eq!(link.kind, EntryKind::Link);
}

#[tokio::test]
async fn stat_gives_one_entry() {
    let served = Served::start().await;
    fs::create_dir(served.local("logs")).unwrap();
    fs::write(served.local("logs/today.log"), b"12345").unwrap();

    let file = served.sftp.stat("/logs/today.log").await.unwrap();
    assert_eq!(file.name, "today.log");
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.size, Some(5));
    let folder = served.sftp.stat("/logs").await.unwrap();
    assert_eq!(folder.kind, EntryKind::Folder);

    let missing = served.sftp.stat("/nope.txt").await.unwrap_err();
    assert_eq!(
        missing,
        FileError::NotFound {
            path: "/nope.txt".into()
        }
    );
    assert_eq!(missing.to_string(), "no such file or folder: /nope.txt");
}

#[tokio::test]
async fn make_rename_and_delete() {
    let served = Served::start().await;
    let sftp = &served.sftp;

    sftp.make_folder("/backups").await.unwrap();
    assert!(served.local("backups").is_dir());

    fs::write(served.local("backups/r1.cfg"), b"config").unwrap();
    sftp.rename("/backups/r1.cfg", "/backups/r1-old.cfg")
        .await
        .unwrap();
    assert!(!served.local("backups/r1.cfg").exists());
    assert_eq!(
        fs::read(served.local("backups/r1-old.cfg")).unwrap(),
        b"config"
    );

    sftp.delete_file("/backups/r1-old.cfg").await.unwrap();
    assert!(!served.local("backups/r1-old.cfg").exists());
    sftp.delete_folder("/backups").await.unwrap();
    assert!(!served.local("backups").exists());
}

#[tokio::test]
async fn renaming_onto_an_existing_name_is_reported_and_changes_nothing() {
    let served = Served::start().await;
    fs::write(served.local("a.cfg"), b"a").unwrap();
    fs::write(served.local("b.cfg"), b"b").unwrap();

    let error = served.sftp.rename("/a.cfg", "/b.cfg").await.unwrap_err();
    assert_eq!(
        error,
        FileError::AlreadyExists {
            path: "/b.cfg".into()
        }
    );
    assert_eq!(error.to_string(), "/b.cfg exists already");
    assert_own_words(&error);
    assert_eq!(fs::read(served.local("a.cfg")).unwrap(), b"a");
    assert_eq!(fs::read(served.local("b.cfg")).unwrap(), b"b");
}

#[tokio::test]
async fn making_a_folder_that_exists_is_reported() {
    let served = Served::start().await;
    fs::create_dir(served.local("logs")).unwrap();
    let error = served.sftp.make_folder("/logs").await.unwrap_err();
    assert_eq!(
        error,
        FileError::AlreadyExists {
            path: "/logs".into()
        }
    );
}

#[tokio::test]
async fn deleting_a_folder_that_is_not_empty_is_reported() {
    let served = Served::start().await;
    fs::create_dir(served.local("logs")).unwrap();
    fs::write(served.local("logs/a.log"), b"a").unwrap();
    let error = served.sftp.delete_folder("/logs").await.unwrap_err();
    assert_eq!(
        error,
        FileError::NotEmpty {
            path: "/logs".into()
        }
    );
    assert_own_words(&error);
    assert!(served.local("logs/a.log").exists());
}

#[tokio::test]
async fn missing_files_give_their_own_message() {
    let served = Served::start().await;
    let sftp = &served.sftp;
    for error in [
        sftp.delete_file("/nope.txt").await.unwrap_err(),
        sftp.rename("/nope.txt", "/other.txt").await.unwrap_err(),
        sftp.open_read("/nope.txt").await.err().unwrap(),
        sftp.list("/nope").await.unwrap_err(),
    ] {
        assert!(matches!(error, FileError::NotFound { .. }), "{error:?}");
        assert_own_words(&error);
    }
}

#[tokio::test]
async fn permission_denied_gives_its_own_message() {
    let served = Served::start().await;
    fs::create_dir(served.local(SFTP_LOCKED)).unwrap();
    fs::write(served.local(&format!("{SFTP_LOCKED}/secret.txt")), b"x").unwrap();
    let path = format!("/{SFTP_LOCKED}/secret.txt");

    let error = served.sftp.open_read(&path).await.err().unwrap();
    assert_eq!(error, FileError::PermissionDenied { path: path.clone() });
    assert_eq!(error.to_string(), format!("permission denied: {path}"));
    let error = served.sftp.delete_file(&path).await.unwrap_err();
    assert_eq!(error, FileError::PermissionDenied { path });
}

#[tokio::test]
async fn a_file_written_through_the_write_stream_reads_back_the_same() {
    let served = Served::start().await;
    // A few megabytes that do not repeat in any short pattern.
    let mut state = 0x2545_f491_u32;
    let data: Vec<u8> = (0..3 * 1024 * 1024 + 4321)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();

    let mut writer = served.sftp.create("/image.bin").await.unwrap();
    for chunk in data.chunks(100_000) {
        writer.write_all(chunk).await.unwrap();
    }
    writer.finish().await.unwrap();
    assert_eq!(fs::read(served.local("image.bin")).unwrap(), data);

    let mut reader = served.sftp.open_read("/image.bin").await.unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(WAIT, reader.read_to_end(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert!(back == data, "{} bytes back of {}", back.len(), data.len());
    assert_eq!(
        served.sftp.stat("/image.bin").await.unwrap().size,
        Some(data.len() as u64)
    );
}

#[tokio::test]
async fn creating_a_file_replaces_one_of_that_name() {
    let served = Served::start().await;
    fs::write(served.local("r1.cfg"), b"a much longer old configuration").unwrap();
    let mut writer = served.sftp.create("/r1.cfg").await.unwrap();
    writer.write_all(b"new").await.unwrap();
    writer.finish().await.unwrap();
    assert_eq!(fs::read(served.local("r1.cfg")).unwrap(), b"new");
}

#[tokio::test]
async fn a_cut_connection_fails_the_next_operation_with_its_own_message() {
    let served = Served::start().await;
    served.server.cut_connections().await;
    tokio::time::timeout(WAIT, served.connection.lost())
        .await
        .unwrap();
    let error = served.sftp.list("/").await.unwrap_err();
    assert!(matches!(error, FileError::ConnectionLost), "{error:?}");
    assert_eq!(error.to_string(), "the connection was lost");
}

#[tokio::test]
async fn a_read_cut_half_way_fails_with_the_connection_lost() {
    let served = Served::start().await;
    fs::write(served.local("big.bin"), vec![7u8; 40 * 1024 * 1024]).unwrap();
    let mut reader = served.sftp.open_read("/big.bin").await.unwrap();
    let mut first = vec![0; 64 * 1024];
    reader.read_exact(&mut first).await.unwrap();

    served.server.cut_connections().await;
    let mut rest = Vec::new();
    let error = tokio::time::timeout(WAIT, reader.read_to_end(&mut rest))
        .await
        .expect("the read ended")
        .expect_err("the read failed");
    let error = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<FileError>())
        .cloned();
    assert_eq!(
        error,
        Some(FileError::ConnectionLost),
        "after {} bytes",
        rest.len()
    );
}

async fn no_sftp(server: &TestSshServer) -> NoSftp {
    let connection = FileConnection::open_unattended(server.target(), known_hosts())
        .await
        .unwrap();
    match tokio::time::timeout(WAIT, connection.start_sftp())
        .await
        .unwrap()
    {
        Err(StartError::NoSftp(why)) => why,
        other => panic!("expected no SFTP, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn a_device_that_refuses_the_subsystem_has_no_sftp() {
    let server = TestSshServer::start().await;
    server.refuse_sftp();
    let why = no_sftp(&server).await;
    assert_eq!(why, NoSftp::Refused);
    assert_eq!(
        StartError::NoSftp(why).to_string(),
        "the device does not offer SFTP: it refused the SFTP subsystem"
    );
}

#[tokio::test]
async fn a_device_that_accepts_the_subsystem_and_closes_at_once_has_no_sftp() {
    let server = TestSshServer::start().await;
    server.close_sftp_at_once();
    let why = no_sftp(&server).await;
    assert_eq!(why, NoSftp::ClosedBeforeReply);
    assert_eq!(
        StartError::NoSftp(why).to_string(),
        "the device does not offer SFTP: it closed the SFTP channel before answering"
    );
}

#[tokio::test]
async fn a_device_that_closes_the_channel_and_then_the_connection_has_no_sftp() {
    // IOS-XE: the connection ends with the channel, yet the channel was
    // closed in order first, which is the device saying it has no SFTP.
    let server = TestSshServer::start().await;
    server.close_sftp_and_hang_up();
    for _ in 0..10 {
        assert_eq!(no_sftp(&server).await, NoSftp::ClosedBeforeReply);
    }
}

#[tokio::test]
async fn sftp_can_start_again_after_a_device_without_it_said_no() {
    let server = TestSshServer::start().await;
    server.close_sftp_at_once();
    let connection = FileConnection::open_unattended(server.target(), known_hosts())
        .await
        .unwrap();
    assert!(connection.start_sftp().await.is_err());
    let folder = TempDir::new().unwrap();
    server.serve_sftp(folder.path());
    let sftp = connection.start_sftp().await.unwrap();
    assert_eq!(sftp.home().await.unwrap(), "/");
}
