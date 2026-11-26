//! An SSH agent that runs inside the test process (`russh`'s agent server),
//! so tests never touch the user's own agent or keys.
//!
//! On Windows it listens on a named pipe with a random name, as the Windows
//! OpenSSH agent does on its fixed one; elsewhere on a Unix socket in a
//! temporary folder. [`TestAgent::start_as_pageant`] listens where Pageant
//! does instead.

use std::path::{Path, PathBuf};

use russh::keys::agent::client::AgentClient;
use russh::keys::{Certificate, PrivateKey};

pub struct TestAgent {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
    /// The folder holding the socket, removed with the agent.
    #[cfg(unix)]
    folder: Option<tempfile::TempDir>,
}

impl TestAgent {
    /// Starts an agent holding `keys`, at a fresh address.
    pub async fn start(keys: &[PrivateKey]) -> Self {
        #[cfg(windows)]
        {
            let name = format!(r"\\.\pipe\enodix-test-agent-{:016x}", rand::random::<u64>());
            Self::start_at(PathBuf::from(name), keys)
                .await
                .expect("the pipe name is random")
        }
        #[cfg(unix)]
        {
            let folder = tempfile::tempdir().unwrap();
            let mut agent = Self::start_at(folder.path().join("agent.sock"), keys)
                .await
                .expect("the folder is new");
            agent.folder = Some(folder);
            agent
        }
    }

    /// Starts an agent holding `keys` on the pipe Pageant listens on, for
    /// this user. `None` if that pipe is taken: Pageant itself is running,
    /// and the test must not stand in for it.
    #[cfg(windows)]
    pub async fn start_as_pageant(keys: &[PrivateKey]) -> Option<Self> {
        Self::start_at(PathBuf::from(pageant_pipe_name()), keys).await
    }

    /// Where the agent listens.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Starts an agent holding `keys` on the named pipe (Windows) or Unix
    /// socket `path`; `None` if something already listens there.
    #[cfg(windows)]
    pub async fn start_at(path: PathBuf, keys: &[PrivateKey]) -> Option<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&path)
            .ok()?;
        let pipe = path.clone();
        // Each client gets its own pipe instance; the next one is created
        // as soon as one is taken.
        let listener = futures::stream::unfold(first, move |server| {
            let pipe = pipe.clone();
            async move {
                let connected = server.connect().await.map(|()| server);
                let next = ServerOptions::new().create(&pipe).ok()?;
                Some((connected, next))
            }
        });
        let task = tokio::spawn(async move {
            let _ = russh::keys::agent::server::serve(Box::pin(listener), ()).await;
        });
        let agent = Self { path, task };
        agent.add(keys).await;
        Some(agent)
    }

    #[cfg(unix)]
    pub async fn start_at(path: PathBuf, keys: &[PrivateKey]) -> Option<Self> {
        let listener = tokio::net::UnixListener::bind(&path).ok()?;
        let listener = futures::stream::unfold(listener, |listener| async move {
            let accepted = listener.accept().await.map(|(stream, _)| stream);
            Some((accepted, listener))
        });
        let task = tokio::spawn(async move {
            let _ = russh::keys::agent::server::serve(Box::pin(listener), ()).await;
        });
        let agent = Self {
            path,
            task,
            folder: None,
        };
        agent.add(keys).await;
        Some(agent)
    }

    async fn add(&self, keys: &[PrivateKey]) {
        #[cfg(windows)]
        let mut client = AgentClient::connect_named_pipe(&self.path).await.unwrap();
        #[cfg(unix)]
        let mut client = AgentClient::connect_uds(&self.path).await.unwrap();
        for key in keys {
            client.add_identity(key, &[]).await.unwrap();
        }
    }
}

/// Agent protocol messages (draft-miller-ssh-agent) the certificate agent
/// answers.
const REQUEST_IDENTITIES: u8 = 11;
const IDENTITIES_ANSWER: u8 = 12;
const SIGN_REQUEST: u8 = 13;
const SIGN_RESPONSE: u8 = 14;
const FAILURE: u8 = 5;

impl TestAgent {
    /// Starts an agent holding one OpenSSH certificate (of `key`, which
    /// signs for it), at a fresh address. russh's agent server holds no
    /// certificates, so this one answers the two requests a login makes
    /// (list, sign) itself. Ed25519 keys only.
    #[cfg(unix)]
    pub async fn start_with_certificate(key: PrivateKey, certificate: Certificate) -> Self {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("agent.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_certificate(stream, key.clone(), certificate.clone()));
            }
        });
        Self {
            path,
            task,
            folder: Some(folder),
        }
    }

    #[cfg(windows)]
    pub async fn start_with_certificate(key: PrivateKey, certificate: Certificate) -> Self {
        use tokio::net::windows::named_pipe::ServerOptions;

        let name = format!(r"\\.\pipe\enodix-test-agent-{:016x}", rand::random::<u64>());
        let path = PathBuf::from(name);
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&path)
            .unwrap();
        let pipe = path.clone();
        let task = tokio::spawn(async move {
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                let Ok(next) = ServerOptions::new().create(&pipe) else {
                    return;
                };
                let connected = std::mem::replace(&mut server, next);
                tokio::spawn(serve_certificate(
                    connected,
                    key.clone(),
                    certificate.clone(),
                ));
            }
        });
        Self { path, task }
    }
}

/// Answers one client of the certificate agent until it hangs up.
async fn serve_certificate<S>(mut stream: S, key: PrivateKey, certificate: Certificate)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use russh::keys::signature::Signer;
    use russh::keys::ssh_encoding::Encode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let blob = certificate.encode_vec().unwrap();
    loop {
        let Ok(length) = stream.read_u32().await else {
            return;
        };
        let mut message = vec![0; length as usize];
        if stream.read_exact(&mut message).await.is_err() {
            return;
        }
        let mut reply = Vec::new();
        match message.first() {
            Some(&REQUEST_IDENTITIES) => {
                reply.push(IDENTITIES_ANSWER);
                1u32.encode(&mut reply).unwrap();
                blob.as_slice().encode(&mut reply).unwrap();
                "test certificate".encode(&mut reply).unwrap();
            }
            Some(&SIGN_REQUEST) => {
                // string key blob, string data, uint32 flags.
                let mut rest = &message[1..];
                let read = |rest: &mut &[u8]| -> Option<Vec<u8>> {
                    let (length, tail) = rest.split_at_checked(4)?;
                    let length = u32::from_be_bytes(length.try_into().ok()?) as usize;
                    let (value, tail) = tail.split_at_checked(length)?;
                    *rest = tail;
                    Some(value.to_vec())
                };
                match (read(&mut rest), read(&mut rest)) {
                    (Some(asked), Some(data)) if asked == blob => {
                        let signature: russh::keys::ssh_key::Signature =
                            key.try_sign(&data).unwrap();
                        reply.push(SIGN_RESPONSE);
                        signature
                            .encode_vec()
                            .unwrap()
                            .as_slice()
                            .encode(&mut reply)
                            .unwrap();
                    }
                    _ => reply.push(FAILURE),
                }
            }
            _ => reply.push(FAILURE),
        }
        let mut framed = (reply.len() as u32).to_be_bytes().to_vec();
        framed.extend(reply);
        if stream.write_all(&framed).await.is_err() {
            return;
        }
    }
}

/// An agent address nothing listens at.
pub fn unused_path() -> PathBuf {
    let name = format!("enodix-test-no-agent-{:016x}", rand::random::<u64>());
    #[cfg(windows)]
    return PathBuf::from(r"\\.\pipe").join(name);
    #[cfg(unix)]
    return std::env::temp_dir().join(name);
}

impl Drop for TestAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The named pipe Pageant (0.75 and later) listens on for the current
/// user, as PuTTY derives it: `pageant.<user>.<sha256 of the
/// CryptProtectMemory-scrambled word "Pageant">`.
#[cfg(windows)]
pub fn pageant_pipe_name() -> String {
    use sha2::{Digest, Sha256};
    use windows_sys::Win32::Security::Authentication::Identity::{
        GetUserNameExA, NameUserPrincipal,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectMemory, CRYPTPROTECTMEMORY_BLOCK_SIZE, CRYPTPROTECTMEMORY_CROSS_PROCESS,
    };
    use windows_sys::Win32::System::WindowsProgramming::GetUserNameA;

    let mut length = 0u32;
    // The user principal name if there is one, else the account name.
    unsafe { GetUserNameExA(NameUserPrincipal, std::ptr::null_mut(), &mut length) };
    let mut name = vec![0u8; length as usize];
    let found =
        length > 0 && unsafe { GetUserNameExA(NameUserPrincipal, name.as_mut_ptr(), &mut length) };
    if !found {
        length = 0;
        unsafe { GetUserNameA(std::ptr::null_mut(), &mut length) };
        name = vec![0u8; length as usize];
        assert_ne!(unsafe { GetUserNameA(name.as_mut_ptr(), &mut length) }, 0);
    }
    let name = std::ffi::CStr::from_bytes_until_nul(&name)
        .unwrap()
        .to_str()
        .unwrap();
    let user = name.split_once('@').map_or(name, |(user, _)| user);

    let word = b"Pageant";
    let size = (word.len() + 1).next_multiple_of(CRYPTPROTECTMEMORY_BLOCK_SIZE as usize);
    let mut scrambled = vec![0u8; size];
    scrambled[..word.len()].copy_from_slice(word);
    unsafe {
        CryptProtectMemory(
            scrambled.as_mut_ptr().cast(),
            size as u32,
            CRYPTPROTECTMEMORY_CROSS_PROCESS,
        )
    };
    let mut hash = Sha256::new();
    hash.update((size as u32).to_be_bytes());
    hash.update(&scrambled);
    let digest: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(r"\\.\pipe\pageant.{user}.{digest}")
}
