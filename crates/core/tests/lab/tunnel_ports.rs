//! The lab check of tunnel ports: a script (`tunnel_ports.py`, beside
//! this file) opens the tunnels with ports 830 and 443 named and talks
//! NETCONF to the Junos device on 830 (ncclient, the device key checked
//! against the keys the app trusts, another key refused) and eAPI over
//! HTTPS to the cEOS device on 443 (requests), through the lab's jump
//! host.
//!
//! Python is the developer's (`NETTERM_LAB_PYTHON`); a check whose library
//! (ncclient, requests) that Python lacks is NOT CHECKED, as is a check
//! with no device of its platform in the lab file. The workspace, the
//! trusted keys and the tunnels are those of `super::scripts`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::scripts::{self, ScriptWorkspace};
use super::{Device, Lab, Platform};

/// What one check of the script reported.
#[derive(Debug, Deserialize)]
pub struct Reported {
    pub check: String,
    pub host: String,
    /// `passed`, `failed` or `not checked`.
    pub result: String,
    pub detail: String,
}

/// Runs the script. `Err`: nothing was checked, and why.
pub async fn check(lab: &Lab) -> Result<Vec<Reported>, String> {
    let python = scripts::python("ncclient and requests")?;
    let cli = scripts::cli()?;
    // Not an entry that names its checks (a second login, for those only).
    let first = |platform: Platform| {
        lab.devices
            .iter()
            .find(|d| d.platform == platform && d.only.is_none())
    };
    let netconf = first(Platform::Junos);
    let eapi = first(Platform::Ceos);
    let devices: Vec<&Device> = netconf.into_iter().chain(eapi).collect();
    if devices.is_empty() {
        return Err("the lab file has no junos and no ceos device".into());
    }
    let prepared = ScriptWorkspace::prepare(lab, &devices, "tunnel-ports").await?;
    let id = |device: Option<&Device>| {
        device.map_or_else(|| "-".to_owned(), |d| d.host_id().as_str().to_owned())
    };
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/lab/tunnel_ports.py");
    let mut command = Command::new(python);
    command
        .arg(script)
        .arg(&prepared.root)
        .arg(id(netconf))
        .arg(id(eapi))
        .env("NETTERM_CLI", cli)
        .env("NETTERM_DATA_DIR", &prepared.data)
        .env("PYTHONPATH", scripts::package())
        .stdin(Stdio::null());
    // The credential agent answers on this runtime while the script runs.
    let run = tokio::task::spawn_blocking(move || command.output());
    let output = tokio::time::timeout(Duration::from_secs(300), run)
        .await
        .map_err(|_| "the script did not end within 5 minutes".to_string())?
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("cannot run the script: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut reported: Vec<Reported> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        reported.push(
            serde_json::from_str(line)
                .map_err(|e| format!("the script printed {line:?} ({e}); stderr: {stderr}"))?,
        );
    }
    if !output.status.success() {
        return Err(format!(
            "the script exited with {}: {stderr}",
            output.status
        ));
    }
    for (check, device) in [("netconf", netconf), ("eapi", eapi)] {
        if device.is_none() {
            reported.push(Reported {
                check: check.into(),
                host: "-".into(),
                result: "not checked".into(),
                detail: "no device of its platform in the lab file".into(),
            });
        } else if !reported.iter().any(|r| r.check == check) {
            reported.push(Reported {
                check: check.into(),
                host: id(device),
                result: "failed".into(),
                detail: format!("the script reported nothing; stderr: {stderr}"),
            });
        }
    }
    Ok(reported)
}
