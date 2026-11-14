//! Jump hosts: the chain of hosts a host is reached through, from the
//! inherited `jump_host` setting. A jump host is another host of the same
//! workspace, resolved with the same rules, so it may have a jump host of
//! its own.

use super::{Folder, HostId, Origin, Protocol, ResolvedHost, Workspace};

/// Why a host's jump hosts cannot be worked out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JumpChainError {
    /// `from` names a jump host the workspace does not have.
    #[error("the jump host {jump_host} of {from} is not in the workspace")]
    Missing { from: HostId, jump_host: String },
    /// Following the jump hosts comes back to a host already on the way:
    /// the hosts in order, the repeated one last.
    #[error("the jump hosts form a loop: {}", loop_path(.0))]
    Loop(Vec<HostId>),
    /// A jump host has to be reached over SSH to tunnel through it.
    #[error("the jump host {jump_host} is not reached over SSH")]
    NotSsh { jump_host: HostId },
}

fn loop_path(ids: &[HostId]) -> String {
    ids.iter()
        .map(HostId::as_str)
        .collect::<Vec<_>>()
        .join(" → ")
}

impl Workspace {
    /// The jump hosts `host` is reached through, in the order they are
    /// connected: the first directly, each next one through the one before,
    /// and `host` through the last. Empty when it is reached directly.
    ///
    /// A host whose inherited jump host is itself (the bastion in the
    /// folder that names it) is reached directly; naming itself in its own
    /// file is a loop. References match host ids ignoring case.
    pub fn jump_chain(&self, host: &ResolvedHost) -> Result<Vec<ResolvedHost>, JumpChainError> {
        let mut chain: Vec<ResolvedHost> = Vec::new();
        let mut seen = vec![host.host.id.clone()];
        let mut current = host.clone();
        while let Some(jump) = current.settings.jump_host.clone() {
            let reference = jump.value.trim();
            let missing = || JumpChainError::Missing {
                from: current.host.id.clone(),
                jump_host: reference.to_owned(),
            };
            let id = self.find_host_id(reference).ok_or_else(missing)?;
            if id == current.host.id && jump.origin != Origin::Host {
                break;
            }
            let looped = seen.contains(&id);
            seen.push(id.clone());
            if looped {
                return Err(JumpChainError::Loop(seen));
            }
            let next = self.resolve_host(&id).ok_or_else(missing)?;
            if next.settings.protocol.value != Protocol::Ssh {
                return Err(JumpChainError::NotSsh { jump_host: id });
            }
            chain.push(next.clone());
            current = next;
        }
        chain.reverse();
        Ok(chain)
    }

    /// The id of the host `reference` names: exactly, else ignoring case.
    pub(crate) fn find_host_id(&self, reference: &str) -> Option<HostId> {
        let exact = HostId::new(reference);
        if self.host(&exact).is_some() {
            return Some(exact);
        }
        fn find(folder: &Folder, lower: &str) -> Option<HostId> {
            folder
                .hosts
                .iter()
                .find(|host| host.id.as_str().to_lowercase() == lower)
                .map(|host| host.id.clone())
                .or_else(|| folder.folders.iter().find_map(|child| find(child, lower)))
        }
        find(&self.tree, &reference.to_lowercase())
    }
}
