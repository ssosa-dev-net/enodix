//! What the lab checks that run Python scripts share: the headless binary,
//! the developer's Python, and a workspace of lab devices behind the lab's
//! jump host whose keys are trusted as the app would have, with the
//! app's credential agent serving their logins.
//!
//! Python is the developer's: `ENODIX_LAB_PYTHON` names an interpreter
//! with the check's libraries installed (the package `enodix` comes from
//! this repository's `python/` folder, through `PYTHONPATH`). The headless
//! binary is `ENODIX_CLI`, else the debug build (`npm run test:lab` builds
//! it). With a jump host in the lab file, the hosts are behind it and a
//! script reaches them through the binary's loopback tunnels
//! (`enodix-cli tunnel`, opened by the package); the keys of the jump host
//! and the devices are trusted first by a terminal session through the
//! chain, into the workspace's own data folder.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use enodix_core::credentials::{
    AgentEndpoint, AgentServer, CredentialProfile, CredentialStore, MemoryStore, ProfileAuth,
    Profiles,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::SessionManager;
use enodix_core::workspace::Workspace;
use tempfile::TempDir;

use super::{Conversation, Device, Lab};

/// The jump host's file and credential profile in a check's workspace.
pub const JUMP_FILE: &str = "jump.yaml";
const JUMP_PROFILE: &str = "jump";

/// Names the Python interpreter the checks run their scripts with.
pub const PYTHON_ENV: &str = "ENODIX_LAB_PYTHON";

/// `ENODIX_LAB_PYTHON`, or why the check is not made (`needs` says what
/// that Python must have).
pub fn python(needs: &str) -> Result<PathBuf, String> {
    std::env::var_os(PYTHON_ENV)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{PYTHON_ENV} names no Python interpreter with {needs} installed"))
}

/// The headless binary.
pub fn cli() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("ENODIX_CLI").filter(|p| !p.is_empty()) {
        return Ok(path.into());
    }
    let name = if cfg!(windows) {
        "enodix-cli.exe"
    } else {
        "enodix-cli"
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "no headless binary at {}: build it with cargo build -p enodix-cli, or set ENODIX_CLI",
            path.display()
        ))
    }
}

/// This repository's `python/` folder: the package `enodix` a script
/// imports (`PYTHONPATH`).
pub fn package() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python")
}

/// A temporary workspace of lab devices (one host file each, named by the
/// device, with its definition as `platform` and a credential profile of
/// its name) behind the lab's jump host, if any, with a data folder
/// whose known hosts trust them and whose credential agent serves the
/// profiles while this lives.
pub struct ScriptWorkspace {
    _temp: TempDir,
    pub root: PathBuf,
    pub data: PathBuf,
    pub workspace: Workspace,
    pub store: Arc<MemoryStore>,
    _agent: AgentServer,
}

impl ScriptWorkspace {
    pub async fn prepare(lab: &Lab, devices: &[&Device], name: &str) -> Result<Self, String> {
        let temp = TempDir::new().map_err(|e| e.to_string())?;
        let root = temp.path().join(name);
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let data = temp.path().join("data");
        fs::create_dir_all(&data).map_err(|e| e.to_string())?;
        if let Some(jump) = &lab.jump_host {
            fs::write(
                root.join("_folder.yaml"),
                format!("jump_host: {JUMP_FILE}\n"),
            )
            .map_err(|e| e.to_string())?;
            fs::write(
                root.join(JUMP_FILE),
                format!(
                    "address: {}\nport: {}\ncredential_profile: {JUMP_PROFILE}\n",
                    jump.address, jump.port
                ),
            )
            .map_err(|e| e.to_string())?;
        }
        for device in devices {
            fs::write(
                root.join(format!("{}.yaml", device.name)),
                format!(
                    "address: {}\nport: {}\nplatform: {}\ncredential_profile: {}\nconnect_timeout: {}\n",
                    device.address,
                    device.port,
                    device.definition.as_deref().unwrap_or_default(),
                    device.name,
                    lab.timeout.as_secs().max(30),
                ),
            )
            .map_err(|e| e.to_string())?;
        }
        let mut workspace = Workspace::open(&root).map_err(|e| e.to_string())?;
        workspace.ensure_id().map_err(|e| e.to_string())?;
        let store = Arc::new(MemoryStore::new());
        for device in devices {
            store
                .put(
                    &workspace.credential_scope(),
                    &CredentialProfile {
                        name: device.name.clone(),
                        username: device.username.clone(),
                        auth: match &device.password {
                            Some(password) => ProfileAuth::Password {
                                password: password.clone(),
                            },
                            None => ProfileAuth::Agent,
                        },
                        enable_secret: device.enable_secret.clone(),
                    },
                )
                .map_err(|error| error.0)?;
        }
        if let Some(jump) = &lab.jump_host {
            store
                .put(
                    &workspace.credential_scope(),
                    &CredentialProfile {
                        name: JUMP_PROFILE.into(),
                        username: jump.username.clone(),
                        auth: jump.auth.clone(),
                        enable_secret: None,
                    },
                )
                .map_err(|error| error.0)?;
        }
        let agent = AgentServer::start(AgentEndpoint::for_data_folder(&data), store.clone())
            .map_err(|e| format!("the credential agent: {e}"))?;
        // The tunnels refuse keys the app has not trusted: trust them as a
        // terminal session would, in the data folder the binary reads.
        if lab.jump_host.is_some() {
            let trusting = SessionManager::new(KnownHosts::in_data_folder(&data));
            let profiles = Profiles::new(store.as_ref(), &workspace);
            for device in devices {
                let target = profiles
                    .profile_target(&device.host_id())
                    .map_err(|e| e.to_string())?;
                let mut session = Conversation::open_ssh(&trusting, target, None, lab.timeout)
                    .await
                    .map_err(|e| format!("{}: trusting its keys: {e}", device.name))?;
                session.close().await?;
            }
        }
        Ok(Self {
            _temp: temp,
            root,
            data,
            workspace,
            store,
            _agent: agent,
        })
    }
}
