//! Reaching a device over TCP, for every network transport.

use std::io;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

use super::OpenError;

/// Resolves to the TCP connection to `host` on `port`, trying each address
/// the name resolves to in turn, each for its share of `timeout` (so an
/// address that never answers, often an IPv6 one, leaves time for the
/// next); a failure says which kind it was (from the last address tried).
pub(super) async fn connect(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<TcpStream, OpenError> {
    let not_resolved = || OpenError::NameNotResolved {
        host: host.to_owned(),
    };
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| not_resolved())?
        .collect();
    let count = u32::try_from(addresses.len()).unwrap_or(u32::MAX).max(1);
    let share = timeout / count;
    let started = Instant::now();
    let mut last = None;
    for address in addresses {
        match tokio::time::timeout(share, TcpStream::connect(address)).await {
            Ok(Ok(socket)) => return Ok(socket),
            Ok(Err(error)) => last = Some(error),
            Err(_) => last = Some(io::ErrorKind::TimedOut.into()),
        }
    }
    let Some(error) = last else {
        return Err(not_resolved());
    };
    Err(match error.kind() {
        io::ErrorKind::ConnectionRefused => OpenError::ConnectionRefused {
            host: host.to_owned(),
            port,
        },
        io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::AddrNotAvailable => OpenError::Unreachable {
            host: host.to_owned(),
            reason: error.to_string(),
        },
        // No address answered within its share (or the system gave up).
        io::ErrorKind::TimedOut => OpenError::TimedOut {
            after: Duration::from_secs(started.elapsed().as_secs_f64().round() as u64),
        },
        _ => OpenError::Connect(error.to_string()),
    })
}
