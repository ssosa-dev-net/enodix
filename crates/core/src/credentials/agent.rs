//! The credential agent: the running app hands out credentials to the
//! headless binary (and through it, to scripts) the way an SSH agent hands
//! out signatures, so that a script needs no password written anywhere.
//!
//! [`AgentServer`] listens on the [`AgentEndpoint`] of a data folder (a
//! named pipe on Windows, a Unix socket elsewhere) that only the current
//! user can open, and answers from the store it is given (the app's
//! [`super::ActiveStore`]). [`AgentStore`] is the other end: a
//! [`CredentialStore`] that reads profiles and secrets over the pipe, so
//! that [`super::Profiles`] and [`super::Secrets`] (inheritance, jump
//! chains, names ignoring case) work over it unchanged. It only reads:
//! nothing can be stored or listed through the agent.
//!
//! The protocol is one JSON object per line each way, request then answer,
//! as many as the client likes on one connection. It is private to this
//! module: the app and the binary come from the same build.
//!
//! Like an SSH agent, the agent trusts every process of the same user.
//! Secrets travel only over the pipe and are wiped from the buffers that
//! carried them.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
#[cfg(windows)]
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroize;

use super::{
    CredentialProfile, CredentialStore, ProfileAuth, SecretValue, StoreContents, StoreError,
    VaultError, VaultStatus,
};
use crate::workspace::HostId;

/// The longest request line the agent reads (names and scopes are short).
const MAX_REQUEST: u64 = 64 * 1024;
/// The longest answer line a client reads.
const MAX_ANSWER: u64 = 1024 * 1024;
/// The version of what the binary prints ([`HostCredentials`],
/// [`NamedSecret`]).
const OUTPUT_FORMAT: u32 = 1;
/// How long a client waits for a busy pipe (every instance in use) before
/// giving up.
#[cfg(windows)]
const BUSY_WAIT: Duration = Duration::from_secs(5);

/// Where the agent of one data folder listens: a named pipe on Windows
/// (named after the folder, so another data folder, as the smoke and the
/// tests use, has its own), a Unix socket in the folder elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEndpoint {
    path: PathBuf,
}

impl AgentEndpoint {
    /// The agent of the app that keeps its data in `data_folder`.
    pub fn for_data_folder(data_folder: &Path) -> Self {
        let folder = std::path::absolute(data_folder).unwrap_or_else(|_| data_folder.into());
        Self::at_folder(&folder)
    }

    #[cfg(windows)]
    fn at_folder(folder: &Path) -> Self {
        // One spelling per folder: components, ignoring case.
        let spelling: Vec<String> = folder
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        let digest = Sha256::digest(spelling.join("\\").as_bytes());
        let id: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
        Self {
            path: PathBuf::from(format!(r"\\.\pipe\netterm-agent-{id}")),
        }
    }

    #[cfg(not(windows))]
    fn at_folder(folder: &Path) -> Self {
        Self {
            path: folder.join("agent.sock"),
        }
    }

    /// The pipe's name or the socket's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for AgentEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.display().fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// Something else already listens there (another instance of the app).
    #[error("another program already serves credentials at {0}")]
    InUse(String),
    /// What serves there runs as another user: it is asked nothing.
    #[error("the program serving credentials at {0} runs as another user")]
    NotOurs(String),
    /// What is there was not left by an agent of this user (Unix: not a
    /// socket, or one others can open): it is asked nothing.
    #[error("the credential agent is not used: {0}")]
    NotAgent(String),
    #[error("the credential agent failed: {0}")]
    Io(String),
    /// The agent answered something this build does not understand.
    #[error("the credential agent answered something unexpected: {0}")]
    Protocol(String),
}

impl From<io::Error> for AgentError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum Request {
    Status,
    Profile { scope: String, name: String },
    Secret { scope: String, name: String },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
enum Answer {
    Status {
        status: VaultStatus,
    },
    Profile {
        profile: Option<CredentialProfile>,
    },
    Secret {
        value: Option<SecretValue>,
    },
    /// The store is a vault that is not unlocked.
    Locked,
    Failed {
        message: String,
    },
}

impl Answer {
    fn wipe(&mut self) {
        match self {
            Self::Profile {
                profile: Some(profile),
            } => profile.wipe(),
            Self::Secret { value: Some(value) } => value.wipe(),
            _ => {}
        }
    }
}

/// What the agent answers `request` with, from `store`.
fn answer(store: &dyn CredentialStore, request: Request) -> Answer {
    if let Request::Status = request {
        return Answer::Status {
            status: store.vault_status(),
        };
    }
    if store.vault_status() == VaultStatus::Locked {
        return Answer::Locked;
    }
    let failed = |error: StoreError| Answer::Failed { message: error.0 };
    match request {
        Request::Status => unreachable!("answered above"),
        Request::Profile { scope, name } => match store.get(&scope, &name) {
            Ok(profile) => Answer::Profile { profile },
            Err(error) => failed(error),
        },
        Request::Secret { scope, name } => match store.get_secret(&scope, &name) {
            Ok(value) => Answer::Secret { value },
            Err(error) => failed(error),
        },
    }
}

/// The running agent. Dropping it stops listening (connections being
/// answered end with it).
pub struct AgentServer {
    endpoint: AgentEndpoint,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for AgentServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentServer")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl AgentServer {
    /// Starts answering at `endpoint` from `store`, on a thread of its own.
    /// Refused ([`AgentError::InUse`]) if something already listens there.
    pub fn start(
        endpoint: AgentEndpoint,
        store: Arc<dyn CredentialStore>,
    ) -> Result<Self, AgentError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let listener = {
            let _entered = runtime.enter();
            listener::Listener::bind(&endpoint)?
        };
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("credential agent".into())
            .spawn(move || runtime.block_on(listener.serve(store, stopped)))?;
        Ok(Self {
            endpoint,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self) -> &AgentEndpoint {
        &self.endpoint
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Answers the requests of one connection until it closes.
async fn serve_connection<S: AsyncRead + AsyncWrite>(stream: S, store: Arc<dyn CredentialStore>) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);
    loop {
        let mut line = Vec::new();
        match (&mut reader)
            .take(MAX_REQUEST)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let too_long = !line.ends_with(b"\n");
        let mut answer = match serde_json::from_slice::<Request>(&line) {
            _ if too_long => Answer::Failed {
                message: "the request is too long".into(),
            },
            // The store may be slow (the keychain, a vault being unlocked):
            // other connections go on meanwhile.
            Ok(request) => {
                let store = store.clone();
                tokio::task::spawn_blocking(move || answer(store.as_ref(), request))
                    .await
                    .unwrap_or_else(|error| Answer::Failed {
                        message: error.to_string(),
                    })
            }
            Err(error) => Answer::Failed {
                message: format!("the request is not understood: {error}"),
            },
        };
        let mut text = serde_json::to_vec(&answer).unwrap_or_default();
        answer.wipe();
        text.push(b'\n');
        let written = writer.write_all(&text).await;
        text.zeroize();
        if written.is_err() || too_long || writer.flush().await.is_err() {
            return;
        }
    }
}

#[cfg(windows)]
mod security {
    //! A security descriptor that grants the current user everything
    //! and nobody else anything.

    use std::ffi::c_void;
    use std::io;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub struct OwnerOnly {
        descriptor: PSECURITY_DESCRIPTOR,
        attributes: Box<SECURITY_ATTRIBUTES>,
    }

    // SAFETY: the descriptor is owned by this value alone and never
    // changed after it is made; it is only read by the system.
    unsafe impl Send for OwnerOnly {}

    impl OwnerOnly {
        pub fn new() -> io::Result<Self> {
            let sid = current_user_sid()?;
            // Protected (no inherited entries), one entry: all access
            // for this user.
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
                .encode_utf16()
                .chain([0])
                .collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
            // SAFETY: `sddl` is NUL-terminated; the descriptor the call
            // allocates is freed in `drop`.
            let made = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            };
            if made == 0 {
                return Err(io::Error::last_os_error());
            }
            let attributes = Box::new(SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            });
            Ok(Self {
                descriptor,
                attributes,
            })
        }

        pub fn attributes(&self) -> *mut c_void {
            ptr::from_ref::<SECURITY_ATTRIBUTES>(&self.attributes)
                .cast_mut()
                .cast()
        }
    }

    impl Drop for OwnerOnly {
        fn drop(&mut self) {
            // SAFETY: allocated by the conversion in `new`.
            unsafe { LocalFree(self.descriptor) };
        }
    }

    /// The user the process serving the named pipe `pipe` (a client's
    /// handle) runs as, as an `S-1-…` string.
    pub fn pipe_server_sid(pipe: HANDLE) -> io::Result<String> {
        let mut id = 0u32;
        // SAFETY: a valid pipe handle and out pointer.
        if unsafe { GetNamedPipeServerProcessId(pipe, &mut id) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: plain calls; the handles are closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, id) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: a valid process handle and out pointer.
        let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
        let result = if opened == 0 {
            Err(io::Error::last_os_error())
        } else {
            let sid = token_user_sid(token);
            // SAFETY: opened above.
            unsafe { CloseHandle(token) };
            sid
        };
        // SAFETY: opened above.
        unsafe { CloseHandle(process) };
        result
    }

    /// The current process's user, as an `S-1-…` string.
    pub fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: plain calls with valid out pointers; the token is
        // closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = token_user_sid(token);
        // SAFETY: opened above.
        unsafe { CloseHandle(token) };
        result
    }

    fn token_user_sid(token: HANDLE) -> io::Result<String> {
        let mut length = 0u32;
        // SAFETY: asks for the size only.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length) };
        // u64s, so that the TOKEN_USER read from it is aligned.
        let mut buffer = vec![0u64; (length as usize).div_ceil(8).max(1)];
        // SAFETY: the buffer holds `length` bytes.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call filled the buffer with a TOKEN_USER.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut text = ptr::null_mut();
        // SAFETY: the SID points into `buffer`; the string the call
        // allocates is freed below.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a NUL-terminated string from the call.
        let sid = unsafe {
            let length = (0..).take_while(|&i| *text.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
        };
        // SAFETY: allocated by the conversion.
        unsafe { LocalFree(text.cast()) };
        Ok(sid)
    }
}

#[cfg(windows)]
mod listener {
    //! A named pipe whose only access rule lets the current user in,
    //! refusing remote clients, created as the pipe's first instance (so
    //! nobody can have made it before, to listen in our place).

    use std::io;
    use std::sync::Arc;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
    use tokio::sync::oneshot;

    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

    use super::{security, serve_connection, AgentEndpoint, AgentError, CredentialStore};

    pub struct Listener {
        name: String,
        security: security::OwnerOnly,
        next: NamedPipeServer,
    }

    impl Listener {
        pub fn bind(endpoint: &AgentEndpoint) -> Result<Self, AgentError> {
            let name = endpoint.path().to_string_lossy().into_owned();
            let security = security::OwnerOnly::new()?;
            let next = create(&name, &security, true).map_err(|error| {
                // The pipe exists already.
                if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
                    AgentError::InUse(name.clone())
                } else {
                    AgentError::from(error)
                }
            })?;
            Ok(Self {
                name,
                security,
                next,
            })
        }

        pub async fn serve(
            mut self,
            store: Arc<dyn CredentialStore>,
            mut stop: oneshot::Receiver<()>,
        ) {
            loop {
                tokio::select! {
                    _ = &mut stop => return,
                    connected = self.next.connect() => {
                        // The next client needs an instance to connect to.
                        let next = match create(&self.name, &self.security, false) {
                            Ok(next) => next,
                            Err(error) => {
                                eprintln!("the credential agent stops: {error}");
                                return;
                            }
                        };
                        let client = std::mem::replace(&mut self.next, next);
                        if connected.is_ok() {
                            tokio::spawn(serve_connection(client, store.clone()));
                        }
                    }
                }
            }
        }
    }

    fn create(
        name: &str,
        security: &security::OwnerOnly,
        first: bool,
    ) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        // SAFETY: the attributes and the descriptor they point to live in
        // `security`, which outlives the call; the system copies them.
        unsafe { options.create_with_security_attributes_raw(name, security.attributes()) }
    }
}

#[cfg(unix)]
mod listener {
    //! A Unix socket only its owner can open (mode 0600), which also
    //! checks each peer's user id.

    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::net::UnixListener;
    use tokio::sync::oneshot;

    use super::{serve_connection, AgentEndpoint, AgentError, CredentialStore};

    pub struct Listener {
        path: PathBuf,
        owner: u32,
        listener: UnixListener,
    }

    impl Listener {
        pub fn bind(endpoint: &AgentEndpoint) -> Result<Self, AgentError> {
            let path = endpoint.path().to_path_buf();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            if path.exists() {
                // A socket nobody answers on is left over from a crash.
                if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                    return Err(AgentError::InUse(path.display().to_string()));
                }
                fs::remove_file(&path)?;
            }
            // Made under another name and given its mode there, then moved
            // into place: a client never finds it open to others (it would
            // refuse it).
            let made = path.with_extension(format!("sock.{}", std::process::id()));
            let _ = fs::remove_file(&made);
            let listener = UnixListener::bind(&made)?;
            fs::set_permissions(&made, fs::Permissions::from_mode(0o600))
                .and_then(|()| fs::rename(&made, &path))
                .inspect_err(|_| {
                    let _ = fs::remove_file(&made);
                })?;
            // The socket is ours: its owner is this process's user.
            let owner = fs::metadata(&path)?.uid();
            Ok(Self {
                path,
                owner,
                listener,
            })
        }

        pub async fn serve(self, store: Arc<dyn CredentialStore>, mut stop: oneshot::Receiver<()>) {
            loop {
                tokio::select! {
                    _ = &mut stop => break,
                    accepted = self.listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            // Out of descriptors, say: do not spin.
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            continue;
                        };
                        let same_user = stream
                            .peer_cred()
                            .is_ok_and(|peer| peer.uid() == self.owner);
                        if same_user {
                            tokio::spawn(serve_connection(stream, store.clone()));
                        }
                    }
                }
            }
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(windows)]
type Stream = std::fs::File;
#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

/// A connection to a running agent.
struct Connection {
    reader: BufReader<Stream>,
    writer: Stream,
}

impl Connection {
    /// `None` if no agent listens at `endpoint`.
    fn open(endpoint: &AgentEndpoint) -> Result<Option<Self>, AgentError> {
        let Some(stream) = connect(endpoint)? else {
            return Ok(None);
        };
        let writer = stream.try_clone()?;
        Ok(Some(Self {
            reader: BufReader::new(stream),
            writer,
        }))
    }

    fn ask(&mut self, request: &Request) -> Result<Answer, AgentError> {
        let mut line = serde_json::to_vec(request).map_err(|e| AgentError::Io(e.to_string()))?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.writer.flush()?;
        let mut answer = Vec::new();
        (&mut self.reader)
            .take(MAX_ANSWER)
            .read_until(b'\n', &mut answer)?;
        if answer.is_empty() {
            return Err(AgentError::Io("the agent closed the connection".into()));
        }
        let parsed = serde_json::from_slice(&answer)
            .map_err(|error| AgentError::Protocol(error.to_string()));
        answer.zeroize();
        parsed
    }
}

/// Opens the pipe, letting the server identify this process but never act
/// as it, and checks that the server runs as this user (anyone can take a
/// pipe name first).
#[cfg(windows)]
fn connect(endpoint: &AgentEndpoint) -> Result<Option<Stream>, AgentError> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;

    let started = Instant::now();
    let pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(endpoint.path())
        {
            Ok(file) => break file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            // Between two instances; try again shortly.
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && started.elapsed() < BUSY_WAIT =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let server = security::pipe_server_sid(pipe.as_raw_handle())?;
    if server != security::current_user_sid()? {
        return Err(AgentError::NotOurs(endpoint.to_string()));
    }
    Ok(Some(pipe))
}

/// Connects to the socket if it is one this user's agent made: a socket
/// (not a file or a link), owned by this user and open to nobody else, as
/// the agent leaves it, whose listener runs as this user. Anything else is
/// refused rather than asked (a data folder shared with others could hold
/// another user's socket).
#[cfg(unix)]
fn connect(endpoint: &AgentEndpoint) -> Result<Option<Stream>, AgentError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let path = endpoint.path();
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // SAFETY: no arguments, cannot fail.
    let user = unsafe { libc::geteuid() };
    if !metadata.file_type().is_socket() {
        return Err(AgentError::NotAgent(format!("{endpoint} is not a socket")));
    }
    if metadata.uid() != user {
        return Err(AgentError::NotOurs(endpoint.to_string()));
    }
    let mode = metadata.mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(AgentError::NotAgent(format!(
            "{endpoint} can be opened by other users (mode {mode:o})"
        )));
    }
    let stream = match std::os::unix::net::UnixStream::connect(path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error.into()),
    };
    // The socket could have been swapped since it was looked at.
    if peer_user(&stream)? != user {
        return Err(AgentError::NotOurs(endpoint.to_string()));
    }
    Ok(Some(stream))
}

/// The user id the other end of `stream` runs as.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_user(stream: &std::os::unix::net::UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;

    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the buffer and its length are this function's own.
    let done = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if done != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

/// The user id the other end of `stream` runs as.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn peer_user(stream: &std::os::unix::net::UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;

    let (mut user, mut group) = (0, 0);
    // SAFETY: both out-parameters are this function's own.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut user, &mut group) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(user)
}

/// The store a running agent serves, read over its pipe. Only reads
/// single profiles and secrets ([`CredentialStore::get`],
/// [`CredentialStore::get_secret`]); everything else is refused. A locked
/// vault on the other side refuses with [`VaultError::Locked`]'s message.
pub struct AgentStore {
    endpoint: AgentEndpoint,
    connection: Mutex<Connection>,
}

impl fmt::Debug for AgentStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentStore")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl AgentStore {
    /// Connects to the agent at `endpoint`; `None` if none is running.
    pub fn connect(endpoint: &AgentEndpoint) -> Result<Option<Self>, AgentError> {
        Ok(Connection::open(endpoint)?.map(|connection| Self {
            endpoint: endpoint.clone(),
            connection: Mutex::new(connection),
        }))
    }

    /// Whether the store the agent serves is a vault, and whether it is
    /// unlocked (while [`VaultStatus::Locked`] it gives nothing out).
    pub fn status(&self) -> Result<VaultStatus, AgentError> {
        match self.ask(&Request::Status)? {
            Answer::Status { status } => Ok(status),
            other => Err(unexpected(&other)),
        }
    }

    fn ask(&self, request: &Request) -> Result<Answer, AgentError> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ask(request)
    }

    fn read(&self, request: &Request) -> Result<Answer, StoreError> {
        match self.ask(request) {
            Ok(Answer::Locked) => Err(StoreError(VaultError::Locked.to_string())),
            Ok(Answer::Failed { message }) => Err(StoreError(message)),
            Ok(answer) => Ok(answer),
            Err(error) => Err(StoreError(error.to_string())),
        }
    }
}

/// An answer of the wrong kind, as a store error.
fn mismatch(answer: &Answer) -> StoreError {
    StoreError(unexpected(answer).to_string())
}

fn unexpected(answer: &Answer) -> AgentError {
    let kind = match answer {
        Answer::Status { .. } => "status",
        Answer::Profile { .. } => "profile",
        Answer::Secret { .. } => "secret",
        Answer::Locked => "locked",
        Answer::Failed { .. } => "failed",
    };
    AgentError::Protocol(format!("a {kind} answer"))
}

fn read_only() -> StoreError {
    StoreError("the credential agent only gives out profiles and secrets by name".into())
}

impl CredentialStore for AgentStore {
    fn list(&self, _scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        Err(read_only())
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        let request = Request::Profile {
            scope: scope.into(),
            name: name.into(),
        };
        match self.read(&request)? {
            Answer::Profile { profile } => Ok(profile),
            other => Err(mismatch(&other)),
        }
    }

    fn put(&self, _scope: &str, _profile: &CredentialProfile) -> Result<(), StoreError> {
        Err(read_only())
    }

    fn delete(&self, _scope: &str, _name: &str) -> Result<bool, StoreError> {
        Err(read_only())
    }

    fn secret_names(&self, _scope: &str) -> Result<Vec<String>, StoreError> {
        Err(read_only())
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        let request = Request::Secret {
            scope: scope.into(),
            name: name.into(),
        };
        match self.read(&request)? {
            Answer::Secret { value } => Ok(value),
            other => Err(mismatch(&other)),
        }
    }

    fn put_secret(
        &self,
        _scope: &str,
        _name: &str,
        _value: &SecretValue,
    ) -> Result<(), StoreError> {
        Err(read_only())
    }

    fn delete_secret(&self, _scope: &str, _name: &str) -> Result<bool, StoreError> {
        Err(read_only())
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        Err(read_only())
    }

    /// As the agent reports it; locked if it cannot be asked.
    fn vault_status(&self) -> VaultStatus {
        self.status().unwrap_or(VaultStatus::Locked)
    }
}

/// A host's login as the headless binary hands it to a script (format 1):
/// the profile its settings resolve to, secrets included. An agent profile
/// carries no secret (`{"kind": "agent"}`): the script's library uses the
/// SSH agent itself.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct HostCredentials {
    pub format: u32,
    pub host: HostId,
    /// The profile's name as stored.
    pub profile: String,
    pub username: String,
    /// `{"kind": "password", "password"}`, `{"kind": "private_key",
    /// "key_file", "passphrase"}` or `{"kind": "agent"}`.
    pub auth: ProfileAuth,
    pub enable_secret: Option<String>,
}

impl HostCredentials {
    pub fn new(host: HostId, profile: CredentialProfile) -> Self {
        let CredentialProfile {
            name,
            username,
            auth,
            enable_secret,
        } = profile;
        Self {
            format: OUTPUT_FORMAT,
            host,
            profile: name,
            username,
            auth,
            enable_secret,
        }
    }
}

impl fmt::Debug for HostCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostCredentials")
            .field("host", &self.host)
            .field("profile", &self.profile)
            .field("username", &self.username)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl Drop for HostCredentials {
    fn drop(&mut self) {
        let mut profile = CredentialProfile {
            name: String::new(),
            username: String::new(),
            auth: std::mem::replace(&mut self.auth, ProfileAuth::Agent),
            enable_secret: self.enable_secret.take(),
        };
        profile.wipe();
    }
}

/// A named secret as the headless binary hands it to a script (format 1).
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct NamedSecret {
    pub format: u32,
    /// As asked for.
    pub name: String,
    pub value: SecretValue,
}

impl NamedSecret {
    pub fn new(name: impl Into<String>, value: SecretValue) -> Self {
        Self {
            format: OUTPUT_FORMAT,
            name: name.into(),
            value,
        }
    }
}

impl fmt::Debug for NamedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamedSecret")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Drop for NamedSecret {
    fn drop(&mut self) {
        self.value.wipe();
    }
}
