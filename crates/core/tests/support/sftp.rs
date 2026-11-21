//! The test SSH server's SFTP side: a folder on disk served over the `sftp`
//! subsystem with `russh-sftp`'s server half, as a device's file system.
//!
//! Paths are absolute from the served folder, which is `/` and also where
//! the login lands ([`SFTP_HOME`]). Like OpenSSH's server with SFTP v3, a
//! listing includes `.` and `..`, and a refusal that has no code of its own
//! is a bare `Failure`: renaming onto an existing name, making a folder that
//! exists, deleting a folder that is not empty. Anything inside a folder
//! named [`SFTP_LOCKED`] is refused as permission denied.
//!
//! [`SftpQuirks`] make it misbehave as devices do, for the transfer
//! engine's tests: holding transfers mid-file (a device that stops
//! answering), keeping fewer bytes than were written, refusing removes (a
//! Junos read-only login).

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use russh_sftp::server::StatusReply;
use tokio::sync::watch;

/// Where the login lands: the served folder.
pub const SFTP_HOME: &str = "/";

/// A folder name whose contents the server refuses (permission denied).
pub const SFTP_LOCKED: &str = "locked";

/// How the server answers a request for the `sftp` subsystem.
#[derive(Debug, Clone, Default)]
pub enum SftpMode {
    /// Refused (the default, as a device without SFTP that says so).
    #[default]
    Refuse,
    /// Accepted, then the channel is closed before SFTP's first reply; the
    /// connection stays.
    CloseAtOnce,
    /// As `CloseAtOnce`, and then the whole connection is closed: what
    /// IOS-XE really does (it ends the connection when a channel ends).
    CloseAndHangUp,
    /// This folder is served.
    Serve(PathBuf),
}

/// How the SFTP side misbehaves, shared by every session of a server.
pub struct SftpQuirks {
    /// Reads and writes from this offset on wait until released. SFTP's
    /// requests are answered one at a time, so everything after them
    /// waits too, as on a device that stopped answering.
    hold: watch::Sender<Option<u64>>,
    /// Writes past this many bytes of a file are acknowledged and dropped.
    keep_at_most: Mutex<Option<u64>>,
    /// Removing a file is refused as permission denied.
    refuse_remove: AtomicBool,
}

impl Default for SftpQuirks {
    fn default() -> Self {
        Self {
            hold: watch::Sender::new(None),
            keep_at_most: Mutex::new(None),
            refuse_remove: AtomicBool::new(false),
        }
    }
}

impl SftpQuirks {
    /// From now on, reads and writes from `offset` on wait (and every
    /// request after them) until [`Self::release`].
    pub fn hold_from(&self, offset: u64) {
        self.hold.send_replace(Some(offset));
    }

    /// Lets held reads and writes go on.
    pub fn release(&self) {
        self.hold.send_replace(None);
    }

    pub fn keep_at_most(&self, bytes: u64) {
        *self.keep_at_most.lock().unwrap() = Some(bytes);
    }

    pub fn refuse_remove(&self) {
        self.refuse_remove.store(true, Ordering::SeqCst);
    }

    /// Waits while a hold covers `offset`.
    async fn held(&self, offset: u64) {
        let mut hold = self.hold.subscribe();
        let _ = hold
            .wait_for(|from| from.is_none_or(|from| offset < from))
            .await;
    }
}

/// One client's SFTP session over the served folder.
pub struct FolderServer {
    root: PathBuf,
    handles: HashMap<String, Open>,
    next: u64,
    quirks: Arc<SftpQuirks>,
}

enum Open {
    File(fs::File),
    /// A folder's entries, given out once.
    Folder(Option<Vec<File>>),
}

impl FolderServer {
    pub fn new(root: PathBuf, quirks: Arc<SftpQuirks>) -> Self {
        Self {
            root,
            handles: HashMap::new(),
            next: 0,
            quirks,
        }
    }

    /// The path on disk of the SFTP path `path` (relative to the home), or
    /// a refusal for a locked one.
    fn local(&self, path: &str) -> Result<PathBuf, StatusReply> {
        let virtual_path = normalized(path);
        if virtual_path
            .components()
            .any(|component| component.as_os_str() == SFTP_LOCKED)
            && virtual_path
                .file_name()
                .is_some_and(|name| name != SFTP_LOCKED)
        {
            return Err(StatusCode::PermissionDenied.into());
        }
        Ok(self
            .root
            .join(virtual_path.strip_prefix("/").unwrap_or(&virtual_path)))
    }

    fn handle(&mut self, open: Open) -> String {
        self.next += 1;
        let handle = self.next.to_string();
        self.handles.insert(handle.clone(), open);
        handle
    }
}

/// `path` as an absolute path from the served folder, with `.` and `..`
/// applied (never above `/`).
fn normalized(path: &str) -> PathBuf {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    PathBuf::from(format!("/{}", parts.join("/")))
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

/// What an I/O error is in SFTP v3's few codes.
fn refusal(error: &io::Error) -> StatusReply {
    match error.kind() {
        io::ErrorKind::NotFound => StatusCode::NoSuchFile.into(),
        io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied.into(),
        _ => StatusCode::Failure.into(),
    }
}

fn attributes(metadata: &fs::Metadata) -> FileAttributes {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        0o120000
    } else if file_type.is_dir() {
        0o040000
    } else {
        0o100000
    };
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let permissions = match (file_type.is_dir(), metadata.permissions().readonly()) {
        (true, _) => 0o755,
        (false, true) => 0o444,
        (false, false) => 0o644,
    };
    let seconds = |time: io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs() as u32)
    };
    FileAttributes {
        size: Some(metadata.len()),
        uid: Some(1000),
        user: None,
        gid: Some(1000),
        group: None,
        permissions: Some(kind | permissions),
        atime: seconds(metadata.accessed()),
        mtime: seconds(metadata.modified()),
    }
}

impl russh_sftp::server::Handler for FolderServer {
    type Error = StatusReply;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported.into()
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = normalized(&path).to_string_lossy().into_owned();
        Ok(Name {
            id,
            files: vec![File::dummy(path)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = fs::metadata(self.local(&path)?).map_err(|e| refusal(&e))?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = fs::symlink_metadata(self.local(&path)?).map_err(|e| refusal(&e))?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        match self.handles.get(&handle) {
            Some(Open::File(file)) => {
                let metadata = file.metadata().map_err(|e| refusal(&e))?;
                Ok(Attrs {
                    id,
                    attrs: attributes(&metadata),
                })
            }
            _ => Err(StatusCode::Failure.into()),
        }
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let folder = self.local(&path)?;
        let mut files = Vec::new();
        for name in [".", ".."] {
            let metadata = fs::metadata(&folder).map_err(|e| refusal(&e))?;
            files.push(File::new(name, attributes(&metadata)));
        }
        for entry in fs::read_dir(&folder).map_err(|e| refusal(&e))? {
            let entry = entry.map_err(|e| refusal(&e))?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|e| refusal(&e))?;
            files.push(File::new(
                entry.file_name().to_string_lossy(),
                attributes(&metadata),
            ));
        }
        let handle = self.handle(Open::Folder(Some(files)));
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Folder(entries)) => match entries.take() {
                Some(files) => Ok(Name { id, files }),
                None => Err(StatusCode::Eof.into()),
            },
            _ => Err(StatusCode::Failure.into()),
        }
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        match self.handles.remove(&handle) {
            Some(_) => Ok(ok(id)),
            None => Err(StatusCode::Failure.into()),
        }
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        fs::create_dir(self.local(&path)?).map_err(|e| refusal(&e))?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        fs::remove_dir(self.local(&path)?).map_err(|e| refusal(&e))?;
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let file = self.local(&filename)?;
        if self.quirks.refuse_remove.load(Ordering::SeqCst) {
            return Err(StatusCode::PermissionDenied.into());
        }
        if file.is_dir() {
            return Err(StatusCode::Failure.into());
        }
        fs::remove_file(file).map_err(|e| refusal(&e))?;
        Ok(ok(id))
    }

    /// SFTP v3's rename: an existing new name is a failure (Windows'
    /// rename would replace it).
    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let from = self.local(&oldpath)?;
        let to = self.local(&newpath)?;
        if fs::symlink_metadata(&from).is_err() {
            return Err(StatusCode::NoSuchFile.into());
        }
        if fs::symlink_metadata(&to).is_ok() {
            return Err(StatusCode::Failure.into());
        }
        fs::rename(from, to).map_err(|e| refusal(&e))?;
        Ok(ok(id))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.local(&filename)?;
        if path.is_dir() {
            return Err(StatusCode::Failure.into());
        }
        let options: fs::OpenOptions = pflags.into();
        let file = options.open(path).map_err(|e| refusal(&e))?;
        let handle = self.handle(Open::File(file));
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        self.quirks.held(offset).await;
        let Some(Open::File(file)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.into());
        };
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| refusal(&e))?;
        let mut data = vec![0; len as usize];
        let mut read = 0;
        while read < data.len() {
            match file.read(&mut data[read..]).map_err(|e| refusal(&e))? {
                0 => break,
                n => read += n,
            }
        }
        if read == 0 {
            return Err(StatusCode::Eof.into());
        }
        data.truncate(read);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        mut data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        self.quirks.held(offset).await;
        let keep = *self.quirks.keep_at_most.lock().unwrap();
        if let Some(keep) = keep {
            data.truncate(keep.saturating_sub(offset) as usize);
            if data.is_empty() {
                return Ok(ok(id));
            }
        }
        let Some(Open::File(file)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure.into());
        };
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| refusal(&e))?;
        file.write_all(&data).map_err(|e| refusal(&e))?;
        Ok(ok(id))
    }
}
