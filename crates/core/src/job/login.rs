//! Back up on login: each SSH session to a host whose `backup_on_login`
//! setting is on saves the host's configuration once connected, so that
//! the state before the user's changes is always in the history.
//!
//! The backup is a [`WhenConnected`] given to the session when it is opened
//! ([`crate::session::OpenOptions::when_connected`]). Each time the session
//! connects (reconnects included), it runs the config-backup step of a job
//! ([`super::JobStep::ConfigBackup`]: privileged mode, no paging, the
//! show-config command) on a second shell channel on the session's own
//! connection: no second login, and nothing in the terminal. If the device
//! refuses a second channel and the session logged in with a stored
//! credential profile, a second, headless connection with that profile is
//! used instead (asking nothing, as a job); a typed or prompted login gives
//! up, as a second connection would have to ask for it again. The outcome
//! reaches the session's owner as [`SessionEvent::Backup`]; the terminal
//! never waits for it.
//!
//! [`SessionEvent::Backup`]: crate::session::SessionEvent::Backup

use std::time::Duration;

use serde::Serialize;

use super::{given, BackupWork, EnableSecret, HostWork, PlannedHost, DEFAULT_COMMAND_TIMEOUT};
use crate::configs::{ConfigStore, SaveOutcome};
use crate::credentials::{HostLogin, Profiles};
use crate::session::{ConnectedSession, SshAuth, WhenConnected};
use crate::workspace::{HostId, Protocol, Workspace};

/// How a session to a host logged in, which decides whether a device that
/// refuses a second channel gets a second connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLogin {
    /// With the host's stored credential profile: a second connection can
    /// log in with it too, asking nothing.
    Profile,
    /// With a login the user typed or was asked for: a second connection
    /// would have to ask again, so there is none.
    Typed,
}

/// How a backup on login ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LoginBackupOutcome {
    /// The configuration changed (or is the host's first): kept as
    /// `version`.
    Saved { version: String },
    /// The configuration is the same as the latest version, `version`.
    Unchanged { version: String },
    /// No configuration was kept, for `reason`.
    Failed { reason: String },
}

impl From<SaveOutcome> for LoginBackupOutcome {
    fn from(outcome: SaveOutcome) -> Self {
        match outcome {
            SaveOutcome::Saved { version } => Self::Saved { version },
            SaveOutcome::Unchanged { version } => Self::Unchanged { version },
        }
    }
}

/// The backup on login of one host, ready to give a session.
#[derive(Debug, Clone)]
pub struct LoginBackup {
    host: PlannedHost,
    store: ConfigStore,
    /// The workspace id the history is kept under.
    workspace: String,
    command_timeout: Duration,
}

impl LoginBackup {
    /// The backup a session to the host `id`, logged in as `login`, makes
    /// once connected, keeping configurations in `store`; `None` when none
    /// applies: the host's `backup_on_login` is off, it is not in the
    /// workspace, its protocol is not SSH, no usable platform definition
    /// with a show-config command applies to it, or the workspace has no
    /// id. The enable secret of the host's stored profile (if any) answers
    /// the privilege command's question, whatever the login.
    pub fn plan(
        workspace: &Workspace,
        profiles: &Profiles<'_>,
        id: &HostId,
        login: SessionLogin,
        store: ConfigStore,
    ) -> Option<Self> {
        let host = workspace.resolve_host(id)?;
        if !host.settings.backup_on_login.value || host.settings.protocol.value != Protocol::Ssh {
            return None;
        }
        let definition = workspace.host_platform(id)?;
        let command = given(&definition.show_config)?;
        let workspace_id = workspace.id()?.to_string();
        let profile = match profiles.login_with(id, Protocol::Ssh) {
            Ok(HostLogin::Profile { profile, .. }) => Some(profile),
            _ => None,
        };
        let enable_secret = profile
            .as_deref()
            .and_then(|profile| profiles.profile(profile).ok().flatten())
            .and_then(|stored| stored.enable_secret.clone())
            .filter(|secret| !secret.is_empty())
            .map(EnableSecret);
        // A second connection asks nothing: none if any hop would ask.
        let target = match login {
            SessionLogin::Profile => profiles
                .profile_target(id)
                .ok()
                .filter(|target| target.jump_hosts.iter().all(|j| j.auth != SshAuth::Prompt)),
            SessionLogin::Typed => None,
        };
        let work = HostWork {
            target,
            profile,
            enable_secret,
            prompt: definition.prompt.clone(),
            privilege_command: given(&definition.privilege_command),
            privilege_prompt: given(&definition.privilege_prompt),
            disable_paging: given(&definition.disable_paging),
            lines: Vec::new(),
            backup: Some(BackupWork {
                command,
                ignored_lines: given(&definition.ignored_lines),
            }),
            napalm_driver: None,
            remote: None,
            definition: None,
        };
        Some(Self {
            host: PlannedHost {
                id: id.clone(),
                name: host.host.name.clone(),
                address: host.host.address.clone(),
                environment: host.settings.environment.as_ref().map(|e| e.value),
                platform: definition.name.clone(),
                destination: None,
                work,
            },
            store,
            workspace: workspace_id,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        })
    }

    /// The same backup, giving the device `timeout` (instead of
    /// [`DEFAULT_COMMAND_TIMEOUT`]) to show its prompt after each line.
    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// The host it backs up.
    pub fn host(&self) -> &HostId {
        &self.host.id
    }
}

impl WhenConnected for LoginBackup {
    fn connected(&self, session: ConnectedSession) {
        let backup = self.clone();
        tokio::spawn(async move {
            let result = super::run::back_up_on_login(
                session.manager(),
                backup.host,
                session.id(),
                (backup.store, backup.workspace),
                backup.command_timeout,
            )
            .await;
            let outcome = match (result.backup, result.state) {
                (Some(outcome), _) => outcome.into(),
                (None, super::HostState::Failed { reason }) => {
                    LoginBackupOutcome::Failed { reason }
                }
                (None, _) => LoginBackupOutcome::Failed {
                    reason: "the backup ended without keeping the configuration".into(),
                },
            };
            session.report_backup(outcome);
        });
    }
}
