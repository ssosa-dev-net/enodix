//! How a session to a workspace host was asked for, kept so that
//! reconnecting it works its target out again (#130): the host's settings
//! and credential profile as they are at the reconnect, as a new connection
//! from the tree would use, rather than those of the first attempt.
//!
//! The rule, by what the session was opened with:
//!
//! - **The host's profile**: the profile the host has now. If it has none
//!   now, or names one that is not set up here, the session asks for the
//!   username and password while connecting ([`SshAuth::Prompt`]), and the
//!   note says why; it never logs in with anything else unasked.
//! - **A typed login** (no profile, one not set up, or the password fall
//!   back after the profile's SSH agent failed): kept, with the host's
//!   settings of now, unless the host has a usable profile now that it did
//!   not have when the login was typed (a new or another profile, or the
//!   one set up meanwhile). Then the profile logs in, and the note says so;
//!   the typed login is dropped.
//! - **Telnet or serial**: the host's telnet or serial target of now.
//!
//! In every case: a host gone from the workspace (deleted, moved outside
//! the app, its file unreadable), with no address, or whose protocol setting
//! changed to another than the session's transport is refused with a reason
//! ([`ReconnectRefused`]); nothing is tried. A host renamed or moved in the
//! app keeps its sessions ([`super::SessionManager::move_hosts`]). A session
//! of a workspace that is not open reconnects as it was made
//! ([`Reconnection::AsMade`]), and a quick connection, which belongs to no
//! host, has no origin and always does.

use std::fmt;
use std::path::{Path, PathBuf};

use super::{SessionTarget, SshAuth, Transport};
use crate::credentials::{ProfileError, Profiles};
use crate::job::SessionLogin;
use crate::workspace::{HostId, HostMove, Protocol, ResolvedHost};

/// A username and password the user typed for a session. `Debug` leaves
/// the password out.
#[derive(Clone, PartialEq, Eq)]
pub struct TypedLogin {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for TypedLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedLogin")
            .field("username", &self.username)
            .field("password", &"..")
            .finish()
    }
}

/// How a session to a workspace host is asked for: the ways in of the
/// tree and the side panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostWay {
    /// Over SSH: with the host's credential profile (`typed` = `None`), or
    /// with the login the user typed.
    Ssh { typed: Option<TypedLogin> },
    /// By telnet, whatever the host's protocol setting.
    Telnet,
    /// On the host's serial console, whatever its protocol setting.
    Serial,
}

/// What a session to a workspace host was opened with: what reconnecting it
/// works the target out from. Kept by the session manager with the session
/// until it is discarded (the typed login with it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOrigin {
    /// The root of the host's workspace (host ids mean something only
    /// there).
    workspace: PathBuf,
    host: HostId,
    /// The host's name then, for saying it is gone.
    name: String,
    /// The host's protocol setting then: a session opened over another
    /// transport on purpose (SSH to a telnet host) stays so while the
    /// setting does not change.
    protocol: Protocol,
    login: OriginLogin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OriginLogin {
    /// SSH with the profile `name`.
    Profile {
        name: String,
    },
    /// SSH with a typed login; `beside`: the usable profile the host had
    /// when it was typed (the password fall back), if any.
    Typed {
        login: TypedLogin,
        beside: Option<String>,
    },
    Telnet,
    Serial,
}

impl HostOrigin {
    pub fn host(&self) -> &HostId {
        &self.host
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The transport the session uses.
    pub fn transport(&self) -> Transport {
        match self.login {
            OriginLogin::Profile { .. } | OriginLogin::Typed { .. } => Transport::Ssh,
            OriginLogin::Telnet => Transport::Telnet,
            OriginLogin::Serial => Transport::Serial,
        }
    }

    /// The host has moved to `moves`' new id, if it is among them (in the
    /// workspace at `root`); whether it was.
    pub(super) fn follow(&mut self, root: &Path, moves: &[HostMove]) -> bool {
        if self.workspace != root {
            return false;
        }
        match moves.iter().find(|moved| moved.from == self.host) {
            Some(moved) => {
                self.host = moved.to.clone();
                true
            }
            None => false,
        }
    }
}

/// A session to a workspace host as worked out: what to connect to, and
/// what to keep for the next reconnect.
#[derive(Debug, Clone)]
pub struct HostSession {
    pub target: SessionTarget,
    pub origin: HostOrigin,
    /// How it logs in (what a backup on login may do).
    pub login: SessionLogin,
    /// For a reconnect whose login is not the one the session had: what the
    /// user is told (the profile it logs in with now, or why it asks).
    pub note: Option<String>,
}

/// What reconnecting a session to a workspace host does.
// Made once per reconnect and taken apart at once: not worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Reconnection {
    /// Connects to the target worked out again.
    Again(HostSession),
    /// The host's workspace is not open, so its settings are not at hand:
    /// reconnects as the session was made ([`super::SessionManager::reconnect`]),
    /// and `note` tells the user.
    AsMade { note: String },
}

/// Why a session to a workspace host cannot be reconnected now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconnectRefused {
    /// The host is not in the workspace any more.
    #[error("{name} is no longer in the workspace")]
    Gone { name: String },
    /// The host has no address to connect to.
    #[error("{name} has no address now")]
    NoAddress { name: String },
    /// The host's protocol setting changed to another than the session's.
    #[error("{name} is set to connect over {} now, and this is {} session", .now.as_str(), transport_name(.transport))]
    ProtocolChanged {
        name: String,
        now: Protocol,
        transport: Transport,
    },
    /// The host's settings do not give a target (a broken jump host chain,
    /// no serial port, the profiles cannot be read, ...).
    #[error("{name} cannot be connected to now: {reason}")]
    Unusable { name: String, reason: ProfileError },
}

fn transport_name(transport: &Transport) -> &'static str {
    match transport {
        Transport::Ssh => "an SSH",
        Transport::Telnet => "a telnet",
        Transport::Serial => "a serial",
        Transport::Local => "a local",
    }
}

impl ReconnectRefused {
    /// What the user can do about it.
    pub fn advice(&self) -> Option<String> {
        Some(match self {
            Self::Gone { .. } => "It was deleted, or moved or renamed outside the app, or its \
                file cannot be read now (see the workspace's problems). Connect to it again \
                from the Hosts view."
                .into(),
            Self::NoAddress { .. } => "Set its address in its details, then press Enter.".into(),
            Self::ProtocolChanged { .. } => "Connect to it again from the Hosts view.".into(),
            Self::Unusable { .. } => return None,
        })
    }
}

impl Reconnection {
    /// What reconnecting the session that `origin` opened does, with the
    /// profiles of the workspace open now (`None`: none is). See the module
    /// documentation for the rule.
    pub fn work_out(
        origin: &HostOrigin,
        open: Option<&Profiles<'_>>,
    ) -> Result<Self, ReconnectRefused> {
        let Some(profiles) =
            open.filter(|profiles| profiles.workspace().root() == origin.workspace)
        else {
            return Ok(Self::AsMade {
                note: format!(
                    "{} is a host of a workspace that is not open: reconnecting with the \
                     settings and login it was opened with.",
                    origin.name
                ),
            });
        };
        let Some(host) = profiles.workspace().resolve_host(&origin.host) else {
            return Err(ReconnectRefused::Gone {
                name: origin.name.clone(),
            });
        };
        let name = host.host.name.clone();
        let transport = origin.transport();
        let now = host.settings.protocol.value;
        if now != origin.protocol && !carries(now, transport) {
            return Err(ReconnectRefused::ProtocolChanged {
                name,
                now,
                transport,
            });
        }
        if transport != Transport::Serial && host.host.address.trim().is_empty() {
            return Err(ReconnectRefused::NoAddress { name });
        }
        let unusable = |reason| ReconnectRefused::Unusable {
            name: name.clone(),
            reason,
        };
        let id = &origin.host;
        let session = match &origin.login {
            OriginLogin::Telnet => profiles
                .host_session(id, HostWay::Telnet)
                .map_err(unusable)?,
            OriginLogin::Serial => profiles
                .host_session(id, HostWay::Serial)
                .map_err(unusable)?,
            OriginLogin::Profile { name: then } => {
                match profiles.usable_profile(&host).map_err(unusable)? {
                    Some(profile) => {
                        let mut session = profiles
                            .host_session(id, HostWay::Ssh { typed: None })
                            .map_err(unusable)?;
                        if !profile.eq_ignore_ascii_case(then) {
                            session.note = Some(profile_now(&profile, &name));
                        }
                        session
                    }
                    None => {
                        let note = match &host.settings.credential_profile {
                            Some(named) => format!(
                                "The credential profile {} is not set up on this computer: type \
                                 the username and password asked for, or set the profile up in \
                                 the Credentials view.",
                                named.value
                            ),
                            None => format!(
                                "{name} has no credential profile now: type the username and \
                                 password asked for."
                            ),
                        };
                        let mut session = profiles.asked_session(&host).map_err(unusable)?;
                        // Kept, so that the profile is used once it is back.
                        session.origin.login = origin.login.clone();
                        session.note = Some(note);
                        session
                    }
                }
            }
            OriginLogin::Typed { login, beside } => {
                // A profile that cannot be read leaves the typed login.
                let usable = profiles.usable_profile(&host).ok().flatten();
                match usable.filter(|profile| {
                    beside
                        .as_ref()
                        .is_none_or(|beside| !beside.eq_ignore_ascii_case(profile))
                }) {
                    Some(profile) => {
                        let mut session = profiles
                            .host_session(id, HostWay::Ssh { typed: None })
                            .map_err(unusable)?;
                        session.note = Some(profile_now(&profile, &name));
                        session
                    }
                    None => {
                        let mut session = profiles
                            .host_session(
                                id,
                                HostWay::Ssh {
                                    typed: Some(login.clone()),
                                },
                            )
                            .map_err(unusable)?;
                        // What it was typed beside stays what decides.
                        session.origin.login = origin.login.clone();
                        session
                    }
                }
            }
        };
        Ok(Self::Again(session))
    }
}

/// Whether a host set to `protocol` is reached over `transport`.
fn carries(protocol: Protocol, transport: Transport) -> bool {
    matches!(
        (protocol, transport),
        (Protocol::Ssh, Transport::Ssh)
            | (Protocol::Telnet, Transport::Telnet)
            | (Protocol::Serial, Transport::Serial)
    )
}

fn profile_now(profile: &str, host: &str) -> String {
    format!("Logging in with the credential profile {profile}, which {host} has now.")
}

impl Profiles<'_> {
    /// The session to the host `id` reached `way`, worked out from its
    /// settings and profile as they are now: the target (jump hosts
    /// included, through [`Self::profile_target`] / [`Self::prompted_target`]),
    /// and its origin, for [`super::SessionManager::open_host`]. An error
    /// says why the host cannot be reached that way.
    pub fn host_session(&self, id: &HostId, way: HostWay) -> Result<HostSession, ProfileError> {
        let host = self
            .workspace()
            .resolve_host(id)
            .ok_or_else(|| ProfileError::NotFound(id.to_string()))?;
        let (target, login, session_login) = match way {
            HostWay::Ssh { typed: None } => {
                let target = self.profile_target(id)?;
                let name = self.usable_profile(&host)?.ok_or_else(|| {
                    ProfileError::Invalid(format!("{id} has no credential profile"))
                })?;
                (
                    SessionTarget::Ssh(target),
                    OriginLogin::Profile { name },
                    SessionLogin::Profile,
                )
            }
            HostWay::Ssh { typed: Some(login) } => {
                let target =
                    self.prompted_target(id, login.username.clone(), login.password.clone())?;
                // The profile cannot be read: as if there were none.
                let beside = self.usable_profile(&host).ok().flatten();
                (
                    SessionTarget::Ssh(target),
                    OriginLogin::Typed { login, beside },
                    SessionLogin::Typed,
                )
            }
            HostWay::Telnet => (
                SessionTarget::Telnet(self.telnet_target(id)?),
                OriginLogin::Telnet,
                SessionLogin::Typed,
            ),
            HostWay::Serial => (
                SessionTarget::Serial(self.serial_target(id)?),
                OriginLogin::Serial,
                SessionLogin::Typed,
            ),
        };
        Ok(HostSession {
            target,
            origin: self.origin(&host, login),
            login: session_login,
            note: None,
        })
    }

    /// An SSH session to `host` that asks for the username and password
    /// while connecting, jump hosts included.
    fn asked_session(&self, host: &ResolvedHost) -> Result<HostSession, ProfileError> {
        let id = &host.host.id;
        let mut target = host.ssh_target_with(String::new(), SshAuth::Prompt);
        target.jump_hosts = self.jump_hosts(id)?;
        Ok(HostSession {
            target: SessionTarget::Ssh(target),
            // Replaced by the caller.
            origin: self.origin(
                host,
                OriginLogin::Profile {
                    name: String::new(),
                },
            ),
            login: SessionLogin::Typed,
            note: None,
        })
    }

    /// The name (as stored) of the profile `host` logs in with, if it names
    /// one that is set up here.
    fn usable_profile(&self, host: &ResolvedHost) -> Result<Option<String>, ProfileError> {
        let Some(named) = host.settings.credential_profile.as_ref() else {
            return Ok(None);
        };
        Ok(self.profile(&named.value)?.map(|profile| profile.name))
    }

    fn origin(&self, host: &ResolvedHost, login: OriginLogin) -> HostOrigin {
        HostOrigin {
            workspace: self.workspace().root().to_path_buf(),
            host: host.host.id.clone(),
            name: host.host.name.clone(),
            protocol: host.settings.protocol.value,
            login,
        }
    }
}
