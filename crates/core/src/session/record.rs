//! What the app keeps of each session (#84): what a window needs to show
//! it, whichever window that is. A session moves between windows, and a
//! window that dies takes nothing with it, so this is kept by the app
//! rather than by the window that opened the session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use super::{SessionId, Transport};
use crate::paste::PasteSafety;
use crate::workspace::{HostId, HostMove, Role, Workspace};

/// A session's name, host and header facts, and its paste safety, as they
/// were when it opened.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    /// Its tab's label: the host's name, an address or a serial port.
    pub name: String,
    /// What its header shows, such as `user@host:port`.
    pub title: String,
    /// The host's address, when the title does not show it.
    pub address: Option<String>,
    /// The jump host an SSH session to the host goes through.
    pub jump_host: Option<String>,
    /// The workspace host it goes to; `None` for a quick connection.
    pub host: Option<HostId>,
    /// The root of that host's workspace (host ids mean something only in
    /// their own workspace).
    pub workspace: Option<PathBuf>,
    /// The host's environment and paste line delay.
    pub safety: PasteSafety,
    /// The host's role.
    pub role: Option<Role>,
}

impl SessionRecord {
    /// A session to no workspace host (a quick connection): nothing beyond
    /// its name and title.
    pub fn new(name: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            address: None,
            jump_host: None,
            host: None,
            workspace: None,
            safety: PasteSafety::default(),
            role: None,
        }
    }

    /// A session over `transport` to `workspace`'s host `id`, with what the
    /// workspace sets for it now; `None` if the host is not in it. Only SSH
    /// goes through a jump host, and never through the host itself.
    pub fn for_host(
        workspace: &Workspace,
        id: &HostId,
        transport: Transport,
        name: impl Into<String>,
        title: impl Into<String>,
    ) -> Option<Self> {
        let resolved = workspace.resolve_host(id)?;
        let title = title.into();
        let address = &resolved.host.address;
        let jump_host = match transport {
            Transport::Ssh => resolved
                .settings
                .jump_host
                .as_ref()
                .map(|jump| jump.value.clone())
                .filter(|jump| !jump.eq_ignore_ascii_case(id.as_str())),
            Transport::Telnet | Transport::Serial | Transport::Local => None,
        };
        Some(Self {
            name: name.into(),
            address: (!address.is_empty() && !title.contains(address.as_str()))
                .then(|| address.clone()),
            title,
            jump_host,
            host: Some(id.clone()),
            workspace: Some(workspace.root().to_path_buf()),
            safety: resolved.paste_safety(),
            role: workspace.host_role(id),
        })
    }

    /// Whether it goes to a host of the workspace at `root` (#88: what
    /// leaving that workspace asks about).
    pub fn of_workspace(&self, root: &std::path::Path) -> bool {
        self.workspace.as_deref() == Some(root)
    }

    /// The session's host, if `workspace` is the one it came from and the
    /// host is still in it: what its snippets are filled from (#86).
    pub fn host_in(&self, workspace: &Workspace) -> Option<&HostId> {
        let host = self.host.as_ref()?;
        (self.workspace.as_deref() == Some(workspace.root())
            && workspace.resolve_host(host).is_some())
        .then_some(host)
    }

    /// What applies to the session's pastes (#87): its host's paste safety
    /// as `workspace` sets it now, while that is the workspace it came from
    /// and has the host; otherwise what it opened with, so a host removed
    /// keeps its safeguard. How a window pastes into a group member
    /// wherever the member is.
    pub fn safety_in(&self, workspace: &Workspace) -> PasteSafety {
        self.host_in(workspace)
            .and_then(|host| workspace.resolve_host(host))
            .map_or(self.safety, |resolved| resolved.paste_safety())
    }
}

/// Every session's record, by session id, until it is forgotten.
#[derive(Debug, Default)]
pub struct SessionRecords(Mutex<HashMap<SessionId, SessionRecord>>);

impl SessionRecords {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, SessionRecord>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `record` for `session`, in place of any it had.
    pub fn record(&self, session: SessionId, record: SessionRecord) {
        self.lock().insert(session, record);
    }

    pub fn get(&self, session: SessionId) -> Option<SessionRecord> {
        self.lock().get(&session).cloned()
    }

    /// Gives `copy` the record of `original` (a duplicate shows what it
    /// showed); whether `original` had one.
    pub fn copy(&self, original: SessionId, copy: SessionId) -> bool {
        let mut records = self.lock();
        let Some(record) = records.get(&original).cloned() else {
            return false;
        };
        records.insert(copy, record);
        true
    }

    /// What applies to `session`'s pastes (#87): its record's
    /// [`SessionRecord::safety_in`] the open `workspace`, or what it opened
    /// with when none is open; none for a session without a record.
    pub fn paste_safety(&self, session: SessionId, workspace: Option<&Workspace>) -> PasteSafety {
        let Some(record) = self.get(session) else {
            return PasteSafety::default();
        };
        workspace.map_or(record.safety, |workspace| record.safety_in(workspace))
    }

    /// The hosts of `moves` in the workspace at `root` have new ids: the
    /// records of sessions to them name the new ones (so their snippets,
    /// pastes and history follow the host). Returns the sessions changed.
    pub fn move_hosts(&self, root: &std::path::Path, moves: &[HostMove]) -> Vec<SessionId> {
        let mut changed = Vec::new();
        for (session, record) in self.lock().iter_mut() {
            if !record.of_workspace(root) {
                continue;
            }
            let Some(host) = &record.host else { continue };
            if let Some(moved) = moves.iter().find(|moved| moved.from == *host) {
                record.host = Some(moved.to.clone());
                changed.push(*session);
            }
        }
        changed.sort_by_key(|session| session.0);
        changed
    }

    /// Forgets `session`'s record: the session was discarded.
    pub fn forget(&self, session: SessionId) {
        self.lock().remove(&session);
    }
}
