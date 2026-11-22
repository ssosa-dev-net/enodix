//! Reaching a device over TCP, for every network transport.

use std::io;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

use super::{ConnectStage, OpenError};

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
            stage: ConnectStage::Tcp {
                host: host.to_owned(),
                port,
                via: None,
            },
        },
        _ => OpenError::Connect(error.to_string()),
    })
}

/// How many keepalive probes may go unanswered after the first silence
/// before the system drops the connection (where it lets a socket say).
const KEEPALIVE_RETRIES: u32 = 2;

/// Turns on TCP keepalive on `socket`: after `every` without a byte from
/// the device, the system sends a probe, again each `every`, and drops the
/// connection after [`KEEPALIVE_RETRIES`] unanswered, so a link that drops
/// packets is noticed after about three times `every` (the next read
/// fails). What each system lets a socket set (#133):
///
/// - Linux: all three (seconds, so at least one second each).
/// - Windows: time and interval (milliseconds); the count since Windows 10
///   1703 (`TCP_KEEPCNT`), else the system's 10 probes: about eleven
///   times `every`.
/// - macOS and the BSDs: all three (seconds).
///
/// A probe is answered by the far end's TCP, not its program, so a
/// device that stopped reading but whose system runs still answers.
///
/// Keepalive probes go out only while nothing sent waits for an
/// acknowledgement; once something does (the user typed into the dead
/// link), the system's retransmissions decide instead. On Linux the same
/// three times `every` bounds those too (`TCP_USER_TIMEOUT`); elsewhere the
/// system's own limit holds (on Windows, five retransmissions by default).
pub(super) fn keep_alive(socket: &TcpStream, every: Duration) -> io::Result<()> {
    let socket = socket2::SockRef::from(socket);
    // Linux and macOS take whole seconds, and refuse zero.
    let every = every.max(Duration::from_secs(1));
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(every)
        .with_interval(every);
    let counted = keepalive.clone().with_retries(KEEPALIVE_RETRIES);
    socket
        .set_tcp_keepalive(&counted)
        // A system that cannot set the count keeps its own.
        .or_else(|_| socket.set_tcp_keepalive(&keepalive))?;
    #[cfg(target_os = "linux")]
    socket.set_tcp_user_timeout(Some(every * (KEEPALIVE_RETRIES + 1)))?;
    Ok(())
}

/// The one test inside the core: what [`keep_alive`] sets is invisible
/// through the public API (the far end's program never sees a keepalive),
/// and a link that drops packets cannot be played on the loopback address,
/// so the options are read back from the socket.
#[cfg(test)]
mod tests {
    use super::*;

    async fn connected() -> (TcpStream, tokio::net::TcpListener) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        (
            TcpStream::connect(("127.0.0.1", port)).await.unwrap(),
            listener,
        )
    }

    #[tokio::test]
    async fn keepalive_is_on_and_timed_from_the_connect_timeout() {
        let (socket, _listener) = connected().await;
        keep_alive(&socket, Duration::from_secs(3)).unwrap();
        let options = socket2::SockRef::from(&socket);
        assert!(options.keepalive().unwrap());
        let (time, interval, count) = read_back(&socket);
        assert_eq!(time, Duration::from_secs(3));
        assert_eq!(interval, Duration::from_secs(3));
        assert_eq!(count, KEEPALIVE_RETRIES);
        #[cfg(target_os = "linux")]
        assert_eq!(
            options.tcp_user_timeout().unwrap(),
            Some(Duration::from_secs(9))
        );
    }

    #[tokio::test]
    async fn keepalive_is_at_least_a_second() {
        let (socket, _listener) = connected().await;
        keep_alive(&socket, Duration::from_millis(200)).unwrap();
        let (time, interval, _) = read_back(&socket);
        assert_eq!(
            (time, interval),
            (Duration::from_secs(1), Duration::from_secs(1))
        );
    }

    /// The idle time, the interval and the count the system holds.
    #[cfg(not(windows))]
    fn read_back(socket: &TcpStream) -> (Duration, Duration, u32) {
        let options = socket2::SockRef::from(socket);
        (
            options.tcp_keepalive_time().unwrap(),
            options.tcp_keepalive_interval().unwrap(),
            options.tcp_keepalive_retries().unwrap(),
        )
    }

    /// The idle time, the interval and the count the system holds (Windows
    /// 10 1709 and later read them back in seconds).
    #[cfg(windows)]
    fn read_back(socket: &TcpStream) -> (Duration, Duration, u32) {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            getsockopt, IPPROTO_TCP, TCP_KEEPCNT, TCP_KEEPIDLE, TCP_KEEPINTVL,
        };
        let raw = socket.as_raw_socket() as usize;
        let read = |option: i32| {
            let mut value: u32 = 0;
            let mut length = std::mem::size_of::<u32>() as i32;
            // SAFETY: `value` and `length` outlive the call, and `length`
            // is the size of `value`.
            let result = unsafe {
                getsockopt(
                    raw,
                    IPPROTO_TCP,
                    option,
                    std::ptr::addr_of_mut!(value).cast(),
                    &mut length,
                )
            };
            assert_eq!(result, 0, "getsockopt {option}");
            value
        };
        (
            Duration::from_secs(read(TCP_KEEPIDLE).into()),
            Duration::from_secs(read(TCP_KEEPINTVL).into()),
            read(TCP_KEEPCNT),
        )
    }
}
