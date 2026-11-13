//! Session test harness: an SSH server that runs inside the test process.
//!
//! The spec's session tests use a containerised OpenSSH server; this harness
//! is the fallback for machines without Docker. It speaks the real SSH
//! protocol (key exchange, authentication, channels, pty and window-change
//! requests) through `russh`'s server side, with a small scripted shell
//! behind it instead of a real one.
//!
//! The scripted shell echoes input like a tty and understands:
//!
//! | line         | effect                                            |
//! |--------------|---------------------------------------------------|
//! | `size`       | prints the pty size as `<cols>x<rows>`            |
//! | `colors`     | prints [`COLOR_OUTPUT`]                           |
//! | `fullscreen` | prints [`FULLSCREEN_OUTPUT`]                      |
//! | `lines <n>`  | prints `line 1` to `line <n>`, one per row          |
//! | `flood`      | prints [`FLOOD_LINE`] without end until Ctrl+C    |
//! | `exit [n]`   | ends the shell with exit status `n` (default 0)   |
//! | `drop`       | drops the connection without ending the shell     |
//!
//! Ctrl+C prints [`INTERRUPTED`] and a new prompt, stopping a flood. A flood
//! is sent as fast as the client's SSH window allows, like a device dumping
//! a huge table.
//!
//! The output helpers acknowledge every byte they receive, as the interface
//! does once the terminal has consumed it.
//! `examples/test_ssh_server.rs` runs the same server standalone, for trying
//! the app by hand and for the end-to-end smoke test.
//!
//! Later tickets extend this file (more auth methods, host keys, ...).
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use netterm_core::session::{SessionEvent, SessionEvents, SessionState, SshTarget};
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{Auth, ChannelOpenHandle, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId, Pty};
use tokio::net::TcpListener;

pub const USERNAME: &str = "engineer";
pub const PASSWORD: &str = "correct horse";
pub const PROMPT: &[u8] = b"test-ssh$ ";

/// The byte a terminal sends for Ctrl+C.
pub const CTRL_C: u8 = 0x03;

/// SGR colour sequences, as a device would send them.
pub const COLOR_OUTPUT: &[u8] = b"\x1b[1;31mdown\x1b[0m \x1b[38;5;46mup\x1b[0m\r\n";

/// What a full-screen program sends: alternate screen, clear, cursor
/// addressing, a UTF-8 box-drawing character, and back to the main screen.
pub const FULLSCREEN_OUTPUT: &[u8] =
    b"\x1b[?1049h\x1b[2J\x1b[H\x1b[5;10H\xe2\x94\x82 top\x1b[?25l\x1b[?25h\x1b[?1049l";

/// One line of `flood` output.
pub const FLOOD_LINE: &[u8] = b"flood flood flood flood flood flood flood\r\n";

/// What Ctrl+C prints before the next prompt.
pub const INTERRUPTED: &[u8] = b"^C\r\n";

/// How long a test waits for something the server should send.
pub const WAIT: Duration = Duration::from_secs(10);

pub struct TestSshServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl TestSshServer {
    /// Starts a server on a free loopback port.
    pub async fn start() -> Self {
        Self::start_on(0).await
    }

    /// Starts a server on the given loopback port (0 picks a free one).
    pub async fn start_on(port: u16) -> Self {
        let config = Arc::new(russh::server::Config {
            auth_rejection_time: Duration::ZERO,
            // A small receive window, as on network devices: input larger
            // than this has to wait for the server to make room.
            window_size: 16 * 1024,
            keys: vec![PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()],
            ..Default::default()
        });
        let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut server = ScriptedServer;
            let _ = server.run_on_socket(config, &listener).await;
        });
        Self { port, task }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// A target that logs in to this server successfully.
    pub fn target(&self) -> SshTarget {
        SshTarget {
            host: "127.0.0.1".into(),
            port: self.port,
            username: USERNAME.into(),
            password: PASSWORD.into(),
        }
    }
}

impl Drop for TestSshServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct ScriptedServer;

impl Server for ScriptedServer {
    type Handler = ScriptedShell;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> ScriptedShell {
        ScriptedShell::default()
    }
}

#[derive(Default)]
struct ScriptedShell {
    /// Pty size and pending input line, per channel.
    channels: HashMap<ChannelId, ShellState>,
}

#[derive(Default)]
struct ShellState {
    cols: u32,
    rows: u32,
    line: Vec<u8>,
    /// Set while a flood is running; setting the flag stops it.
    flood: Option<Arc<AtomicBool>>,
}

/// Sends [`FLOOD_LINE`] until `stop` is set, then the interrupt and a prompt.
fn start_flood(channel: ChannelId, session: &Session) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let handle = session.handle();
    let flooding = stop.clone();
    tokio::spawn(async move {
        let chunk = FLOOD_LINE.repeat(200);
        while !flooding.load(Ordering::Relaxed) {
            if handle.data(channel, chunk.clone()).await.is_err() {
                return;
            }
        }
        let _ = handle.data(channel, [INTERRUPTED, PROMPT].concat()).await;
    });
    stop
}

impl ScriptedShell {
    fn run_line(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), russh::Error> {
        let state = self.channels.entry(channel).or_default();
        let line = String::from_utf8_lossy(&std::mem::take(&mut state.line)).into_owned();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("size") => {
                let size = format!("{}x{}\r\n", state.cols, state.rows);
                session.data(channel, size.into_bytes())?;
            }
            Some("colors") => session.data(channel, COLOR_OUTPUT)?,
            Some("fullscreen") => session.data(channel, FULLSCREEN_OUTPUT)?,
            Some("lines") => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                session.data(channel, numbered_lines(count))?;
            }
            Some("flood") => {
                state.flood = Some(start_flood(channel, session));
                return Ok(());
            }
            Some("exit") => {
                let status = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                session.exit_status_request(channel, status)?;
                session.eof(channel)?;
                session.close(channel)?;
                return Ok(());
            }
            Some("drop") => return Err(russh::Error::Disconnect),
            _ => {}
        }
        session.data(channel, PROMPT)?;
        Ok(())
    }
}

impl Handler for ScriptedShell {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == USERNAME && password == PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), ShellState::default());
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let state = self.channels.entry(channel).or_default();
        state.cols = col_width;
        state.rows = row_height;
        session.channel_success(channel)
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let state = self.channels.entry(channel).or_default();
        state.cols = col_width;
        state.rows = row_height;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, PROMPT)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        for &byte in data {
            if byte == CTRL_C {
                let state = self.channels.entry(channel).or_default();
                state.line.clear();
                match state.flood.take() {
                    // The flood prints the interrupt and prompt as it stops.
                    Some(stop) => stop.store(true, Ordering::Relaxed),
                    None => session.data(channel, [INTERRUPTED, PROMPT].concat())?,
                }
            } else if self
                .channels
                .get(&channel)
                .is_some_and(|s| s.flood.is_some())
            {
                // Input typed during a flood is not echoed, like a busy device.
            } else if byte == b'\r' {
                session.data(channel, &b"\r\n"[..])?;
                self.run_line(channel, session)?;
            } else {
                session.data(channel, vec![byte])?;
                self.channels.entry(channel).or_default().line.push(byte);
            }
        }
        Ok(())
    }
}

/// What the scripted shell prints for `lines <count>`.
pub fn numbered_lines(count: u32) -> Vec<u8> {
    (1..=count)
        .flat_map(|n| format!("line {n}\r\n").into_bytes())
        .collect()
}

/// Waits for the next state change, discarding output on the way.
pub async fn next_state(events: &mut SessionEvents) -> SessionState {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::State(state)) => return state,
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                None => panic!("session events ended while waiting for a state change"),
            }
        }
    })
    .await
    .expect("timed out waiting for a state change")
}

/// Collects output until it ends with `expected`, and returns all of it.
///
/// Panics if a state change arrives first or nothing matching arrives in time.
pub async fn output_ending_with(events: &mut SessionEvents, expected: &[u8]) -> Vec<u8> {
    output_batches_ending_with(events, expected).await.concat()
}

/// Like [`output_ending_with`], but returns each output event separately.
pub async fn output_batches_ending_with(
    events: &mut SessionEvents,
    expected: &[u8],
) -> Vec<Vec<u8>> {
    let mut batches = Vec::new();
    let mut received = Vec::new();
    let result = tokio::time::timeout(WAIT, async {
        while !received.ends_with(expected) {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    received.extend_from_slice(&bytes);
                    batches.push(bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await;
    if result.is_err() {
        panic!(
            "timed out waiting for output ending with {:?}; received {:?}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&received)
        );
    }
    batches
}

/// Collects output without acknowledging it until none arrives for `quiet`.
pub async fn unacknowledged_output(events: &mut SessionEvents, quiet: Duration) -> Vec<u8> {
    let mut received = Vec::new();
    loop {
        match tokio::time::timeout(quiet, events.next()).await {
            Ok(Some(SessionEvent::Output(bytes))) => received.extend_from_slice(&bytes),
            Ok(other) => panic!("expected output, got {other:?}"),
            Err(_) => return received,
        }
    }
}
