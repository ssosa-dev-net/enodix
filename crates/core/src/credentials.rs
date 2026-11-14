//! Credential profiles: named logins (a username; a password, a private
//! key file with an optional passphrase, or the SSH agent; and an optional
//! enable secret)
//! that hosts use through the inherited `credential_profile` setting.
//!
//! Profiles live in a [`CredentialStore`], never in the workspace: the
//! workspace files only name a profile, so a shared workspace lets each
//! person keep their own secret under the same name. A store keeps
//! profiles per workspace ([`Workspace::credential_scope`]) and matches
//! names ignoring case. [`KeychainStore`] is the operating system's
//! keychain (Windows Credential Manager); [`MemoryStore`] keeps them in
//! memory, for tests.
//!
//! [`Profiles`] is what the app works with: it lists, saves and deletes
//! the profiles of one workspace, and tells how a host logs in
//! ([`Profiles::login`]).
//!
//! No `Debug` output here shows a secret.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::session::{JumpHost, SerialTarget, SshAgent, SshAuth, SshTarget, TelnetTarget};
use crate::workspace::{Folder, HostId, JumpChainError, Protocol, ResolvedHost, Workspace};

mod keychain;

pub use keychain::KeychainStore;

/// The longest profile name accepted, in characters.
pub const MAX_PROFILE_NAME: usize = 100;

/// A named login, secrets included.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialProfile {
    pub name: String,
    pub username: String,
    pub auth: ProfileAuth,
    /// Kept for devices that ask for one; nothing sends it automatically
    /// yet.
    pub enable_secret: Option<String>,
}

/// How a profile logs in.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileAuth {
    Password {
        password: String,
    },
    /// The key stays in its file; the profile only refers to it.
    PrivateKey {
        key_file: PathBuf,
        passphrase: Option<String>,
    },
    /// The keys of the SSH agent this computer runs
    /// ([`SshAgent::System`]); no secret is stored.
    Agent,
}

impl ProfileAuth {
    fn ssh_auth(&self) -> SshAuth {
        match self {
            Self::Password { password } => SshAuth::Password(password.clone()),
            Self::PrivateKey {
                key_file,
                passphrase,
            } => SshAuth::PrivateKey {
                key_file: key_file.clone(),
                passphrase: passphrase.clone(),
            },
            Self::Agent => SshAuth::Agent(SshAgent::System),
        }
    }
}

const HIDDEN: &str = "..";

impl fmt::Debug for CredentialProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialProfile")
            .field("name", &self.name)
            .field("username", &self.username)
            .field("auth", &self.auth)
            .field(
                "enable_secret",
                &self.enable_secret.as_ref().map(|_| HIDDEN),
            )
            .finish()
    }
}

impl fmt::Debug for ProfileAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password { .. } => f.write_str("Password(..)"),
            Self::PrivateKey {
                key_file,
                passphrase,
            } => f
                .debug_struct("PrivateKey")
                .field("key_file", key_file)
                .field("passphrase", &passphrase.as_ref().map(|_| HIDDEN))
                .finish(),
            Self::Agent => f.write_str("Agent"),
        }
    }
}

/// Where profiles are kept: per scope (one per workspace), by name,
/// ignoring case.
pub trait CredentialStore: Send + Sync {
    /// Every profile kept for `scope`, in no particular order.
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError>;
    /// The profile called `name` (ignoring case), if there is one.
    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError>;
    /// Keeps `profile`, replacing the one of the same name (ignoring case).
    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError>;
    /// Removes the profile called `name` (ignoring case); `false` if there
    /// was none.
    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError>;
}

/// The store could not be read or written. The message never holds a
/// secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(pub String);

/// A store that keeps profiles in memory until it is dropped.
#[derive(Default)]
pub struct MemoryStore {
    /// By scope, then by lower-case name.
    profiles: Mutex<BTreeMap<(String, String), CredentialProfile>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn profiles(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), CredentialProfile>> {
        self.profiles.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn key(scope: &str, name: &str) -> (String, String) {
    (scope.to_owned(), name.to_lowercase())
}

impl CredentialStore for MemoryStore {
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        Ok(self
            .profiles()
            .iter()
            .filter(|((s, _), _)| s == scope)
            .map(|(_, profile)| profile.clone())
            .collect())
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        Ok(self.profiles().get(&key(scope, name)).cloned())
    }

    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
        self.profiles()
            .insert(key(scope, &profile.name), profile.clone());
        Ok(())
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        Ok(self.profiles().remove(&key(scope, name)).is_some())
    }
}

/// A profile as the user enters it to create or change one. A secret left
/// as `None` keeps the one the profile already has.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileDraft {
    pub name: String,
    pub username: String,
    pub auth: AuthDraft,
    /// `None` keeps it; `Some("")` removes it.
    pub enable_secret: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthDraft {
    /// `None` keeps the stored password (only a password profile has one).
    Password {
        password: Option<String>,
    },
    /// `passphrase`: `None` keeps it (from a key profile), `Some("")`
    /// means the key has none.
    PrivateKey {
        key_file: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
}

impl fmt::Debug for ProfileDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileDraft")
            .field("name", &self.name)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// What the interface shows of a profile: everything but the secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileSummary {
    pub name: String,
    pub username: String,
    pub auth: AuthSummary,
    pub has_enable_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSummary {
    Password,
    PrivateKey {
        key_file: PathBuf,
        has_passphrase: bool,
    },
    Agent,
}

impl From<&CredentialProfile> for ProfileSummary {
    fn from(profile: &CredentialProfile) -> Self {
        Self {
            name: profile.name.clone(),
            username: profile.username.clone(),
            auth: match &profile.auth {
                ProfileAuth::Password { .. } => AuthSummary::Password,
                ProfileAuth::PrivateKey {
                    key_file,
                    passphrase,
                } => AuthSummary::PrivateKey {
                    key_file: key_file.clone(),
                    has_passphrase: passphrase.is_some(),
                },
                ProfileAuth::Agent => AuthSummary::Agent,
            },
            has_enable_secret: profile.enable_secret.is_some(),
        }
    }
}

/// How a host logs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostLogin {
    /// With its profile, without asking.
    Profile {
        /// The profile's name as stored.
        profile: String,
        target: SshTarget,
    },
    /// The host names a profile this store has nothing for: the user has
    /// to set it up (or log in by hand).
    NotSetUp { profile: String },
    /// No profile: ask for a username and password.
    Prompt,
    /// Over telnet: the device asks for the login in the terminal, so
    /// nothing is asked before connecting. Connect with
    /// [`Profiles::telnet_target`].
    Telnet,
    /// Over its serial console: like telnet, the device asks for any login
    /// in the terminal. Connect with [`Profiles::serial_target`].
    Serial,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("{0} not found")]
    NotFound(String),
    #[error("a profile called {0} already exists")]
    AlreadyExists(String),
    #[error("the credential profile {0} is not set up on this computer")]
    NotSetUp(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the credential store failed: {0}")]
    Store(#[from] StoreError),
    /// The host's jump hosts cannot be worked out (missing, a loop, ...).
    #[error("{0}")]
    JumpChain(#[from] JumpChainError),
    /// The host is reached through a jump host, which telnet cannot use.
    #[error("{host} is reached through the jump host {jump_host}, and telnet sessions cannot go through jump hosts")]
    TelnetThroughJumpHost { host: HostId, jump_host: HostId },
    /// The host's settings name no serial port to open.
    #[error("{0} has no serial port set; choose one in its serial line settings")]
    NoSerialPort(HostId),
}

/// The credential profiles of one workspace.
pub struct Profiles<'a> {
    store: &'a dyn CredentialStore,
    workspace: &'a Workspace,
    scope: String,
}

impl<'a> Profiles<'a> {
    pub fn new(store: &'a dyn CredentialStore, workspace: &'a Workspace) -> Self {
        Self {
            store,
            workspace,
            scope: workspace.credential_scope(),
        }
    }

    /// The stored profiles, by name ignoring case.
    pub fn list(&self) -> Result<Vec<ProfileSummary>, ProfileError> {
        let mut profiles = self.store.list(&self.scope)?;
        profiles.sort_by_cached_key(|profile| (profile.name.to_lowercase(), profile.name.clone()));
        Ok(profiles.iter().map(ProfileSummary::from).collect())
    }

    /// The profile called `name` (ignoring case), secrets included.
    pub fn profile(&self, name: &str) -> Result<Option<CredentialProfile>, ProfileError> {
        Ok(self.store.get(&self.scope, name.trim())?)
    }

    /// The profile names the workspace uses (on any folder or host) that
    /// have nothing stored, by name ignoring case, each once.
    pub fn missing(&self) -> Result<Vec<String>, ProfileError> {
        let stored: BTreeSet<String> = self
            .store
            .list(&self.scope)?
            .into_iter()
            .map(|profile| profile.name.to_lowercase())
            .collect();
        let mut named = BTreeMap::new();
        collect_profile_names(self.workspace.tree(), &mut named);
        Ok(named
            .into_iter()
            .filter(|(lower, _)| !stored.contains(lower))
            .map(|(_, name)| name)
            .collect())
    }

    /// Creates a profile (`previous` = `None`) or changes the one called
    /// `previous`, renaming it if the draft has another name. Names and
    /// usernames are trimmed. Nothing is stored if the draft is refused.
    pub fn save(
        &self,
        previous: Option<&str>,
        draft: &ProfileDraft,
    ) -> Result<ProfileSummary, ProfileError> {
        let name = draft.name.trim();
        validate_name(name)?;
        let username = draft.username.trim();
        if username.is_empty() {
            return Err(ProfileError::Invalid("a username is needed".into()));
        }
        let current =
            match previous {
                Some(previous) => Some(self.profile(previous)?.ok_or_else(|| {
                    ProfileError::NotFound(format!("profile {}", previous.trim()))
                })?),
                None => None,
            };
        let renamed = current
            .as_ref()
            .is_none_or(|current| current.name.to_lowercase() != name.to_lowercase());
        if renamed && self.store.get(&self.scope, name)?.is_some() {
            return Err(ProfileError::AlreadyExists(name.into()));
        }

        let profile = CredentialProfile {
            name: name.into(),
            username: username.into(),
            auth: auth(&draft.auth, current.as_ref().map(|c| &c.auth))?,
            enable_secret: match &draft.enable_secret {
                None => current.as_ref().and_then(|c| c.enable_secret.clone()),
                Some(secret) if secret.is_empty() => None,
                Some(secret) => Some(secret.clone()),
            },
        };
        // The new one first, so that a failure loses nothing.
        self.store.put(&self.scope, &profile)?;
        if let Some(current) = current.filter(|_| renamed) {
            self.store
                .delete(&self.scope, &current.name)
                .map_err(|error| {
                    StoreError(format!(
                        "saved as {name}, but {} could not be removed: {error}",
                        current.name
                    ))
                })?;
        }
        Ok(ProfileSummary::from(&profile))
    }

    /// Deletes the profile called `name` (ignoring case).
    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        if self.store.delete(&self.scope, name.trim())? {
            Ok(())
        } else {
            Err(ProfileError::NotFound(format!("profile {}", name.trim())))
        }
    }

    /// How the host `id` logs in over the protocol its settings resolve
    /// to: see [`Self::login_with`].
    pub fn login(&self, id: &HostId) -> Result<HostLogin, ProfileError> {
        let host = self.resolve(id)?;
        self.login_over(&host, host.settings.protocol.value)
    }

    /// How the host `id` logs in when reached over `protocol`: over telnet
    /// or serial in the terminal ([`HostLogin::Telnet`],
    /// [`HostLogin::Serial`]); over SSH with the profile its settings
    /// resolve to, if that profile is stored.
    /// The target in [`HostLogin::Profile`] is the host alone, without its
    /// jump hosts: connect with [`Self::profile_target`].
    pub fn login_with(&self, id: &HostId, protocol: Protocol) -> Result<HostLogin, ProfileError> {
        let host = self.resolve(id)?;
        self.login_over(&host, protocol)
    }

    fn login_over(
        &self,
        host: &ResolvedHost,
        protocol: Protocol,
    ) -> Result<HostLogin, ProfileError> {
        match protocol {
            Protocol::Telnet => return Ok(HostLogin::Telnet),
            Protocol::Serial => return Ok(HostLogin::Serial),
            Protocol::Ssh => {}
        }
        let Some(name) = host.settings.credential_profile.as_ref() else {
            return Ok(HostLogin::Prompt);
        };
        Ok(match self.profile(&name.value)? {
            Some(profile) => HostLogin::Profile {
                target: host.ssh_target_with(profile.username.clone(), profile.auth.ssh_auth()),
                profile: profile.name,
            },
            None => HostLogin::NotSetUp {
                profile: name.value.clone(),
            },
        })
    }

    /// The target the host `id` logs in with through its profile, jump
    /// hosts included, for connecting without a prompt; an error says why
    /// it cannot (no profile, or one that is not set up here, or a jump
    /// host that cannot be used).
    pub fn profile_target(&self, id: &HostId) -> Result<SshTarget, ProfileError> {
        let mut target = match self.login_with(id, Protocol::Ssh)? {
            HostLogin::Profile { target, .. } => target,
            HostLogin::NotSetUp { profile } => return Err(ProfileError::NotSetUp(profile)),
            HostLogin::Prompt | HostLogin::Telnet | HostLogin::Serial => {
                return Err(ProfileError::Invalid(format!(
                    "{id} has no credential profile"
                )))
            }
        };
        target.jump_hosts = self.jump_hosts(id)?;
        Ok(target)
    }

    /// The target the host `id` logs in with by `username` and `password`
    /// (as prompted for), jump hosts included.
    pub fn prompted_target(
        &self,
        id: &HostId,
        username: String,
        password: String,
    ) -> Result<SshTarget, ProfileError> {
        let host = self.resolve(id)?;
        let mut target = host.ssh_target(username, password);
        target.jump_hosts = self.jump_hosts(id)?;
        Ok(target)
    }

    /// The target for reaching the host `id` by telnet, whatever protocol
    /// its settings name (the side panel offers telnet for every host).
    /// Telnet does not go through jump hosts: a host that has one is
    /// refused rather than reached around it.
    pub fn telnet_target(&self, id: &HostId) -> Result<TelnetTarget, ProfileError> {
        let host = self.resolve(id)?;
        if let Some(jump) = self.workspace.jump_chain(&host)?.last() {
            return Err(ProfileError::TelnetThroughJumpHost {
                host: id.clone(),
                jump_host: jump.host.id.clone(),
            });
        }
        Ok(host.telnet_target())
    }

    /// The target for opening the host `id`'s serial console, whatever
    /// protocol its settings name: its serial port and line parameters,
    /// each its own or inherited. Jump hosts do not apply (the port is on
    /// this computer).
    pub fn serial_target(&self, id: &HostId) -> Result<SerialTarget, ProfileError> {
        self.resolve(id)?
            .serial_target()
            .ok_or_else(|| ProfileError::NoSerialPort(id.clone()))
    }

    /// The jump hosts the host `id` is reached through, in the order they
    /// are connected, each logging in with its own profile. A jump host
    /// without a usable profile (none, or one not set up here) asks for
    /// its username and password while connecting ([`SshAuth::Prompt`],
    /// empty username).
    pub fn jump_hosts(&self, id: &HostId) -> Result<Vec<JumpHost>, ProfileError> {
        let host = self.resolve(id)?;
        self.workspace
            .jump_chain(&host)?
            .into_iter()
            .map(|jump| {
                let name = jump.host.id.to_string();
                let profile = match jump.settings.credential_profile.as_ref() {
                    Some(profile) => self.profile(&profile.value)?,
                    None => None,
                };
                let (username, auth) = profile.map_or((String::new(), SshAuth::Prompt), |p| {
                    (p.username, p.auth.ssh_auth())
                });
                let target = jump.ssh_target_with(username, auth);
                Ok(JumpHost {
                    name,
                    host: target.host,
                    port: target.port,
                    username: target.username,
                    auth: target.auth,
                    legacy_crypto: target.legacy_crypto,
                    connect_timeout: target.connect_timeout,
                })
            })
            .collect()
    }

    fn resolve(&self, id: &HostId) -> Result<ResolvedHost, ProfileError> {
        self.workspace
            .resolve_host(id)
            .ok_or_else(|| ProfileError::NotFound(id.to_string()))
    }
}

fn validate_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty() {
        return Err(ProfileError::Invalid("a profile needs a name".into()));
    }
    if name.chars().any(char::is_control) {
        return Err(ProfileError::Invalid(
            "a profile name cannot hold control characters".into(),
        ));
    }
    if name.chars().count() > MAX_PROFILE_NAME {
        return Err(ProfileError::Invalid(format!(
            "a profile name can have at most {MAX_PROFILE_NAME} characters"
        )));
    }
    Ok(())
}

/// The login of a draft, taking the secrets it leaves out from `current`.
fn auth(draft: &AuthDraft, current: Option<&ProfileAuth>) -> Result<ProfileAuth, ProfileError> {
    match draft {
        AuthDraft::Password { password } => {
            let password = match (password, current) {
                (Some(password), _) => password.clone(),
                (None, Some(ProfileAuth::Password { password })) => password.clone(),
                (None, _) => return Err(ProfileError::Invalid("a password is needed".into())),
            };
            Ok(ProfileAuth::Password { password })
        }
        AuthDraft::PrivateKey {
            key_file,
            passphrase,
        } => {
            if key_file.as_os_str().is_empty() {
                return Err(ProfileError::Invalid("a private key file is needed".into()));
            }
            let passphrase = match (passphrase, current) {
                (Some(passphrase), _) if passphrase.is_empty() => None,
                (Some(passphrase), _) => Some(passphrase.clone()),
                (None, Some(ProfileAuth::PrivateKey { passphrase, .. })) => passphrase.clone(),
                (None, _) => None,
            };
            Ok(ProfileAuth::PrivateKey {
                key_file: key_file.clone(),
                passphrase,
            })
        }
        AuthDraft::Agent => Ok(ProfileAuth::Agent),
    }
}

/// Every `credential_profile` set in `folder` and below, by lower-case
/// name, keeping the first spelling met.
fn collect_profile_names(folder: &Folder, names: &mut BTreeMap<String, String>) {
    let own = folder.settings.credential_profile.iter().chain(
        folder
            .hosts
            .iter()
            .filter_map(|h| h.settings.credential_profile.as_ref()),
    );
    for name in own {
        names
            .entry(name.to_lowercase())
            .or_insert_with(|| name.clone());
    }
    for child in &folder.folders {
        collect_profile_names(child, names);
    }
}
