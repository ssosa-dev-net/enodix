//! `tunnel <workspace> [limit]`: loopback tunnels to the hosts behind jump
//! hosts, for the Python bridge (see `netterm_core::tunnel`).
//!
//! The hosts are those `inventory` would export with the same limit (a
//! script job's scope included). Each chain is connected through the core
//! with the jump hosts' stored logins (from the running app, the keychain
//! or the vault, as for `credentials`, whose master password may be asked
//! on the terminal) and the app's known host keys; no host-key or login
//! question is asked. One JSON line on stdout says, per host behind jump
//! hosts, where its tunnel listens (`127.0.0.1` and a port) with the
//! device's trusted host keys, or why it is not tunnelled. The tunnels then
//! serve until stdin ends (the starting process closed it, ended or was
//! killed); what goes wrong meanwhile is reported on stderr, one line each.

use std::io::Read;

use netterm_core::credentials::Profiles;
use netterm_core::known_hosts::KnownHosts;
use netterm_core::tunnel::{self, TunnelOptions, TunnelPlan, Tunnels};
use netterm_core::workspace::HostId;

use crate::credentials::{data_folder, open_store};
use crate::{print, scoped_export, Failure, NAME};

/// The version of the JSON line's format.
const FORMAT: u32 = 1;

pub fn run(args: &[String]) -> Result<(), Failure> {
    let (workspace, exported) = scoped_export(args)?;
    let ids: Vec<HostId> = exported.hosts.into_iter().map(|host| host.id).collect();
    let behind = tunnel::behind_jump_hosts(&workspace, &ids);
    let known_hosts = KnownHosts::in_data_folder(&data_folder()?);
    // No store is opened (no master password asked) when nothing is behind
    // a jump host.
    let plan = if behind.is_empty() {
        TunnelPlan::default()
    } else {
        let store = open_store()?;
        tunnel::plan(
            &workspace,
            &Profiles::new(store.as_ref(), &workspace),
            &known_hosts,
            &behind,
        )
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| Failure::Error(format!("cannot start: {error}")))?;
    let tunnels = runtime
        .block_on(Tunnels::open(
            plan,
            known_hosts,
            TunnelOptions::default(),
            |event| eprintln!("{NAME}: tunnel: {event}"),
        ))
        .map_err(|error| Failure::Error(format!("cannot listen on 127.0.0.1: {error}")))?;
    let line = serde_json::json!({ "format": FORMAT, "tunnels": tunnels.status() });
    print(&format!("{line}\n"))?;

    // Until whoever started it lets go of stdin.
    let mut sink = [0u8; 256];
    let mut stdin = std::io::stdin().lock();
    while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
    drop(tunnels);
    runtime.shutdown_background();
    Ok(())
}
