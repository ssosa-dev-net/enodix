//! SSH agents: reaching the one a target names and listing its keys.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::agent::AgentIdentity;
use russh::keys::PublicKey;

use super::{AgentError, SshAgent};

/// The Windows OpenSSH agent's named pipe.
const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// The variable naming the SSH agent to use, as OpenSSH reads it.
const SSH_AUTH_SOCK: &str = "SSH_AUTH_SOCK";

/// How long an agent may take to answer when asked for its keys.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) type Client = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// A running agent and the keys it holds.
pub(super) struct Agent {
    pub client: Client,
    pub keys: Vec<PublicKey>,
}

/// Reaches every agent `which` names that runs, with their keys. Fails if
/// none runs, or if none holds a key.
pub(super) async fn open(which: &SshAgent) -> Result<Vec<Agent>, AgentError> {
    let endpoints = endpoints(which);
    if endpoints.is_empty() {
        return Err(AgentError::NotRunning(format!(
            "{SSH_AUTH_SOCK} is not set"
        )));
    }
    let mut agents = Vec::new();
    let mut problems = Vec::new();
    for endpoint in endpoints {
        match tokio::time::timeout(LIST_TIMEOUT, endpoint.open()).await {
            Ok(Ok(agent)) => agents.push(agent),
            Ok(Err(problem)) => problems.push(format!("{endpoint}: {problem}")),
            Err(_) => problems.push(format!("{endpoint}: did not answer")),
        }
    }
    if agents.is_empty() {
        return Err(AgentError::NotRunning(problems.join("; ")));
    }
    if agents.iter().all(|agent| agent.keys.is_empty()) {
        return Err(AgentError::NoKeys);
    }
    Ok(agents)
}

/// Where an agent listens.
enum Endpoint {
    /// A named pipe (Windows) or Unix socket.
    Socket(PathBuf),
    Pageant,
}

/// The agents to try for `which`, in order.
fn endpoints(which: &SshAgent) -> Vec<Endpoint> {
    match which {
        SshAgent::At(path) => vec![Endpoint::Socket(path.clone())],
        SshAgent::Pageant => vec![Endpoint::Pageant],
        // As OpenSSH does: the variable wins; on Windows the OpenSSH
        // agent's pipe is the default, and PuTTY users run Pageant. On
        // Windows only a named pipe counts: shells such as Git Bash set the
        // variable to a Unix socket path that native programs cannot use.
        SshAgent::System => match std::env::var_os(SSH_AUTH_SOCK)
            .filter(|v| !v.is_empty() && (!cfg!(windows) || is_named_pipe(v)))
        {
            Some(path) => vec![Endpoint::Socket(path.into())],
            None if cfg!(windows) => vec![
                Endpoint::Socket(OPENSSH_AGENT_PIPE.into()),
                Endpoint::Pageant,
            ],
            None => Vec::new(),
        },
    }
}

/// Whether `path` names a Windows named pipe (`\\.\pipe\...`, with either
/// slash).
fn is_named_pipe(path: &std::ffi::OsStr) -> bool {
    let path = path.to_string_lossy().replace('/', r"\");
    path.to_ascii_lowercase().starts_with(r"\\.\pipe\")
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(path) if path.as_os_str() == OPENSSH_AGENT_PIPE => {
                f.write_str("the Windows OpenSSH agent")
            }
            Self::Socket(path) => write!(f, "the agent at {}", path.display()),
            Self::Pageant => f.write_str("Pageant"),
        }
    }
}

impl Endpoint {
    /// Connects and lists the keys; the error says what went wrong.
    async fn open(&self) -> Result<Agent, String> {
        let mut client = match self {
            Self::Socket(path) => connect_socket(path).await,
            Self::Pageant => connect_pageant().await,
        }
        .map_err(describe)?;
        let identities = client
            .request_identities()
            .await
            .map_err(|error| match self {
                // Without Pageant, its client only fails once asked something.
                Self::Pageant => "not running".to_owned(),
                Self::Socket(_) => describe(error),
            })?;
        let keys = identities
            .into_iter()
            .filter_map(|identity| match identity {
                AgentIdentity::PublicKey { key, .. } => Some(key),
                // Certificates are not supported yet.
                AgentIdentity::Certificate { .. } => None,
            })
            .collect();
        Ok(Agent { client, keys })
    }
}

fn describe(error: russh::keys::Error) -> String {
    match error {
        russh::keys::Error::IO(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "not running".to_owned()
        }
        error => error.to_string(),
    }
}

#[cfg(windows)]
async fn connect_socket(path: &std::path::Path) -> Result<Client, russh::keys::Error> {
    Ok(AgentClient::connect_named_pipe(path).await?.dynamic())
}

#[cfg(unix)]
async fn connect_socket(path: &std::path::Path) -> Result<Client, russh::keys::Error> {
    Ok(AgentClient::connect_uds(path).await?.dynamic())
}

#[cfg(windows)]
async fn connect_pageant() -> Result<Client, russh::keys::Error> {
    Ok(AgentClient::connect_pageant().await?.dynamic())
}

#[cfg(not(windows))]
async fn connect_pageant() -> Result<Client, russh::keys::Error> {
    Err(russh::keys::Error::IO(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Pageant runs only on Windows",
    )))
}
