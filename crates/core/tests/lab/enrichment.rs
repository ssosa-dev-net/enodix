//! The lab check of host enrichment: the script the app ships
//! (`python -m netterm.enrich`) run as a real job against the lab devices
//! NAPALM has a driver for, then the preview applied.
//!
//! Python is the developer's: `NETTERM_LAB_PYTHON` names an interpreter
//! with NAPALM installed (the package `netterm` comes from this repository's
//! `python/` folder, through `PYTHONPATH`); without it the check prints why
//! it is NOT CHECKED. The headless binary is `NETTERM_CLI`, else the debug
//! build (`npm run test:lab` builds it). With a jump host in the
//! lab file, the hosts are behind it in the check's workspace and the script
//! reaches them through the headless binary's loopback tunnels
//! (`netterm-cli tunnel`, opened by the package); the keys of the jump host
//! and the devices are trusted first by a terminal session through the
//! chain, into the check's own data folder, as the app would have.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use netterm_core::credentials::{
    AgentEndpoint, AgentServer, CredentialProfile, CredentialStore, MemoryStore, ProfileAuth,
    Profiles,
};
use netterm_core::enrich::{self, napalm_driver, EnrichField};
use netterm_core::job::{self, JobEvent, JobOptions, JobStep, JobTarget, ScriptOptions};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::SessionManager;
use netterm_core::workspace::{HostId, Workspace};
use tempfile::TempDir;

use super::{Conversation, Device, Lab};

/// The jump host's file and credential profile in the check's workspace.
const JUMP_FILE: &str = "jump.yaml";
const JUMP_PROFILE: &str = "jump";

/// Names the Python interpreter (with NAPALM) the check runs the script
/// with.
pub const PYTHON_ENV: &str = "NETTERM_LAB_PYTHON";

/// What the check found: lines to print, and failures.
#[derive(Debug, Default)]
pub struct Checked {
    pub notes: Vec<String>,
    pub failures: Vec<String>,
    /// How many devices had their facts filled in.
    pub filled: usize,
}

fn cli() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("NETTERM_CLI").filter(|p| !p.is_empty()) {
        return Ok(path.into());
    }
    let name = if cfg!(windows) {
        "netterm-cli.exe"
    } else {
        "netterm-cli"
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "no headless binary at {}: build it with cargo build -p netterm-cli, or set NETTERM_CLI",
            path.display()
        ))
    }
}

/// Runs the check. `Err`: not checked, and why.
pub async fn check(lab: &Lab) -> Result<Checked, String> {
    let python = std::env::var_os(PYTHON_ENV)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| format!("{PYTHON_ENV} names no Python interpreter with NAPALM installed"))?;
    let cli = cli()?;
    let temp = TempDir::new().map_err(|e| e.to_string())?;
    let root = temp.path().join("enrichment");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let data = temp.path().join("data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;

    let built_in = Workspace::open(&root).map_err(|e| e.to_string())?;
    let supported: Vec<(&Device, &'static str)> = lab
        .devices
        .iter()
        .filter_map(|device| {
            let definition = built_in.platform(device.definition.as_deref()?)?;
            Some((device, napalm_driver(definition)?))
        })
        .collect();
    let mut checked = Checked::default();
    for device in &lab.devices {
        if !supported.iter().any(|(d, _)| d.name == device.name) {
            checked.notes.push(format!(
                "{}: no NAPALM driver for its platform, not checked",
                device.name
            ));
        }
    }
    if supported.is_empty() {
        return Err("no device of the lab has a platform NAPALM has a driver for".into());
    }

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
    for (device, driver) in &supported {
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
        checked
            .notes
            .push(format!("{}: NAPALM driver {driver}", device.name));
    }
    let mut workspace = Workspace::open(&root).map_err(|e| e.to_string())?;
    workspace.ensure_id().map_err(|e| e.to_string())?;
    let store = Arc::new(MemoryStore::new());
    for (device, _) in &supported {
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
    let _agent = AgentServer::start(AgentEndpoint::for_data_folder(&data), store.clone())
        .map_err(|e| format!("the credential agent: {e}"))?;
    // The tunnels refuse keys the app has not trusted: trust them as a
    // terminal session would, in the data folder the binary reads.
    if lab.jump_host.is_some() {
        let trusting = SessionManager::new(KnownHosts::in_data_folder(&data));
        let profiles = Profiles::new(store.as_ref(), &workspace);
        for (device, _) in &supported {
            let target = profiles
                .profile_target(&device.host_id())
                .map_err(|e| e.to_string())?;
            let mut session = Conversation::open_ssh(&trusting, target, None, lab.timeout)
                .await
                .map_err(|e| format!("{}: trusting its keys: {e}", device.name))?;
            session.close().await?;
        }
    }

    let plan = job::plan(
        &workspace,
        &Profiles::new(store.as_ref(), &workspace),
        &JobTarget::Folder { path: "".into() },
        &JobStep::Enrich,
    )
    .map_err(|e| e.to_string())?;
    for skipped in &plan.skipped {
        checked
            .failures
            .push(format!("{}: skipped: {}", skipped.name, skipped.message));
    }
    let manager = SessionManager::new(KnownHosts::in_memory());
    let options = JobOptions {
        script: Some(ScriptOptions {
            interpreter: python.into(),
            cli: Some(cli),
            environment: vec![
                ("NETTERM_DATA_DIR".into(), data.clone().into()),
                (
                    "PYTHONPATH".into(),
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../python")
                        .into(),
                ),
            ],
        }),
        ..JobOptions::default()
    };
    let mut running = job::start(&manager, plan, options).map_err(|e| e.to_string())?;
    let report = loop {
        let event = tokio::time::timeout(Duration::from_secs(300), running.next())
            .await
            .map_err(|_| "the script did not end within 5 minutes".to_string())?
            .ok_or("the job ended without a report")?;
        if let JobEvent::Finished(report) = event {
            break report;
        }
    };
    let result = &report.hosts[0];
    for line in result.output.lines() {
        checked.notes.push(format!("script: {line}"));
    }
    let facts = result.facts.clone().ok_or("the job kept no facts")?;
    for host in &facts.hosts {
        match (&host.facts, &host.error) {
            (Some(facts), _) => {
                let missing: Vec<&str> = EnrichField::ALL
                    .into_iter()
                    .filter(|field| facts.get(*field).is_none())
                    .map(EnrichField::key)
                    .collect();
                if !missing.is_empty() {
                    checked.failures.push(format!(
                        "{}: reported no {}",
                        host.id,
                        missing.join(", ")
                    ));
                }
            }
            (None, error) => checked.failures.push(format!(
                "{}: {}",
                host.id,
                error.as_deref().unwrap_or("no facts")
            )),
        }
    }

    let preview = enrich::preview(&workspace, &facts, &[]);
    let applied = enrich::apply(&mut workspace, &preview, &[]);
    for host in &applied.failed {
        checked.failures.push(format!(
            "{}: not filled in: {}",
            host.id,
            host.reason.as_deref().unwrap_or_default()
        ));
    }
    checked.filled = applied.filled.len();
    for (device, _) in &supported {
        let id = HostId::new(format!("{}.yaml", device.name));
        let host = workspace.host(&id).ok_or("a lab host is gone")?;
        let line = format!(
            "{}: vendor {:?}, model {:?}, serial number {:?}, os_version {:?}",
            device.name,
            host.vendor,
            host.model,
            host.serial_number,
            host.custom
                .iter()
                .find(|f| f.key == enrich::OS_VERSION_FIELD)
                .map(|f| f.value.as_str()),
        );
        if host.vendor.is_none() {
            checked
                .failures
                .push(format!("{line}: nothing was written"));
        }
        checked.notes.push(line);
    }
    Ok(checked)
}
