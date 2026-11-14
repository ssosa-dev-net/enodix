//! The lab check of host enrichment: the script the app ships
//! (`python -m netterm.enrich`) run as a real job against the lab devices
//! NAPALM has a driver for, then the preview applied.
//!
//! Python is the developer's: `NETTERM_LAB_PYTHON` names an interpreter
//! with NAPALM installed (the package `netterm` comes from this repository's
//! `python/` folder, through `PYTHONPATH`); without it the check prints why
//! it is NOT CHECKED. The headless binary is `NETTERM_CLI`, else the debug
//! build (`npm run test:lab` builds it). NAPALM makes its own connections
//! and cannot go through jump hosts, so with a jump host in the lab file the
//! check opens the system's `ssh -L` tunnels through it (a key file or the
//! agent; a password jump host is NOT CHECKED) and the hosts point at them.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

use super::{Device, Lab, LabJumpHost};

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

/// Tunnels opened with the system's `ssh`, closed when dropped.
struct Tunnels(Vec<Child>);

impl Drop for Tunnels {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A tunnel from a local port to `address:port` through the jump host.
/// The jump host's key is trusted on first use into `known_hosts`, a
/// file of the check's own (never the user's).
fn tunnel(
    jump: &LabJumpHost,
    known_hosts: &Path,
    address: &str,
    port: u16,
) -> Result<(u16, Child), String> {
    let local = free_port();
    let mut command = Command::new("ssh");
    command
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts.display()));
    command.args([
        "-N",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-p",
        &jump.port.to_string(),
        "-L",
        &format!("127.0.0.1:{local}:{address}:{port}"),
    ]);
    match &jump.auth {
        ProfileAuth::PrivateKey { key_file, .. } => {
            command.arg("-i").arg(key_file);
        }
        ProfileAuth::Agent => {}
        ProfileAuth::Password { .. } => {
            return Err(
                "the jump host logs in with a password, which ssh -L cannot be given".into(),
            )
        }
    }
    command.arg(format!("{}@{}", jump.username, jump.address));
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("ssh could not be started: {error}"))?;
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("ssh -L to {address}:{port} ended: {status}"));
        }
        if TcpStream::connect(("127.0.0.1", local)).is_ok() {
            return Ok((local, child));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
    Err(format!("ssh -L to {address}:{port} did not open"))
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

    let known_hosts = temp.path().join("known_hosts");
    let mut tunnels = Tunnels(Vec::new());
    for (device, driver) in &supported {
        let (address, port) = match &lab.jump_host {
            Some(jump) => {
                let (port, child) = tunnel(jump, &known_hosts, &device.address, device.port)?;
                tunnels.0.push(child);
                ("127.0.0.1".to_string(), port)
            }
            None => (device.address.clone(), device.port),
        };
        fs::write(
            root.join(format!("{}.yaml", device.name)),
            format!(
                "address: {address}\nport: {port}\nplatform: {}\ncredential_profile: {}\nconnect_timeout: {}\n",
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
    let _agent = AgentServer::start(AgentEndpoint::for_data_folder(&data), store.clone())
        .map_err(|e| format!("the credential agent: {e}"))?;

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
    drop(tunnels);
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
