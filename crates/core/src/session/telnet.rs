//! The telnet transport (RFC 854): a TCP connection carrying the terminal's
//! bytes, with the options a terminal needs negotiated in band.
//!
//! As the connection opens, the session offers the window size and the
//! terminal type and asks the device to suppress go-ahead, as telnet
//! clients do; it also takes the device's own requests. It agrees to:
//!
//! - window size (NAWS, RFC 1073): sends the terminal size once agreed and
//!   after every resize;
//! - terminal type (RFC 1091): `xterm-256color`, as for SSH;
//! - echo (RFC 857): the device echoes if it offers to; while it does not,
//!   the session echoes typed input itself (printable characters, Enter
//!   and backspace; not control characters or escape sequences), as a
//!   telnet client does. So a device that takes the echo over to ask for a
//!   password masks it;
//! - suppress go-ahead (RFC 858), both ways.
//!
//! Everything else is refused. Data `0xFF` bytes are doubled both ways,
//! and Enter (CR) goes out as the telnet end of line, CR LF.
//!
//! Telnet has no login of its own: the device asks for a username and
//! password in its output, and the user answers in the terminal.
//!
//! Telnet has no keepalives either, so a link that goes silent (a cable
//! pulled further along, a firewall that dropped the state) is noticed two
//! ways (#133):
//!
//! - A probe: once the device has been quiet for a connect timeout, the
//!   session asks it for a timing mark (`IAC DO TIMING-MARK`, RFC 860),
//!   which a telnet server answers (`WILL` or `WONT`) once it has taken
//!   what came before. A device that has answered one and then answers
//!   nothing for about three timeouts ([`PROBES_UNANSWERED`] more probes)
//!   is lost. A device that never answers (it is not obliged to) is asked
//!   [`PROBES_BEFORE_ANSWER`] times, then left alone, never taken for
//!   dead; a device that has never sent a telnet command (a console
//!   server's raw port) is never asked, as the bytes would reach the
//!   console. Silence while reading waits (output held back) does not
//!   count: the answer would wait behind the held output.
//! - TCP keepalive on the socket, timed from the connect timeout too
//!   ([`tcp::keep_alive`]), for the devices the probe leaves alone. It
//!   notices only a link that drops packets (no acknowledgement comes),
//!   and only while the session is reading.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Notify};

use super::ssh::{self, JumpConnections};
use super::{
    tcp, CloseReason, Command, Link, OpenError, SessionState, TelnetTarget, TerminalSize, TERM,
};
use crate::known_hosts::KnownHosts;

/// The device's side of the connection: a TCP socket, or a channel the
/// last jump host forwards.
type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// Interpret as command.
const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
/// Subnegotiation begin and end.
const SB: u8 = 250;
const SE: u8 = 240;

const ECHO: u8 = 1;
const SUPPRESS_GO_AHEAD: u8 = 3;
/// The probe of a quiet link (RFC 860).
const TIMING_MARK: u8 = 6;
const TERMINAL_TYPE: u8 = 24;
const WINDOW_SIZE: u8 = 31;

/// Terminal-type subnegotiation: "here it is" and "send it".
const TERMINAL_TYPE_IS: u8 = 0;
const TERMINAL_TYPE_SEND: u8 = 1;

/// The longest subnegotiation kept; a longer one is cut (none of those the
/// session uses comes close).
const MAX_SUBNEGOTIATION: usize = 256;

/// How much is read from the device at a time.
const READ_SIZE: usize = 16 * 1024;

/// How long closing waits for input still queued to go out.
const CLOSE_FLUSH: Duration = Duration::from_secs(2);

/// How many bytes of answers to the device's requests may wait to be sent
/// before the session stops reading: a device that sends requests without
/// end and never reads the answers cannot make them pile up (#118). Typed
/// input does not count: a device may not read it until its own output has
/// been read.
const MAX_PENDING_REPLIES: usize = 64 * 1024;

/// How many times the session refuses the same option before it stops
/// answering it: a device that asks again after every refusal would keep
/// the two answering each other for ever (#118).
const MAX_REFUSALS: u8 = 3;

/// How many probes (one per connect timeout of silence) may go unanswered,
/// by a device that has answered one, before the link counts as lost: as
/// SSH's keepalives, about three timeouts of silence.
const PROBES_UNANSWERED: u32 = 2;

/// How many times a device that has never answered a probe is asked
/// before the session stops asking it.
const PROBES_BEFORE_ANSWER: u32 = 3;

/// Runs one telnet session over `link` until it ends.
pub(super) async fn run(
    target: TelnetTarget,
    known_hosts: KnownHosts,
    link: &mut Link,
) -> CloseReason {
    let (prompter, asker, replies) = link.askers();
    let connected = tokio::select! {
        connected = connect(&target, &known_hosts, prompter, asker) => connected,
        () = link.close_requested(&replies) => return CloseReason::ClosedLocally,
    };
    let (mut reader, writer, jumps) = match connected {
        Ok(connected) => connected,
        Err(error) => return CloseReason::Failed(error),
    };

    // Input is written by its own task, so reading goes on while the
    // device is slow to take input, and the other way round.
    let (to_device, queued) = mpsc::unbounded_channel();
    let replies = Arc::new(PendingReplies::default());
    let mut writing = tokio::spawn(write_out(writer, queued, replies.clone()));
    let send = |bytes: Vec<u8>| {
        let _ = to_device.send(Out { bytes, reply: 0 });
    };

    let mut telnet = Telnet::new(link.size);
    send(telnet.opening());
    link.state(SessionState::Connected);

    let flow = link.flow();
    let mut buffer = vec![0; READ_SIZE];
    // A connect timeout of silence from the device, while reading.
    let interval = target.connect_timeout.max(MIN_PROBE_INTERVAL);
    let silence = tokio::time::sleep(interval);
    tokio::pin!(silence);
    let mut probes = Probes::default();
    let reason = loop {
        tokio::select! {
            // The writer stops early only when writing failed.
            _ = &mut writing => break CloseReason::ConnectionLost,
            () = &mut silence => {
                silence.as_mut().reset(tokio::time::Instant::now() + interval);
                // While reading waits, an answer could not be read.
                let reading = flow.has_room() && replies.has_room();
                match probes.quiet(reading, telnet.speaks_telnet) {
                    Probe::Ask => send(vec![IAC, DO, TIMING_MARK]),
                    Probe::Lost => break CloseReason::ConnectionLost,
                    Probe::Wait => {}
                }
            }
            command = link.command() => match command {
                Command::Write(bytes) => {
                    let (encoded, echo) = telnet.input(&bytes);
                    send(encoded);
                    if !echo.is_empty() && !link.output(echo) {
                        break CloseReason::ClosedLocally;
                    }
                }
                Command::Resize(size) => {
                    if let Some(report) = telnet.resize(size) {
                        send(report);
                    }
                }
                // Nothing asks for these over telnet; the manager sends
                // breaks to serial sessions only.
                Command::Answer(_) | Command::HostKey(_) | Command::Break | Command::OpenChannel { .. } => {}
                Command::Close => break CloseReason::ClosedLocally,
            },
            // Output is read only while the consumer is less than a window
            // behind (flow control), and the device takes the answers to
            // its requests; commands are taken regardless.
            read = async {
                flow.room_to_read().await;
                replies.room().await;
                reader.read(&mut buffer).await
            } => match read {
                Ok(0) => break CloseReason::RemoteExited { exit_status: None },
                Ok(count) => {
                    let Received { data, replies: answers, marks } = telnet.receive(&buffer[..count]);
                    silence.as_mut().reset(tokio::time::Instant::now() + interval);
                    probes.heard(marks > 0);
                    if !answers.is_empty() {
                        replies.queued(answers.len());
                        let _ = to_device.send(Out {
                            reply: answers.len(),
                            bytes: answers,
                        });
                    }
                    if !data.is_empty() && !link.output(data) {
                        break CloseReason::ClosedLocally;
                    }
                }
                Err(_) => break CloseReason::ConnectionLost,
            },
        }
    };

    // Closing from this side lets queued input go out first, then ends the
    // connection; otherwise it is gone already.
    drop(to_device);
    if reason == CloseReason::ClosedLocally {
        let _ = tokio::time::timeout(CLOSE_FLUSH, &mut writing).await;
    }
    writing.abort();
    // Disconnects the jump hosts, after the device.
    drop(jumps);
    reason
}

/// Opens the connection to the device: directly, or through the jump hosts
/// (whose host keys and logins are checked and asked as for SSH), the last
/// of which opens it. The jump connections are kept while it is in use.
async fn connect(
    target: &TelnetTarget,
    known_hosts: &KnownHosts,
    prompter: super::Prompter,
    asker: super::HostKeyAsker,
) -> Result<(Reader, Writer, Option<JumpConnections>), OpenError> {
    if target.jump_hosts.is_empty() {
        let socket = tcp::connect(&target.host, target.port, target.connect_timeout).await?;
        // Keystrokes go out at once, not gathered (Nagle).
        let _ = socket.set_nodelay(true);
        // For a link that drops packets, if the probe cannot be used.
        let _ = tcp::keep_alive(&socket, target.connect_timeout);
        let (reader, writer) = socket.into_split();
        return Ok((Box::new(reader), Box::new(writer), None));
    }
    let jumps = ssh::open_jump_chain(&target.jump_hosts, known_hosts, asker, prompter).await?;
    let channel = jumps
        .forward(&target.host, target.port, target.connect_timeout)
        .await?;
    let (reader, writer) = tokio::io::split(channel.into_stream());
    Ok((Box::new(reader), Box::new(writer), Some(jumps)))
}

/// The shortest wait between probes, whatever the connect timeout (a zero
/// timeout must not make the session probe without pause).
const MIN_PROBE_INTERVAL: Duration = Duration::from_millis(100);

/// What to do after a connect timeout of silence from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// Ask the device for a timing mark.
    Ask,
    /// Nothing: the device is not asked (yet, or any more).
    Wait,
    /// The device stopped answering: the link is lost.
    Lost,
}

/// The probes of a quiet link, and what the device made of them.
#[derive(Debug, Default)]
struct Probes {
    /// The device has answered a probe: it can be counted on to answer.
    answers: bool,
    /// Probes sent since the device was last heard from.
    unanswered: u32,
    /// Probes sent before the device ever answered one.
    asked: u32,
}

impl Probes {
    /// A connect timeout went by without a byte from the device, `reading`
    /// or not (while reading waits, answers cannot be read, so the silence
    /// does not count); `speaks_telnet`: the device has sent a telnet
    /// command (devices that have not are never asked).
    fn quiet(&mut self, reading: bool, speaks_telnet: bool) -> Probe {
        if !reading {
            self.unanswered = 0;
            return Probe::Wait;
        }
        if self.answers {
            self.unanswered += 1;
            return if self.unanswered > PROBES_UNANSWERED {
                Probe::Lost
            } else {
                Probe::Ask
            };
        }
        if speaks_telnet && self.asked < PROBES_BEFORE_ANSWER {
            self.asked += 1;
            return Probe::Ask;
        }
        Probe::Wait
    }

    /// Something came from the device; `answer`: an answer to a probe
    /// among it.
    fn heard(&mut self, answer: bool) {
        self.unanswered = 0;
        if answer {
            self.answers = true;
        }
    }
}

/// Something to send the device: input, or answers to its requests
/// (`reply` bytes of it, counted in [`PendingReplies`]).
struct Out {
    bytes: Vec<u8>,
    reply: usize,
}

/// The answers to the device's requests not yet sent; the session reads
/// only while there are fewer than [`MAX_PENDING_REPLIES`].
#[derive(Default)]
struct PendingReplies {
    bytes: AtomicUsize,
    sent: Notify,
}

impl PendingReplies {
    fn queued(&self, bytes: usize) {
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
    }

    fn sent(&self, bytes: usize) {
        self.bytes.fetch_sub(bytes, Ordering::AcqRel);
        self.sent.notify_one();
    }

    /// Whether few enough answers wait: reading may go on now.
    fn has_room(&self) -> bool {
        self.bytes.load(Ordering::Acquire) < MAX_PENDING_REPLIES
    }

    /// Resolves once few enough answers wait. Only the session's reading
    /// waits here, so `notify_one`'s stored permit covers an answer sent
    /// between the check and the wait.
    async fn room(&self) {
        while self.bytes.load(Ordering::Acquire) >= MAX_PENDING_REPLIES {
            self.sent.notified().await;
        }
    }
}

/// Writes what is queued to the device, in order, then ends the connection
/// once the queue closes. Returns early only if writing failed.
async fn write_out(
    mut writer: Writer,
    mut queued: mpsc::UnboundedReceiver<Out>,
    replies: Arc<PendingReplies>,
) -> std::io::Result<()> {
    while let Some(Out { bytes, reply }) = queued.recv().await {
        if !bytes.is_empty() {
            writer.write_all(&bytes).await?;
        }
        if reply > 0 {
            replies.sent(reply);
        }
    }
    writer.shutdown().await
}

/// What arrived from the device, once the telnet commands are taken out.
#[derive(Debug, Default, PartialEq, Eq)]
struct Received {
    /// The device's output for the terminal.
    data: Vec<u8>,
    /// The session's answers to the device's requests.
    replies: Vec<u8>,
    /// How many answers to a probe (`WILL` or `WONT TIMING-MARK`) came.
    marks: usize,
}

/// Where the parser is in the device's byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parse {
    Data,
    /// After a carriage return: a NUL that follows is part of it.
    CarriageReturn,
    /// After `IAC`.
    Command,
    /// After `IAC` and a negotiation verb: the option comes next.
    Option(u8),
    /// Inside a subnegotiation.
    Sub,
    /// After `IAC` inside a subnegotiation.
    SubCommand,
}

/// The telnet protocol of one connection, without the connection: what
/// the device's bytes mean, and what to send.
struct Telnet {
    parse: Parse,
    sub: Vec<u8>,
    /// Whether the device echoes input (`WILL ECHO`); while it does not,
    /// the session echoes it.
    device_echo: Agreement,
    /// Whether the device suppresses go-ahead.
    device_suppress_go_ahead: Agreement,
    /// Whether the session suppresses go-ahead (it never sends one anyway).
    suppress_go_ahead: Agreement,
    /// Whether the session reports the window size.
    window_size: Agreement,
    /// Whether the session tells the terminal type.
    terminal_type: Agreement,
    size: TerminalSize,
    /// Inside an escape sequence typed while the session echoes: none of
    /// it is echoed.
    in_escape: Escape,
    /// The last input ended with a CR sent as CR LF: a LF right after it
    /// was part of the same Enter.
    after_carriage_return: bool,
    /// How many times each option the session does not take was refused,
    /// for the device's side (`WILL`) and the session's (`DO`).
    refused: [[u8; 256]; 2],
    /// The device has sent a negotiation or subnegotiation: it speaks
    /// telnet, so a probe is a telnet command to it, not bytes for a
    /// console.
    speaks_telnet: bool,
}

impl Telnet {
    fn new(size: TerminalSize) -> Self {
        Self {
            parse: Parse::Data,
            sub: Vec::new(),
            device_echo: Agreement::Off,
            device_suppress_go_ahead: Agreement::Off,
            suppress_go_ahead: Agreement::Off,
            window_size: Agreement::Off,
            terminal_type: Agreement::Off,
            size,
            in_escape: Escape::None,
            after_carriage_return: false,
            refused: [[0; 256]; 2],
            speaks_telnet: false,
        }
    }

    /// What the session offers as the connection opens: to report the
    /// window size and the terminal type, and to suppress go-ahead (as
    /// telnet clients do, for devices that wait to be offered these).
    fn opening(&mut self) -> Vec<u8> {
        self.window_size = Agreement::Asked;
        self.terminal_type = Agreement::Asked;
        self.device_suppress_go_ahead = Agreement::Asked;
        vec![
            IAC,
            WILL,
            WINDOW_SIZE,
            IAC,
            WILL,
            TERMINAL_TYPE,
            IAC,
            DO,
            SUPPRESS_GO_AHEAD,
        ]
    }

    /// Takes bytes from the device apart into output and commands, and
    /// answers the commands.
    fn receive(&mut self, bytes: &[u8]) -> Received {
        let mut received = Received::default();
        for &byte in bytes {
            self.receive_byte(byte, &mut received);
        }
        received
    }

    fn receive_byte(&mut self, byte: u8, received: &mut Received) {
        match self.parse {
            Parse::Data => self.data(byte, received),
            Parse::CarriageReturn => {
                self.parse = Parse::Data;
                // CR NUL is a bare carriage return.
                if byte != 0 {
                    self.data(byte, received);
                }
            }
            Parse::Command => {
                self.parse = Parse::Data;
                match byte {
                    IAC => received.data.push(IAC),
                    WILL | WONT | DO | DONT => {
                        self.speaks_telnet = true;
                        self.parse = Parse::Option(byte);
                    }
                    SB => {
                        self.speaks_telnet = true;
                        self.sub.clear();
                        self.parse = Parse::Sub;
                    }
                    // Go-ahead, no-operation, data mark, break and the
                    // rest mean nothing to a terminal.
                    _ => {}
                }
            }
            // An answer to a probe: nothing to answer back (RFC 860).
            Parse::Option(WILL | WONT) if byte == TIMING_MARK => {
                self.parse = Parse::Data;
                received.marks += 1;
            }
            Parse::Option(verb) => {
                self.parse = Parse::Data;
                self.negotiate(verb, byte, &mut received.replies);
            }
            Parse::Sub => {
                if byte == IAC {
                    self.parse = Parse::SubCommand;
                } else if self.sub.len() < MAX_SUBNEGOTIATION {
                    self.sub.push(byte);
                }
            }
            Parse::SubCommand => match byte {
                IAC => {
                    if self.sub.len() < MAX_SUBNEGOTIATION {
                        self.sub.push(IAC);
                    }
                    self.parse = Parse::Sub;
                }
                SE => {
                    self.parse = Parse::Data;
                    self.subnegotiation(&mut received.replies);
                }
                // A broken subnegotiation: drop it.
                _ => self.parse = Parse::Data,
            },
        }
    }

    fn data(&mut self, byte: u8, received: &mut Received) {
        match byte {
            IAC => self.parse = Parse::Command,
            b'\r' => {
                received.data.push(byte);
                self.parse = Parse::CarriageReturn;
            }
            _ => received.data.push(byte),
        }
    }

    /// Answers `verb` (`WILL`, `WONT`, `DO`, `DONT`) about `option`. Only a
    /// change of state is answered, and the answer to one of the session's
    /// own requests is not answered again, so negotiations cannot loop
    /// (RFC 854; RFC 1143's rule, without its queue).
    fn negotiate(&mut self, verb: u8, option: u8, replies: &mut Vec<u8>) {
        let (state, agree, refuse) = match verb {
            // The device offers to do something, or says it will not.
            WILL | WONT => (
                match option {
                    ECHO => Some(&mut self.device_echo),
                    SUPPRESS_GO_AHEAD => Some(&mut self.device_suppress_go_ahead),
                    _ => None,
                },
                DO,
                DONT,
            ),
            // The device asks the session to do something, or to stop.
            _ => (
                match option {
                    SUPPRESS_GO_AHEAD => Some(&mut self.suppress_go_ahead),
                    WINDOW_SIZE => Some(&mut self.window_size),
                    TERMINAL_TYPE => Some(&mut self.terminal_type),
                    // Echoing the device's output back to it, and the rest.
                    _ => None,
                },
                WILL,
                WONT,
            ),
        };
        let Some(state) = state else {
            if verb == WILL || verb == DO {
                // `WILL` asks about the device's side, `DO` about ours.
                let refused = &mut self.refused[usize::from(verb == DO)][usize::from(option)];
                if *refused < MAX_REFUSALS {
                    *refused += 1;
                    replies.extend_from_slice(&[IAC, refuse, option]);
                }
            }
            return;
        };
        let enable = verb == WILL || verb == DO;
        let before = *state;
        match (enable, before) {
            (true, Agreement::Off) => {
                *state = Agreement::On;
                replies.extend_from_slice(&[IAC, agree, option]);
            }
            (true, Agreement::Asked) => *state = Agreement::On,
            (false, Agreement::On) => {
                *state = Agreement::Off;
                replies.extend_from_slice(&[IAC, refuse, option]);
            }
            (false, Agreement::Asked) => *state = Agreement::Off,
            (true, Agreement::On) | (false, Agreement::Off) => {}
        }
        if verb == DO && option == WINDOW_SIZE && before != Agreement::On {
            replies.extend_from_slice(&self.size_report());
        }
    }

    fn subnegotiation(&mut self, replies: &mut Vec<u8>) {
        if self.terminal_type == Agreement::On && self.sub == [TERMINAL_TYPE, TERMINAL_TYPE_SEND] {
            replies.extend_from_slice(&[IAC, SB, TERMINAL_TYPE, TERMINAL_TYPE_IS]);
            replies.extend_from_slice(&escape(TERM.as_bytes()));
            replies.extend_from_slice(&[IAC, SE]);
        }
    }

    /// The window-size subnegotiation for the current size.
    fn size_report(&self) -> Vec<u8> {
        let clamp = |cells: u32| u16::try_from(cells).unwrap_or(u16::MAX).to_be_bytes();
        let [cols_high, cols_low] = clamp(self.size.cols);
        let [rows_high, rows_low] = clamp(self.size.rows);
        let mut report = vec![IAC, SB, WINDOW_SIZE];
        report.extend_from_slice(&escape(&[cols_high, cols_low, rows_high, rows_low]));
        report.extend_from_slice(&[IAC, SE]);
        report
    }

    /// The terminal has a new size: what to tell the device, if it asked.
    fn resize(&mut self, size: TerminalSize) -> Option<Vec<u8>> {
        self.size = size;
        (self.window_size == Agreement::On).then(|| self.size_report())
    }

    /// Typed input: the bytes to send the device, and what to echo to the
    /// terminal while the device does not echo itself.
    fn input(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut encoded = Vec::with_capacity(bytes.len() + 2);
        let mut echo = Vec::new();
        let local_echo = self.device_echo != Agreement::On;
        for &byte in bytes {
            let after_carriage_return = std::mem::take(&mut self.after_carriage_return);
            match byte {
                b'\n' if after_carriage_return => {}
                b'\r' => {
                    encoded.extend_from_slice(b"\r\n");
                    self.after_carriage_return = true;
                    self.in_escape = Escape::None;
                    if local_echo {
                        echo.extend_from_slice(b"\r\n");
                    }
                }
                IAC => {
                    encoded.extend_from_slice(&[IAC, IAC]);
                    if local_echo {
                        echo.push(byte);
                    }
                }
                // Backspace and delete rub the last character out.
                0x08 | 0x7f => {
                    encoded.push(byte);
                    if local_echo {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                _ => {
                    encoded.push(byte);
                    if local_echo && self.echoes(byte) {
                        echo.push(byte);
                    }
                }
            }
        }
        (encoded, echo)
    }

    /// Whether local echo shows the typed `byte`: printable characters and
    /// tabs do; control characters and escape sequences (arrow keys, which
    /// would move the cursor on screen) do not.
    fn echoes(&mut self, byte: u8) -> bool {
        match (self.in_escape, byte) {
            (_, 0x1b) => {
                self.in_escape = Escape::Start;
                false
            }
            (Escape::Start, b'[' | b'O') => {
                self.in_escape = Escape::Sequence;
                false
            }
            // Alt with a key: ESC, then the key.
            (Escape::Start, _) | (Escape::Sequence, 0x40..=0x7e) => {
                self.in_escape = Escape::None;
                false
            }
            (Escape::Sequence, _) => false,
            (Escape::None, byte) => byte == b'\t' || byte >= 0x20,
        }
    }
}

/// Where typed input is in an escape sequence (an arrow key, a function
/// key, Alt with a key), which local echo leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    /// After ESC.
    Start,
    /// In a control sequence (`ESC [` or `ESC O`), until its final byte.
    Sequence,
}

/// Where a telnet option stands between the session and the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agreement {
    Off,
    /// The session asked for it and waits for the answer.
    Asked,
    On,
}

/// Doubles every `IAC` byte, as telnet data and subnegotiations need.
fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        escaped.push(byte);
        if byte == IAC {
            escaped.push(IAC);
        }
    }
    escaped
}
