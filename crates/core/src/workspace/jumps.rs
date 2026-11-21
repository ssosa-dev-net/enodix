//! Jump hosts: the chain of hosts a host is reached through, from the
//! inherited `jump_host` setting. A jump host is another host of the same
//! workspace, resolved with the same rules, so it may have a jump host of
//! its own.

use super::{Folder, HostId, Origin, Protocol, ResolvedHost, Workspace};
use crate::names;

/// Why a host's jump hosts cannot be worked out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JumpChainError {
    /// `from` names a jump host the workspace does not have. `named` is
    /// the one host whose name (not id) is `jump_host`, as a newcomer writes
    /// it (#116): the message then gives its id.
    #[error("{}", missing(.from, .jump_host, .named.as_ref()))]
    Missing {
        from: HostId,
        jump_host: String,
        named: Option<HostId>,
    },
    /// Following the jump hosts comes back to a host already on the way:
    /// the hosts in order, the repeated one last.
    #[error("the jump hosts form a loop: {}", loop_path(.0))]
    Loop(Vec<HostId>),
    /// A jump host has to be reached over SSH to tunnel through it.
    #[error("the jump host {jump_host} is not reached over SSH")]
    NotSsh { jump_host: HostId },
}

fn missing(from: &HostId, jump_host: &str, named: Option<&HostId>) -> String {
    match named {
        Some(id) => format!(
            "the jump host {jump_host} of {from} is named by its host name; name it by its id, {id}, as the Jump host setting lists it"
        ),
        None => format!(
            "the jump host {jump_host} of {from} is not in the workspace; choose one the Jump host setting lists"
        ),
    }
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
                named: self.host_named(reference),
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

    /// The id of the host `reference` names: exactly, else ignoring case
    /// and Unicode form.
    pub(crate) fn find_host_id(&self, reference: &str) -> Option<HostId> {
        let exact = HostId::new(reference);
        if self.host(&exact).is_some() {
            return Some(exact);
        }
        self.host_id_ignoring_case(reference)
    }

    /// The id of the one host whose name is `name` (ignoring case and
    /// Unicode form); `None` when no host or several have it.
    fn host_named(&self, name: &str) -> Option<HostId> {
        fn collect(folder: &Folder, folded: &str, found: &mut Vec<HostId>) {
            found.extend(
                folder
                    .hosts
                    .iter()
                    .filter(|host| names::folded(&host.name) == folded)
                    .map(|host| host.id.clone()),
            );
            for child in &folder.folders {
                collect(child, folded, found);
            }
        }
        let mut found = Vec::new();
        collect(&self.tree, &names::folded(name), &mut found);
        if found.len() == 1 {
            found.pop()
        } else {
            None
        }
    }
}
