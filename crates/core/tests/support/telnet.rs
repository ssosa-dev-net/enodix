//! An in-process telnet server (RFC 854) with a scripted device behind it.
//!
//! It speaks real telnet on a loopback port: option negotiation (it asks
//! for the terminal type and the window size, and offers to echo and to
//! suppress go-ahead), subnegotiations, `IAC` escaping and the NVT
//! end-of-line conventions. Behind it is a device that logs in like a
//! router (`Username: `, `Password: `) and then runs a few commands:
//!
//! | line        | effect                                                   |
//! |-------------|----------------------------------------------------------|
//! | `size`      | prints the last window size the client sent (`80x24`)    |
//! | `term`      | prints the terminal type the client sent                 |
//! | `echo off`  | stops echoing (`WONT ECHO`), as a line-mode device       |
//! | `echo on`   | echoes again (`WILL ECHO`)                               |
//! | `binary`    | prints the bytes `A`, `0xFF`, `B` and a new line         |
//! | `flood`     | prints [`FLOOD_LINE`] without end until Ctrl+C           |
//! | `exit`      | closes the connection                                    |
//! | other       | prints [`UNKNOWN_COMMAND`] and the line as received      |
//!
//! Ctrl+C prints [`INTERRUPTED`] and a new prompt, stopping a flood.
//! `DO TIMING-MARK` (the session's probe of a quiet link, #133) is answered
//! with `WILL TIMING-MARK` once everything before it has been taken, as
//! BSD telnetd and IOS do, and counted ([`TestTelnetServer::probes`]).
//! Output goes through a small bounded queue, so a client that stops
//! reading stops a flood once the TCP buffers fill, as on a real device.
//!
//! Login takes [`USERNAME`] and [`PASSWORD`]; anything else prints
//! [`LOGIN_INCORRECT`] and asks again. The password is never echoed.

use netterm_core::session::{TelnetTarget, DEFAULT_CONNECT_TIMEOUT};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use super::{CTRL_C, FLOOD_LINE, INTERRUPTED, PASSWORD, USERNAME};

pub const TELNET_PROMPT: &[u8] = b"test-telnet> ";
pub const USERNAME_PROMPT: &[u8] = b"Username: ";
pub const PASSWORD_PROMPT: &[u8] = b"Password: ";
pub const BANNER: &[u8] = b"Test telnet device\r\n\r\n";
pub const LOGIN_INCORRECT: &[u8] = b"\r\n% Login invalid\r\n\r\n";
pub const UNKNOWN_COMMAND: &[u8] = b"% Unknown command: ";

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const ECHO: u8 = 1;
const SGA: u8 = 3;
const TTYPE: u8 = 24;
const NAWS: u8 = 31;
const TIMING_MARK: u8 = 6;
const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

/// How the device echoes while the user logs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoMode {
    /// It offers to echo from the start (`WILL ECHO`) and echoes everything
    /// but the password, as most routers and Linux do.
    Remote,
    /// It leaves echoing to the client (no `WILL ECHO`) and offers to echo
    /// only while asking for the password, echoing nothing then, so the
    /// client stops its own echo: as old line-mode devices do.
    Line,
}

pub struct TestTelnetServer {
    port: u16,
    task: tokio::task::JoinHandle<()>,
    probes: Arc<AtomicUsize>,
}

impl TestTelnetServer {
    /// A server on a free loopback port that echoes from the start.
    pub async fn start() -> Self {
        Self::start_on(0, EchoMode::Remote).await
    }

    /// A server on a free loopback port whose device logs in as `mode` says.
    pub async fn start_with(mode: EchoMode) -> Self {
        Self::start_on(0, mode).await
    }

    /// A server whose device never asks for the window size or terminal
    /// type: it waits for the client to offer them (`WILL NAWS`, `WILL
    /// TTYPE`), as some devices do, and accepts the offers.
    pub async fn start_waiting_for_offers() -> Self {
        Self::start_listening(0, EchoMode::Remote, false).await
    }

    /// A server on the given loopback port (0 picks a free one).
    pub async fn start_on(port: u16, mode: EchoMode) -> Self {
        Self::start_listening(port, mode, true).await
    }

    /// `asks`: the device asks for the window size and terminal type.
    async fn start_listening(port: u16, mode: EchoMode, asks: bool) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let probes = Arc::<AtomicUsize>::default();
        let counted = probes.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let _ = socket.set_nodelay(true);
                tokio::spawn(serve(socket, mode, asks, counted.clone()));
            }
        });
        Self { port, task, probes }
    }

    /// How many timing marks (`DO TIMING-MARK`) it has answered, on every
    /// connection.
    pub fn probes(&self) -> usize {
        self.probes.load(Ordering::SeqCst)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// A target for this server.
    pub fn target(&self) -> TelnetTarget {
        TelnetTarget {
            host: "127.0.0.1".into(),
            port: self.port,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }
}

impl Drop for TestTelnetServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Where the device's conversation has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Username,
    Password,
    Shell,
}

/// The parser's position in the client's byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parse {
    Data,
    /// After a carriage return: a following LF or NUL belongs to it.
    Cr,
    Iac,
    Verb(u8),
    Sub,
    SubIac,
}

struct Device {
    output: mpsc::Sender<Vec<u8>>,
    parse: Parse,
    sub: Vec<u8>,
    stage: Stage,
    line: Vec<u8>,
    username: Vec<u8>,
    /// The device echoes what it receives (it said `WILL ECHO`).
    echoing: bool,
    cols: u16,
    rows: u16,
    terminal_type: String,
    /// Set while a flood runs; dropping it stops the flood.
    flood: Option<tokio::sync::oneshot::Sender<()>>,
    mode: EchoMode,
    /// The device asked for the window size and terminal type itself;
    /// otherwise it accepts the client's offers.
    asks: bool,
    /// Timing marks answered.
    probes: Arc<AtomicUsize>,
}

async fn serve(socket: TcpStream, mode: EchoMode, asks: bool, probes: Arc<AtomicUsize>) {
    let (mut reader, writer) = socket.into_split();
    // Bounded, so that a flood waits for the client like a real device.
    let (output, queued) = mpsc::channel(4);
    tokio::spawn(write_out(writer, queued));
    let mut device = Device {
        output,
        parse: Parse::Data,
        sub: Vec::new(),
        stage: Stage::Username,
        line: Vec::new(),
        username: Vec::new(),
        echoing: mode == EchoMode::Remote,
        cols: 0,
        rows: 0,
        terminal_type: String::new(),
        flood: None,
        mode,
        asks,
        probes,
    };
    let mut greeting = if asks {
        vec![IAC, DO, TTYPE, IAC, DO, NAWS]
    } else {
        Vec::new()
    };
    greeting.extend_from_slice(&[IAC, WILL, SGA]);
    if mode == EchoMode::Remote {
        greeting.extend_from_slice(&[IAC, WILL, ECHO]);
    }
    greeting.extend_from_slice(BANNER);
    greeting.extend_from_slice(USERNAME_PROMPT);
    if device.send(greeting).await.is_err() {
        return;
    }
    let mut buffer = vec![0; 4096];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        for &byte in &buffer[..read] {
            match device.receive(byte).await {
                Ok(true) => {}
                // `exit`, or the client went away.
                Ok(false) | Err(()) => return,
            }
        }
    }
}

async fn write_out(mut writer: OwnedWriteHalf, mut queued: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = queued.recv().await {
        if writer.write_all(&bytes).await.is_err() {
            return;
        }
    }
    let _ = writer.shutdown().await;
}

/// Doubles every `IAC` in data, as telnet requires.
fn escape(data: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(data.len());
    for &byte in data {
        escaped.push(byte);
        if byte == IAC {
            escaped.push(IAC);
        }
    }
    escaped
}

impl Device {
    async fn send(&self, bytes: Vec<u8>) -> Result<(), ()> {
        self.output.send(bytes).await.map_err(drop)
    }

    /// Takes one byte from the client; `false` once the connection should
    /// close.
    async fn receive(&mut self, byte: u8) -> Result<bool, ()> {
        match self.parse {
            Parse::Data => self.data(byte).await,
            Parse::Cr => {
                self.parse = Parse::Data;
                if byte == b'\n' || byte == 0 {
                    Ok(true)
                } else {
                    self.data(byte).await
                }
            }
            Parse::Iac => {
                self.parse = Parse::Data;
                match byte {
                    IAC => return self.character(IAC).await,
                    WILL | WONT | DO | DONT => self.parse = Parse::Verb(byte),
                    SB => {
                        self.sub.clear();
                        self.parse = Parse::Sub;
                    }
                    _ => {}
                }
                Ok(true)
            }
            Parse::Verb(verb) => {
                self.parse = Parse::Data;
                self.negotiate(verb, byte).await?;
                Ok(true)
            }
            Parse::Sub => {
                if byte == IAC {
                    self.parse = Parse::SubIac;
                } else {
                    self.sub.push(byte);
                }
                Ok(true)
            }
            Parse::SubIac => {
                if byte == IAC {
                    self.sub.push(IAC);
                    self.parse = Parse::Sub;
                } else {
                    self.parse = Parse::Data;
                    if byte == SE {
                        self.subnegotiation();
                    }
                }
                Ok(true)
            }
        }
    }

    async fn data(&mut self, byte: u8) -> Result<bool, ()> {
        match byte {
            IAC => {
                self.parse = Parse::Iac;
                Ok(true)
            }
            b'\r' => {
                self.parse = Parse::Cr;
                self.end_of_line().await
            }
            _ => self.character(byte).await,
        }
    }

    async fn negotiate(&mut self, verb: u8, option: u8) -> Result<(), ()> {
        match (verb, option) {
            (WILL, TTYPE) => {
                let mut ask = if self.asks {
                    Vec::new()
                } else {
                    vec![IAC, DO, TTYPE]
                };
                ask.extend_from_slice(&[IAC, SB, TTYPE, TTYPE_SEND, IAC, SE]);
                self.send(ask).await?;
            }
            (WILL, NAWS) if !self.asks => self.send(vec![IAC, DO, NAWS]).await?,
            // A timing mark: answered in order, after what came before.
            (DO, TIMING_MARK) => {
                self.send(vec![IAC, WILL, TIMING_MARK]).await?;
                self.probes.fetch_add(1, Ordering::SeqCst);
            }
            // The client refuses to let the device echo: it echoes itself.
            (DONT, ECHO) if self.echoing => {
                self.echoing = false;
                self.send(vec![IAC, WONT, ECHO]).await?;
            }
            // Acknowledgements, and options it does not care about.
            _ => {}
        }
        Ok(())
    }

    fn subnegotiation(&mut self) {
        match self.sub.as_slice() {
            [NAWS, cols_high, cols_low, rows_high, rows_low] => {
                self.cols = u16::from_be_bytes([*cols_high, *cols_low]);
                self.rows = u16::from_be_bytes([*rows_high, *rows_low]);
            }
            [TTYPE, TTYPE_IS, name @ ..] => {
                self.terminal_type = String::from_utf8_lossy(name).into_owned();
            }
            _ => {}
        }
    }

    async fn character(&mut self, byte: u8) -> Result<bool, ()> {
        if self.stage == Stage::Shell && byte == CTRL_C {
            self.line.clear();
            match self.flood.take() {
                // The flood prints the interrupt and prompt as it stops.
                Some(stop) => drop(stop),
                None => self.send([INTERRUPTED, TELNET_PROMPT].concat()).await?,
            }
            return Ok(true);
        }
        if self.flood.is_some() {
            // Input typed during a flood is ignored, like a busy device.
            return Ok(true);
        }
        self.line.push(byte);
        if self.echoing && self.stage != Stage::Password {
            self.send(escape(&[byte])).await?;
        }
        Ok(true)
    }

    async fn end_of_line(&mut self) -> Result<bool, ()> {
        if self.flood.is_some() {
            return Ok(true);
        }
        let line = std::mem::take(&mut self.line);
        if self.echoing && self.stage != Stage::Password {
            self.send(b"\r\n".to_vec()).await?;
        }
        match self.stage {
            Stage::Username => {
                self.username = line;
                self.stage = Stage::Password;
                let mut ask = Vec::new();
                if self.mode == EchoMode::Line {
                    // Takes the echo over so that the client stops its own.
                    ask.extend_from_slice(&[IAC, WILL, ECHO]);
                    self.echoing = true;
                }
                ask.extend_from_slice(PASSWORD_PROMPT);
                self.send(ask).await?;
            }
            Stage::Password => {
                let mut reply = Vec::new();
                if self.mode == EchoMode::Line {
                    reply.extend_from_slice(&[IAC, WONT, ECHO]);
                    self.echoing = false;
                }
                let accepted = self.username == USERNAME.as_bytes() && line == PASSWORD.as_bytes();
                if accepted {
                    reply.extend_from_slice(b"\r\n");
                    reply.extend_from_slice(TELNET_PROMPT);
                    self.stage = Stage::Shell;
                } else {
                    reply.extend_from_slice(LOGIN_INCORRECT);
                    reply.extend_from_slice(USERNAME_PROMPT);
                    self.stage = Stage::Username;
                }
                self.send(reply).await?;
            }
            Stage::Shell => return self.run(&line).await,
        }
        Ok(true)
    }

    /// Runs a shell command; `false` closes the connection.
    async fn run(&mut self, line: &[u8]) -> Result<bool, ()> {
        let mut reply = match line {
            b"size" => format!("{}x{}\r\n", self.cols, self.rows).into_bytes(),
            b"term" => format!("{}\r\n", self.terminal_type).into_bytes(),
            b"echo off" => {
                self.echoing = false;
                vec![IAC, WONT, ECHO]
            }
            b"echo on" => {
                self.echoing = true;
                vec![IAC, WILL, ECHO]
            }
            b"binary" => escape(b"A\xffB\r\n"),
            b"flood" => {
                self.flood = Some(start_flood(self.output.clone()));
                return Ok(true);
            }
            b"exit" => return Ok(false),
            b"" => Vec::new(),
            other => escape(&[UNKNOWN_COMMAND, other, b"\r\n"].concat()),
        };
        reply.extend_from_slice(TELNET_PROMPT);
        self.send(reply).await?;
        Ok(true)
    }
}

/// Sends [`FLOOD_LINE`] until the returned sender is dropped, then the
/// interrupt and a prompt.
fn start_flood(output: mpsc::Sender<Vec<u8>>) -> tokio::sync::oneshot::Sender<()> {
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let chunk = FLOOD_LINE.repeat(200);
        loop {
            tokio::select! {
                biased;
                _ = &mut stopped => break,
                sent = output.send(chunk.clone()) => if sent.is_err() { return },
            }
        }
        let _ = output.send([INTERRUPTED, TELNET_PROMPT].concat()).await;
    });
    stop
}
