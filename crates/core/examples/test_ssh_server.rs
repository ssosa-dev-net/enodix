//! Runs the session test harness's SSH server standalone, so the app can be
//! tried by hand without a real device:
//!
//! ```text
//! cargo run -p netterm-core --example test_ssh_server -- 2222 [--agent <pipe or socket>] [--old-device <port>] [--telnet <port>] [--device <port>]
//! ```
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
//! listens on the given port. The shells behind them are
//! scripted; see `tests/support/mod.rs` and `tests/support/telnet.rs` for
//! their commands.

#[path = "../tests/support/mod.rs"]
mod support;

/// The hostname of the `--device` server's shell.
const DEVICE_HOSTNAME: &str = "lab-r1";

fn usage() -> ! {
    eprintln!(
        "usage: test_ssh_server [port] [--agent <pipe or socket>] [--old-device <port>] [--telnet <port>] [--device <port>]"
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
    println!(
        "test SSH server listening on 127.0.0.1:{} (username {:?} or {:?}, password {:?}, code {:?})",
        server.port(),
        support::USERNAME,
        support::OTP_USERNAME,
        support::PASSWORD,
        support::ONE_TIME_CODE,
    );
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
    if let Some(telnet) = &telnet {
        println!("telnet device listening on 127.0.0.1:{}", telnet.port());
    }
    std::future::pending::<()>().await;
}
