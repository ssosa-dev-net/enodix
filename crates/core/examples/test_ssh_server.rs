//! Runs the session test harness's SSH server standalone, so the app can be
//! tried by hand without a real device:
//!
//! ```text
//! cargo run -p netterm-core --example test_ssh_server -- 2222 [--agent <pipe or socket>] [--old-device <port>] [--telnet <port>] [--device <port>] [--credential-agent <data folder> <workspace id>] [--sftp <folder>] [--scp-only <port> <folder>] [--banner <port>]
//! ```
//!
//! Port 0 (for any of them) takes a free one; the lines printed name it.
//!
//! Then quick-connect to 127.0.0.1 on that port with the printed login.
//! The user `engineer-otp` logs in with the same password, then a one-time
//! code (keyboard-interactive). With `--agent`, an SSH agent holding a key
//! the server accepts also listens at the given named pipe (Windows) or
//! Unix socket: point `SSH_AUTH_SOCK` at it to log in with an agent
//! profile. With `--old-device`, a second server with the same login
//! listens on the given port, restricted to an old device's algorithms
//! (`support::algorithms::old_device`): it is reached only with the
//! legacy-crypto switch on. With `--telnet`, the in-process telnet server
//! (`tests/support/telnet.rs`, same username and password, asked for in
//! the terminal) listens on the given port. With `--device`, a server with
//! the same login whose shell runs in device mode as `lab-r1` (prompt
//! `lab-r1# `, for jobs: the built-in `frr` platform's prompt matches it)
//! listens on the given port. With `--credential-agent`, the app's
//! credential agent (`netterm_core::credentials::AgentServer`) serves the
//! data folder from memory, as a running app would, with test profiles
//! for the workspace id: `lab` (password, with an enable secret), `keys`
//! (the `--device` server's client key, written into the data folder with
//! a passphrase; without `--device`, the first server's) and `agent`, and
//! the secret `netbox-token`. It prints them, secrets included, as one
//! JSON line after `credential agent: `, for the Python bridge's tests
//! (`python/tests`). It also prints each server's host key (`host key: `,
//! `device host key: `), for tests that trust them as the app would. With
//! `--sftp`, the first server and the `--device` server serve the folder
//! (created if missing) over SFTP, as `/` (`tests/support/sftp.rs`); without
//! it they refuse the subsystem, as a device without SFTP. With
//! `--scp-only`, a server with the same logins (`engineer-otp`'s one-time
//! code included, which the smoke test answers per transfer) listens on
//! the given port,
//! has no SFTP the way IOS-XE has none (it accepts the subsystem, closes
//! the channel and then the whole connection) and serves the folder
//! (created if missing) over SCP, as `/` (`tests/support/scp.rs`). With
//! `--banner`, a server with the same logins that sends [`LOGIN_BANNER`]
//! before the login (an SSH login banner, #134) listens on the given port.
//! The shells behind them are
//! scripted; see `tests/support/mod.rs` and `tests/support/telnet.rs` for
//! their commands.

#[path = "../tests/support/mod.rs"]
mod support;

/// What the `--banner` server sends before the login, with bare line ends
/// as devices send it.
const LOGIN_BANNER: &str = "AUTHORIZED ACCESS ONLY
This is the test server's login banner.
";

/// The hostname of the `--device` server's shell.
const DEVICE_HOSTNAME: &str = "lab-r1";

fn usage() -> ! {
    eprintln!(
        "usage: test_ssh_server [port] [--agent <pipe or socket>] [--old-device <port>] [--telnet <port>] [--device <port>] [--credential-agent <data folder> <workspace id>] [--sftp <folder>] [--scp-only <port> <folder>] [--banner <port>]"
    );
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let mut port = 2222;
    let mut agent_path = None;
    let mut old_device_port = None;
    let mut telnet_port = None;
    let mut device_port = None;
    let mut credential_agent = None;
    let mut sftp_folder = None;
    let mut scp_only = None;
    let mut banner_port = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--agent" {
            agent_path = Some(args.next().unwrap_or_else(|| usage()));
        } else if arg == "--old-device" {
            let port = args.next().unwrap_or_else(|| usage());
            old_device_port = Some(port.parse().unwrap_or_else(|_| usage()));
        } else if arg == "--device" {
            let port = args.next().unwrap_or_else(|| usage());
            device_port = Some(port.parse().unwrap_or_else(|_| usage()));
        } else if arg == "--credential-agent" {
            let folder = args.next().unwrap_or_else(|| usage());
            let scope = args.next().unwrap_or_else(|| usage());
            credential_agent = Some((std::path::PathBuf::from(folder), scope));
        } else if arg == "--sftp" {
            let folder = args.next().unwrap_or_else(|| usage());
            sftp_folder = Some(std::path::PathBuf::from(folder));
        } else if arg == "--scp-only" {
            let port: u16 = args
                .next()
                .and_then(|port| port.parse().ok())
                .unwrap_or_else(|| usage());
            let folder = args.next().unwrap_or_else(|| usage());
            scp_only = Some((port, std::path::PathBuf::from(folder)));
        } else if arg == "--banner" {
            let port = args.next().unwrap_or_else(|| usage());
            banner_port = Some(port.parse().unwrap_or_else(|_| usage()));
        } else if arg == "--telnet" {
            let port = args.next().unwrap_or_else(|| usage());
            telnet_port = Some(port.parse().unwrap_or_else(|_| usage()));
        } else {
            port = arg.parse().unwrap_or_else(|_| usage());
        }
    }
    let server = support::TestSshServer::start_on(port).await;
    let old_device = match old_device_port {
        Some(port) => Some(
            support::TestSshServer::start_on_with_algorithms(
                port,
                support::algorithms::old_device(),
            )
            .await,
        ),
        None => None,
    };
    let device = match device_port {
        Some(port) => Some(support::TestSshServer::start_device_on(port, DEVICE_HOSTNAME).await),
        None => None,
    };
    if let Some(folder) = &sftp_folder {
        std::fs::create_dir_all(folder).expect("the SFTP folder can be created");
        server.serve_sftp(folder);
        if let Some(device) = &device {
            device.serve_sftp(folder);
        }
        println!("sftp folder: {}", folder.display());
    }
    let scp_only = match scp_only {
        Some((port, folder)) => {
            std::fs::create_dir_all(&folder).expect("the SCP folder can be created");
            let scp = support::TestSshServer::start_on(port).await;
            scp.serve_scp(&folder);
            // As IOS-XE: the SFTP channel is closed, and the connection
            // with it.
            scp.close_sftp_and_hang_up();
            Some(scp)
        }
        None => None,
    };
    let banner = match banner_port {
        Some(port) => {
            let banner = support::TestSshServer::start_on(port).await;
            banner.quirks().auth_banner(LOGIN_BANNER);
            Some(banner)
        }
        None => None,
    };
    let telnet = match telnet_port {
        Some(port) => Some(
            support::telnet::TestTelnetServer::start_on(port, support::telnet::EchoMode::Remote)
                .await,
        ),
        None => None,
    };
    let _agent = match agent_path {
        Some(path) => {
            let agent = support::agent::TestAgent::start_at(
                path.clone().into(),
                &[server.client_key().clone()],
            )
            .await;
            if agent.is_none() {
                eprintln!("something already listens at {path}");
                std::process::exit(1);
            }
            agent
        }
        None => None,
    };
    let _credential_agent = credential_agent.map(|(folder, scope)| {
        serve_credentials(&folder, &scope, device.as_ref().unwrap_or(&server))
    });
    println!(
        "test SSH server listening on 127.0.0.1:{} (username {:?} or {:?}, password {:?}, code {:?})",
        server.port(),
        support::USERNAME,
        support::OTP_USERNAME,
        support::PASSWORD,
        support::ONE_TIME_CODE,
    );
    // So that a test can trust them, as the app would have.
    println!(
        "host key: {}",
        server.host_key().to_openssh().unwrap_or_default()
    );
    if let Some(device) = &device {
        println!(
            "device host key: {}",
            device.host_key().to_openssh().unwrap_or_default()
        );
    }
    if let Some(old_device) = &old_device {
        println!(
            "old device (legacy algorithms only) listening on 127.0.0.1:{}",
            old_device.port()
        );
    }
    if let Some(device) = &device {
        println!(
            "device {DEVICE_HOSTNAME} (device-mode shell) listening on 127.0.0.1:{}",
            device.port()
        );
    }
    if let Some(scp) = &scp_only {
        println!("SCP-only device listening on 127.0.0.1:{}", scp.port());
    }
    if let Some(banner) = &banner {
        println!(
            "device with a login banner listening on 127.0.0.1:{}",
            banner.port()
        );
    }
    if let Some(telnet) = &telnet {
        println!("telnet device listening on 127.0.0.1:{}", telnet.port());
    }
    std::future::pending::<()>().await;
}

/// The passphrase of the `keys` profile's key file.
const KEY_PASSPHRASE: &str = "key phrase";
/// The `netbox-token` secret's value.
const SECRET_VALUE: &str = "t0ken-for-tests";

/// Serves test profiles for `scope` as the app's credential agent of the
/// data folder `folder`, and prints them as one JSON line.
fn serve_credentials(
    folder: &std::path::Path,
    scope: &str,
    server: &support::TestSshServer,
) -> netterm_core::credentials::AgentServer {
    use netterm_core::credentials::{
        AgentEndpoint, AgentServer, CredentialProfile, CredentialStore, MemoryStore, ProfileAuth,
        SecretValue,
    };

    std::fs::create_dir_all(folder).unwrap_or_else(|error| {
        eprintln!("cannot create {}: {error}", folder.display());
        std::process::exit(1);
    });
    let key_file = folder.join("client_key");
    server.write_client_key(&key_file, Some(KEY_PASSPHRASE));
    let profiles = [
        CredentialProfile {
            name: "lab".into(),
            username: support::USERNAME.into(),
            auth: ProfileAuth::Password {
                password: support::PASSWORD.into(),
            },
            enable_secret: Some(support::ENABLE_SECRET.into()),
        },
        CredentialProfile {
            name: "keys".into(),
            username: support::USERNAME.into(),
            auth: ProfileAuth::PrivateKey {
                key_file: key_file.clone(),
                passphrase: Some(KEY_PASSPHRASE.into()),
            },
            enable_secret: None,
        },
        CredentialProfile {
            name: "agent".into(),
            username: support::USERNAME.into(),
            auth: ProfileAuth::Agent,
            enable_secret: None,
        },
    ];
    let store = MemoryStore::new();
    for profile in &profiles {
        store.put(scope, profile).unwrap();
    }
    store
        .put_secret(scope, "netbox-token", &SecretValue::new(SECRET_VALUE))
        .unwrap();
    let agent = AgentServer::start(
        AgentEndpoint::for_data_folder(folder),
        std::sync::Arc::new(store),
    )
    .unwrap_or_else(|error| {
        eprintln!("cannot serve credentials for {}: {error}", folder.display());
        std::process::exit(1);
    });
    let described = serde_json::json!({
        "data_folder": folder,
        "scope": scope,
        "profiles": profiles,
        "secrets": { "netbox-token": SECRET_VALUE },
    });
    println!("credential agent: {described}");
    agent
}
