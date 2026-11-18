//! The lab check of host enrichment: the script the app ships
//! (`python -m netterm.enrich`) run as a real job against the lab devices
//! NAPALM has a driver for, then the preview applied.
//!
//! Python is the developer's (`NETTERM_LAB_PYTHON`, with NAPALM); without it
//! the check prints why it is NOT CHECKED. The workspace, the trusted keys
//! and the tunnels are those of `super::scripts`.

use std::time::Duration;

use netterm_core::credentials::Profiles;
use netterm_core::enrich::{self, napalm_driver, EnrichField};
use netterm_core::job::{self, JobEvent, JobOptions, JobStep, JobTarget, ScriptOptions};
use netterm_core::known_hosts::KnownHosts;
use netterm_core::session::SessionManager;
use netterm_core::workspace::{HostId, Workspace};
use tempfile::TempDir;

use super::scripts::{self, ScriptWorkspace};
use super::{Device, Lab};

/// What the check found: lines to print, and failures.
#[derive(Debug, Default)]
pub struct Checked {
    pub notes: Vec<String>,
    pub failures: Vec<String>,
    /// How many devices had their facts filled in.
    pub filled: usize,
}

/// Runs the check. `Err`: not checked, and why.
pub async fn check(lab: &Lab) -> Result<Checked, String> {
    let python = scripts::python("NAPALM")?;
    let cli = scripts::cli()?;
    let probe = TempDir::new().map_err(|e| e.to_string())?;
    let built_in = Workspace::open(probe.path()).map_err(|e| e.to_string())?;
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
    for (device, driver) in &supported {
        checked
            .notes
            .push(format!("{}: NAPALM driver {driver}", device.name));
    }
    let devices: Vec<&Device> = supported.iter().map(|(device, _)| *device).collect();
    let mut prepared = ScriptWorkspace::prepare(lab, &devices, "enrichment").await?;
    let store = prepared.store.clone();
    let data = prepared.data.clone();
    let workspace = &mut prepared.workspace;

    let plan = job::plan(
        workspace,
        &Profiles::new(store.as_ref(), workspace),
        &JobTarget::Folder { path: "".into() },
        &JobStep::Enrich,
    )
    .map_err(|e| e.to_string())?;
    // The job targets the whole workspace, so it also meets the check's
    // jump host, which is no device and has no platform: skipped, rightly.
    let jump = HostId::new(scripts::JUMP_FILE);
    for skipped in plan.skipped.iter().filter(|skipped| skipped.id != jump) {
        checked
            .failures
            .push(format!("{}: skipped: {}", skipped.name, skipped.message));
    }
    let manager = SessionManager::new(KnownHosts::in_memory());
    let options = JobOptions {
        script: Some(ScriptOptions {
            interpreter: python,
            cli: Some(cli),
            environment: vec![
                ("NETTERM_DATA_DIR".into(), data.clone().into()),
                ("PYTHONPATH".into(), scripts::package().into()),
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

    let preview = enrich::preview(workspace, &facts, &[]);
    let applied = enrich::apply(workspace, &preview, &[]);
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
