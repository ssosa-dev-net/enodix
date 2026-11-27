//! Agent forwarding the user approves (#26 follow-up): a workspace's
//! `agent_forwarding` setting only asks for it. A terminal session to a
//! workspace host whose settings ask forwards the SSH agent only once the
//! user has said so on this computer: for that tab (until it is closed),
//! or always for that host.
//!
//! "Always" is kept in the per-user application data folder, in
//! [`FILE_NAME`], never in a workspace, and covers only what the user saw
//! when answering: the workspace's identity, the host's id, and the address
//! and port connected to ([`AgentForwardingKey`]). A changed address,
//! another host, or the same files opened as another workspace ask again.
//! The file is read on every check, so an approval given in another window
//! (or removed by hand) counts at once. A file that cannot be read counts as
//! no approval: the session asks rather than forwarding.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

/// The approvals file's name inside the data folder.
pub const FILE_NAME: &str = "agent_forwarding.json";

/// What one approval covers: forwarding the agent to this address and port,
/// for this host of this workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentForwardingKey {
    /// The workspace's identity: its id (`_workspace.yaml`), or, for one
    /// without an id, `path:` and its root folder.
    pub workspace: String,
    /// The host's id in that workspace.
    pub host: String,
    /// The address and port the session connects to.
    pub address: String,
    pub port: u16,
}

/// The question a session asks before forwarding the agent to a host whose
/// settings ask for it; nothing is sent to the device until it is answered
/// ([`super::SessionManager::decide_agent_forwarding`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentForwardingQuestion {
    /// The host's name, as the workspace names it.
    pub name: String,
    /// The address and port the session connects to.
    pub host: String,
    pub port: u16,
}

/// The user's answer to an [`AgentForwardingQuestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentForwardingDecision {
    /// Connect without forwarding the agent: the default, and what closing
    /// the question means. Holds for the tab until it is closed.
    WithoutForwarding,
    /// Forward it for this tab: its reconnects too, until it is closed.
    ForwardThisSession,
    /// Forward it, and from now on to this host at this address and port
    /// without asking (kept in the data folder).
    AlwaysForward,
}

/// The hosts the user always forwards the agent to. Cloning gives another
/// handle to the same approvals.
#[derive(Clone)]
pub struct AgentApprovals {
    store: Arc<Mutex<Store>>,
}

enum Store {
    File(PathBuf),
    Memory(Vec<AgentForwardingKey>),
}

#[derive(Default, Serialize, Deserialize)]
struct Approvals {
    /// Each host the agent is always forwarded to.
    #[serde(default)]
    always: Vec<AgentForwardingKey>,
}

impl AgentApprovals {
    /// The approvals kept in `data_folder` (the per-user application data
    /// folder). The file is created when the first one is remembered.
    pub fn in_data_folder(data_folder: &Path) -> Self {
        Self::with(Store::File(data_folder.join(FILE_NAME)))
    }

    /// Approvals kept only as long as this value (and its clones) lives.
    pub fn in_memory() -> Self {
        Self::with(Store::Memory(Vec::new()))
    }

    fn with(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the user always forwards the agent for `key`.
    pub fn approved(&self, key: &AgentForwardingKey) -> bool {
        match &*self.lock() {
            Store::Memory(keys) => keys.contains(key),
            Store::File(file) => read(file).is_ok_and(|approvals| approvals.always.contains(key)),
        }
    }

    /// Remembers to always forward the agent for `key`.
    pub fn remember(&self, key: AgentForwardingKey) -> io::Result<()> {
        match &mut *self.lock() {
            Store::Memory(keys) => {
                if !keys.contains(&key) {
                    keys.push(key);
                }
                Ok(())
            }
            Store::File(file) => {
                let mut approvals = match read(file) {
                    Ok(approvals) => approvals,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Approvals::default(),
                    // Never overwrite a file that cannot be read.
                    Err(error) => return Err(error),
                };
                if !approvals.always.contains(&key) {
                    approvals.always.push(key);
                }
                let text = serde_json::to_string_pretty(&approvals).map_err(io::Error::other)?;
                write_atomically(file, &text)
            }
        }
    }
}

fn read(file: &Path) -> io::Result<Approvals> {
    let text = fs::read_to_string(file)?;
    serde_json::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Writes `text` beside `file`, then puts it in place, so that a crash
/// never leaves half a file. Creates the folder if needed.
fn write_atomically(file: &Path, text: &str) -> io::Result<()> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    let mut temporary = file.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text)?;
    fs::rename(&temporary, file)
}

/// What a tab answered, kept for its reconnects: the answer holds while the
/// key stays the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TabAnswer {
    key: AgentForwardingKey,
    forward: bool,
}

/// What a session to a workspace host whose settings ask for forwarding
/// needs to settle it before connecting.
pub(super) struct Gate {
    pub(super) key: AgentForwardingKey,
    pub(super) question: AgentForwardingQuestion,
    pub(super) approvals: AgentApprovals,
    /// The tab's answer, shared with the manager's entry.
    pub(super) answer: Arc<Mutex<Option<TabAnswer>>>,
}

impl Gate {
    /// Whether to forward the agent this connection: as remembered, as the
    /// tab answered, or as the user answers now (asked through `link`).
    /// `None`: the session was closed while asking.
    pub(super) async fn settle(self, link: &mut super::Link) -> Option<bool> {
        if self.approvals.approved(&self.key) {
            return Some(true);
        }
        let answered = self
            .answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(answer) = answered.filter(|answer| answer.key == self.key) {
            return Some(answer.forward);
        }
        let decision = link.ask_agent_forwarding(self.question).await?;
        let forward = match decision {
            AgentForwardingDecision::WithoutForwarding => false,
            AgentForwardingDecision::ForwardThisSession => true,
            AgentForwardingDecision::AlwaysForward => {
                // Not saved: it still forwards this session, and asks next
                // time.
                let _ = self.approvals.remember(self.key.clone());
                true
            }
        };
        *self.answer.lock().unwrap_or_else(PoisonError::into_inner) = Some(TabAnswer {
            key: self.key,
            forward,
        });
        Some(forward)
    }
}
