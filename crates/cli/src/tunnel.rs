//! `tunnel <workspace> [limit] [--port <number>...]`: loopback tunnels to
//! the hosts behind jump hosts, for the Python bridge (see
//! `netterm_core::tunnel`).
//!
//! The hosts are those `inventory` would export with the same limit (a
//! script job's scope included). Each chain is connected through the core
//! with the jump hosts' stored logins (from the running app, the keychain
//! or the vault, as for `credentials`, whose master password may be asked
//! on the terminal) and the app's known host keys; no host-key or login
//! question is asked. One JSON line on stdout says, per host behind jump
//! hosts, where its tunnel listens (`127.0.0.1` and a port) with the
//! device's trusted host keys, or why it is not tunnelled. Each `--port`
//! is a device port tunnelled too, for every host: a listening entry then
//! has `extra_ports`, from device port to the loopback port serving it. The tunnels then
//! serve until stdin ends (the starting process closed it, ended or was
//! killed); what goes wrong meanwhile is reported on stderr, one line each.

use std::io::Read;

use netterm_core::credentials::Profiles;
use netterm_core::known_hosts::KnownHosts;
use netterm_core::tunnel::{self, TunnelOptions, TunnelPlan, Tunnels};
use netterm_core::workspace::HostId;

use crate::credentials::{data_folder, open_store};
use crate::{option_and_value, print, scoped_export, Failure, END_OF_OPTIONS, NAME};

/// The version of the JSON line's format.
const FORMAT: u32 = 1;

pub fn run(args: &[String]) -> Result<(), Failure> {
    let (args, ports) = ports(args)?;
    let (workspace, exported) = scoped_export(&args)?;
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
            &ports,
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

/// Takes the `--port <number>` (or `--port=<number>`) options out of
/// `args`, up to the end of the options (`--`): the rest, and the ports in
/// the order given. A value that is not a port (1 to 65535, in digits) is a
/// usage error.
fn ports(args: &[String]) -> Result<(Vec<String>, Vec<u16>), Failure> {
    let mut rest = Vec::with_capacity(args.len());
    let mut ports = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == END_OF_OPTIONS {
            rest.push(arg.clone());
            rest.extend(args.by_ref().cloned());
            break;
        }
        let (option, attached) = option_and_value(arg);
        if option != "--port" {
            rest.push(arg.clone());
            continue;
        }
        let value = match attached {
            // Checked as a port below, the empty value too.
            Some(value) => value,
            None => args
                .next()
                .map(String::as_str)
                .filter(|value| !value.starts_with('-'))
                .ok_or_else(|| Failure::Usage("--port needs a value".into()))?,
        };
        let port = Some(value)
            .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|&port| port != 0)
            .ok_or_else(|| {
                Failure::Usage(format!(
                    "--port takes a port from 1 to 65535, not {value:?}"
                ))
            })?;
        ports.push(port);
    }
    Ok((rest, ports))
}
