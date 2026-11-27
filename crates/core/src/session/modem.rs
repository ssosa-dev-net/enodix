//! XMODEM and YMODEM file transfers on a serial session (#47).
//!
//! The protocols are the ones consoles still speak to load an image or a
//! config over the console port: XMODEM with its checksum, its CRC-16 and
//! its 1 KiB blocks, and YMODEM, which adds a header block with the file's
//! name and size and sends several files in one batch.
//!
//! [`Transfer`] is the protocol engine. It does no input or output of its
//! own on the line: the serial transport (`serial.rs`) gives it what the
//! device sent ([`Transfer::received`]) and the time when its deadline
//! passes ([`Transfer::tick`]), and writes out what it has to send
//! ([`Transfer::take_output`]). While a transfer runs the session's output
//! belongs to it: nothing reaches the terminal, and typed input waits
//! until it ends. It reads and writes the files itself, a block at a time.
//!
//! Timing and retries follow the protocols: a receiver asks to start every
//! few seconds (`C` for CRC, NAK for the checksum, falling back from one
//! to the other), a block not answered within [`TransferTimeouts::reply`]
//! is sent again, at most [`TransferTimeouts::retries`] times in a row,
//! and either side stops the other with a run of CAN bytes.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::SessionError;

const SOH: u8 = 0x01;
const STX: u8 = 0x02;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const BS: u8 = 0x08;
const NAK: u8 = 0x15;
const CAN: u8 = 0x18;
/// What pads the last block of a file (CP/M's end of file).
const SUB: u8 = 0x1A;
/// The receiver's request for CRC-16 blocks.
const CRC_REQUEST: u8 = b'C';

/// How many CAN bytes cancel a transfer; lrzsz sends this many, then as
/// many backspaces to wipe them off a terminal that shows them.
const CANCEL_LENGTH: usize = 8;

/// How often a receiver asks the sender to start.
const REQUEST_INTERVAL: Duration = Duration::from_secs(3);

/// How many CRC requests a receiver makes before it asks for the checksum
/// instead (for XMODEM, whose senders may not know CRC).
const CRC_REQUESTS: u32 = 3;

/// How long the line must stay quiet before a receiver asks for a block
/// again, after a damaged block or bytes where a block should start: the
/// PURGE of Christensen's and Forsberg's descriptions, which read with a
/// one-second timeout until nothing comes. What arrives meanwhile is the
/// rest of a block, never control bytes.
const PURGE_QUIET: Duration = Duration::from_secs(1);

/// How long an EOT must be followed by silence to be one: a sender says
/// nothing more until it is answered, while a block that lost its SOH
/// goes on right behind its number (block 4's is the EOT byte).
const EOT_QUIET: Duration = Duration::from_millis(250);

/// How long a YMODEM sender waits for the receiver to acknowledge the
/// empty header that ends a batch, before taking the batch as done.
const END_OF_BATCH_WAIT: Duration = Duration::from_secs(2);

/// The file transfer protocols a serial session speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModemProtocol {
    /// XMODEM with 128-byte blocks. Sending, it uses the checksum or the
    /// CRC as the receiver asks; receiving, it asks for the checksum.
    Xmodem,
    /// XMODEM with 128-byte blocks and CRC-16. Receiving, it asks for the
    /// CRC and falls back to the checksum if the sender does not answer.
    XmodemCrc,
    /// XMODEM-1K: 1 KiB blocks with CRC-16 (128-byte ones for the end of a
    /// file, or when the receiver only knows the checksum).
    Xmodem1k,
    /// YMODEM batch: a header block with each file's name and size, 1 KiB
    /// blocks with CRC-16, any number of files.
    Ymodem,
}

impl ModemProtocol {
    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            Self::Xmodem => "XMODEM",
            Self::XmodemCrc => "XMODEM-CRC",
            Self::Xmodem1k => "XMODEM-1K",
            Self::Ymodem => "YMODEM",
        }
    }

    fn batch(self) -> bool {
        self == Self::Ymodem
    }
}

/// What to transfer: see [`super::SessionManager::start_transfer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRequest {
    pub protocol: ModemProtocol,
    pub action: TransferAction,
    #[serde(default)]
    pub timeouts: TransferTimeouts,
}

/// Which way a transfer goes, and its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "camelCase")]
pub enum TransferAction {
    /// Sends these files to the device: one for XMODEM, any number for
    /// YMODEM.
    Send(Vec<PathBuf>),
    /// Receives from the device into this file (XMODEM, which sends no
    /// name) or this folder (YMODEM, each file under the name the device
    /// gives, never replacing a file already there).
    Receive(PathBuf),
}

/// How long a transfer waits for the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferTimeouts {
    /// How long to wait for the other side to start: for the receiver to
    /// ask for the file, or the sender to send the first block.
    #[serde(with = "seconds")]
    pub start: Duration,
    /// How long to wait for an answer to a block, or for the next block,
    /// before sending it or asking for it again.
    #[serde(with = "seconds")]
    pub reply: Duration,
    /// How many times in a row a block is sent or asked for again before
    /// the transfer gives up.
    pub retries: u32,
}

impl Default for TransferTimeouts {
    fn default() -> Self {
        Self {
            start: Duration::from_secs(60),
            reply: Duration::from_secs(10),
            retries: 10,
        }
    }
}

mod seconds {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(value.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let seconds = f64::deserialize(deserializer)?;
        Duration::try_from_secs_f64(seconds).map_err(serde::de::Error::custom)
    }
}

/// Whether a transfer sends files or receives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    Send,
    Receive,
}

/// Where a transfer stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TransferState {
    /// Waiting for the other side to start.
    Waiting,
    /// Blocks are moving.
    Running,
    /// Every file went through.
    Done,
    /// Stopped by the user (`by_device` false) or by the device.
    #[serde(rename_all = "camelCase")]
    Cancelled { by_device: bool },
    /// Gave up: the other side stopped answering, or a file failed.
    Failed { reason: String },
}

impl TransferState {
    /// Whether the transfer has ended, one way or another.
    pub fn ended(&self) -> bool {
        !matches!(self, Self::Waiting | Self::Running)
    }
}

/// A transfer's progress, as [`super::SessionEvent::Transfer`] reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferStatus {
    pub direction: TransferDirection,
    pub protocol: ModemProtocol,
    pub state: TransferState,
    /// The file being moved now (its name, without the folder), if any.
    pub file: Option<String>,
    /// Bytes of that file moved so far.
    pub file_bytes: u64,
    /// That file's size, when known: always sending, and receiving YMODEM
    /// from a device that says.
    pub file_size: Option<u64>,
    /// Files that went through whole.
    pub files_done: usize,
    /// Files to send in all (sending only).
    pub files_total: Option<usize>,
    /// Bytes of every file moved so far.
    pub bytes: u64,
    /// Blocks that went through.
    pub blocks: u64,
    /// Blocks sent or asked for again, in all.
    pub retries: u64,
    /// The files written, receiving (in full: a file cut short by a
    /// cancel or a failure is removed).
    pub saved: Vec<PathBuf>,
}

/// Why a transfer did not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error(transparent)]
    Session(#[from] SessionError),
    /// Only serial sessions transfer files.
    #[error("files are transferred with XMODEM and YMODEM on serial sessions only")]
    Unsupported,
    /// The session runs a transfer already.
    #[error("a transfer is already running on this session")]
    Running,
    /// Sending with no file.
    #[error("there is no file to send")]
    NothingToSend,
    /// Several files with XMODEM, which has no file names.
    #[error("{0} sends one file at a time; YMODEM sends several")]
    OneFileOnly(&'static str),
    /// A file to send that cannot be read, or a place to receive into that
    /// cannot be written.
    #[error("{path}: {reason}")]
    File { path: String, reason: String },
}

fn file_error(path: &Path, reason: impl ToString) -> TransferError {
    TransferError::File {
        path: path.display().to_string(),
        reason: reason.to_string(),
    }
}

/// CRC-16/XMODEM (polynomial 0x1021, starting at 0).
pub(super) fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte))
}

/// A block as it goes on the line: SOH or STX, its number and complement,
/// `data` (128 or 1024 bytes), then the CRC or the checksum.
fn packet(number: u8, data: &[u8], crc: bool) -> Vec<u8> {
    let mut packet = Vec::with_capacity(data.len() + 5);
    packet.push(if data.len() == 1024 { STX } else { SOH });
    packet.push(number);
    packet.push(!number);
    packet.extend_from_slice(data);
    if crc {
        packet.extend_from_slice(&crc16(data).to_be_bytes());
    } else {
        packet.push(checksum(data));
    }
    packet
}

/// The cancel sequence: CAN bytes, then backspaces over them.
fn cancel_sequence() -> Vec<u8> {
    let mut bytes = vec![CAN; CANCEL_LENGTH];
    bytes.extend_from_slice(&[BS; CANCEL_LENGTH]);
    bytes
}

/// A file transfer in progress on a serial session.
pub(super) struct Transfer {
    protocol: ModemProtocol,
    timeouts: TransferTimeouts,
    side: Side,
    status: TransferStatus,
    /// What to send to the device.
    output: Vec<u8>,
    /// Whether `status` changed since it was last taken.
    changed: bool,
    /// When to call [`Transfer::tick`].
    deadline: Instant,
    /// Consecutive CAN bytes seen between blocks.
    cancels: usize,
    /// Bytes that came after the transfer ended: the terminal's again.
    leftover: Vec<u8>,
}

enum Side {
    Send(Sender),
    Receive(Box<Receiver>),
}

impl Transfer {
    /// A transfer as `request` asks, with its files opened: nothing is
    /// sent until [`Transfer::start`].
    pub(super) fn new(request: TransferRequest) -> Result<Self, TransferError> {
        let protocol = request.protocol;
        let (side, direction, files_total) = match request.action {
            TransferAction::Send(paths) => {
                if paths.is_empty() {
                    return Err(TransferError::NothingToSend);
                }
                if paths.len() > 1 && !protocol.batch() {
                    return Err(TransferError::OneFileOnly(protocol.name()));
                }
                let total = paths.len();
                let files = paths
                    .iter()
                    .map(|path| SendFile::open(path))
                    .collect::<Result<Vec<_>, _>>()?;
                (
                    Side::Send(Sender::new(files)),
                    TransferDirection::Send,
                    Some(total),
                )
            }
            TransferAction::Receive(to) => (
                Side::Receive(Box::new(Receiver::new(protocol, to)?)),
                TransferDirection::Receive,
                None,
            ),
        };
        Ok(Self {
            protocol,
            timeouts: request.timeouts,
            side,
            status: TransferStatus {
                direction,
                protocol,
                state: TransferState::Waiting,
                file: None,
                file_bytes: 0,
                file_size: None,
                files_done: 0,
                files_total,
                bytes: 0,
                blocks: 0,
                retries: 0,
                saved: Vec::new(),
            },
            output: Vec::new(),
            changed: true,
            deadline: Instant::now(),
            cancels: 0,
            leftover: Vec::new(),
        })
    }

    /// Starts: a receiver asks for the first block, a sender waits to be
    /// asked.
    pub(super) fn start(&mut self, now: Instant) {
        self.deadline = now + self.timeouts.start;
        let mut side = std::mem::replace(&mut self.side, Side::Send(Sender::new(Vec::new())));
        match &mut side {
            Side::Send(sender) => {
                sender.waiting_since = now;
                if self.protocol.batch() {
                    // A batch begins with the first file's header.
                    sender.phase = SendPhase::HeaderRequest;
                }
                if let Some(file) = sender.files.first() {
                    self.status.file = Some(file.name.clone());
                    self.status.file_size = Some(file.size);
                }
            }
            Side::Receive(receiver) => {
                receiver.asking_since = now;
                receiver.ask(self, now);
            }
        }
        self.side = side;
        self.changed = true;
    }

    /// When [`Transfer::tick`] is due.
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// What the device sent.
    pub(super) fn received(&mut self, bytes: &[u8], now: Instant) {
        let mut side = std::mem::replace(&mut self.side, Side::Send(Sender::new(Vec::new())));
        for (at, &byte) in bytes.iter().enumerate() {
            if self.ended() {
                self.leftover.extend_from_slice(&bytes[at..]);
                break;
            }
            match &mut side {
                Side::Send(sender) => sender.byte(self, byte, now),
                Side::Receive(receiver) => receiver.byte(self, byte, now),
            }
        }
        if !self.ended() {
            if let Side::Receive(receiver) = &mut side {
                receiver.after_input(self, now);
            }
        }
        self.side = side;
        self.close_if_ended();
    }

    /// The deadline passed: send or ask again, or give up.
    pub(super) fn tick(&mut self, now: Instant) {
        if self.ended() || now < self.deadline {
            return;
        }
        let mut side = std::mem::replace(&mut self.side, Side::Send(Sender::new(Vec::new())));
        match &mut side {
            Side::Send(sender) => sender.timed_out(self, now),
            Side::Receive(receiver) => receiver.timed_out(self, now),
        }
        self.side = side;
        self.close_if_ended();
    }

    /// The user stops the transfer: the device is told to stop too.
    pub(super) fn cancel(&mut self) {
        if self.ended() {
            return;
        }
        self.output.extend(cancel_sequence());
        self.end(TransferState::Cancelled { by_device: false });
        self.close_if_ended();
    }

    /// What to send to the device now.
    pub(super) fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// The status, if it changed since it was last taken.
    pub(super) fn take_status(&mut self) -> Option<TransferStatus> {
        std::mem::take(&mut self.changed).then(|| self.status.clone())
    }

    /// What the device sent after the transfer ended, for the terminal.
    pub(super) fn take_leftover(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.leftover)
    }

    pub(super) fn ended(&self) -> bool {
        self.status.state.ended()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn running(&mut self) {
        if self.status.state == TransferState::Waiting {
            self.status.state = TransferState::Running;
            self.changed = true;
        }
    }

    fn retried(&mut self) {
        self.status.retries += 1;
        self.changed = true;
    }

    fn end(&mut self, state: TransferState) {
        if !self.ended() {
            self.status.state = state;
            self.changed = true;
        }
    }

    /// Gives up, telling the device to stop.
    fn fail(&mut self, reason: impl Into<String>) {
        self.output.extend(cancel_sequence());
        self.end(TransferState::Failed {
            reason: reason.into(),
        });
    }

    /// A run of CAN bytes between blocks means the device cancelled.
    /// Returns whether `byte` was one.
    fn cancel_byte(&mut self, byte: u8) -> bool {
        if byte == CAN {
            self.cancels += 1;
            if self.cancels >= 2 {
                self.end(TransferState::Cancelled { by_device: true });
            }
            true
        } else {
            self.cancels = 0;
            false
        }
    }

    /// Once ended, a receiver removes the file it had not finished.
    fn close_if_ended(&mut self) {
        if !self.ended() {
            return;
        }
        if let Side::Receive(receiver) = &mut self.side {
            receiver.discard_unfinished();
        }
    }
}

/// A file to send.
struct SendFile {
    name: String,
    size: u64,
    modified: Option<u64>,
    file: File,
}

impl SendFile {
    fn open(path: &Path) -> Result<Self, TransferError> {
        let file = File::open(path).map_err(|error| file_error(path, error))?;
        let metadata = file.metadata().map_err(|error| file_error(path, error))?;
        if !metadata.is_file() {
            return Err(file_error(path, "not a file"));
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| file_error(path, "not a file"))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs());
        Ok(Self {
            name,
            size: metadata.len(),
            modified,
            file,
        })
    }

    /// The next `size` bytes of the file, fewer at its end.
    fn read_block(&mut self, size: usize) -> io::Result<Vec<u8>> {
        let mut data = vec![0; size];
        let mut filled = 0;
        while filled < size {
            match self.file.read(&mut data[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        data.truncate(filled);
        Ok(data)
    }

    /// The YMODEM header: the name, NUL, then the size and the
    /// modification time (octal seconds since 1970).
    fn header(&self) -> Vec<u8> {
        let mut header = self.name.as_bytes().to_vec();
        header.push(0);
        header.extend_from_slice(self.size.to_string().as_bytes());
        if let Some(modified) = self.modified {
            header.extend_from_slice(format!(" {modified:o}").as_bytes());
        }
        header
    }
}

/// What a sender waits for.
enum SendPhase {
    /// The receiver to ask for the next YMODEM header (or the empty one
    /// that ends the batch).
    HeaderRequest,
    /// The receiver to ask for the file's first block.
    DataRequest,
    /// The answer to `packet`.
    Answer { sent: Sent, packet: Vec<u8> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sent {
    Header,
    EndOfBatch,
    /// A data block with this many bytes of the file.
    Data(usize),
    Eot,
}

struct Sender {
    /// The files not yet sent; the first is the one being sent.
    files: Vec<SendFile>,
    phase: SendPhase,
    crc: bool,
    number: u8,
    /// Retries of the block now being sent.
    tries: u32,
    waiting_since: Instant,
}

impl Sender {
    fn new(files: Vec<SendFile>) -> Self {
        Self {
            files,
            phase: SendPhase::DataRequest,
            crc: false,
            number: 1,
            tries: 0,
            waiting_since: Instant::now(),
        }
    }

    fn byte(&mut self, transfer: &mut Transfer, byte: u8, now: Instant) {
        if transfer.cancel_byte(byte) {
            return;
        }
        match std::mem::replace(&mut self.phase, SendPhase::DataRequest) {
            SendPhase::HeaderRequest => {
                if byte == CRC_REQUEST || byte == NAK {
                    self.crc = byte == CRC_REQUEST;
                    transfer.running();
                    let (sent, data) = match self.files.first() {
                        Some(file) => (Sent::Header, file.header()),
                        None => (Sent::EndOfBatch, Vec::new()),
                    };
                    let size = if data.len() > 128 { 1024 } else { 128 };
                    let mut data = data;
                    data.resize(size, 0);
                    self.number = 0;
                    self.put(transfer, sent, packet(0, &data, self.crc), now);
                } else {
                    self.phase = SendPhase::HeaderRequest;
                }
            }
            SendPhase::DataRequest => {
                if byte == CRC_REQUEST || byte == NAK {
                    self.crc = byte == CRC_REQUEST;
                    transfer.running();
                    self.number = 1;
                    self.next_block(transfer, now);
                } else {
                    self.phase = SendPhase::DataRequest;
                }
            }
            SendPhase::Answer { sent, packet } => match byte {
                ACK => self.acknowledged(transfer, sent, now),
                NAK => self.again(transfer, sent, packet, now),
                // A receiver asking again for the header it did not get.
                CRC_REQUEST if sent == Sent::Header || sent == Sent::EndOfBatch => {
                    self.again(transfer, sent, packet, now);
                }
                _ => self.phase = SendPhase::Answer { sent, packet },
            },
        }
    }

    fn put(&mut self, transfer: &mut Transfer, sent: Sent, packet: Vec<u8>, now: Instant) {
        transfer.send(&packet);
        transfer.deadline = now
            + if sent == Sent::EndOfBatch {
                transfer.timeouts.reply.min(END_OF_BATCH_WAIT)
            } else {
                transfer.timeouts.reply
            };
        self.phase = SendPhase::Answer { sent, packet };
    }

    fn again(&mut self, transfer: &mut Transfer, sent: Sent, packet: Vec<u8>, now: Instant) {
        // YMODEM receivers answer the first EOT with a NAK, as a matter of
        // course: that is not a retry.
        let expected = sent == Sent::Eot && self.tries == 0 && transfer.protocol.batch();
        if !expected {
            self.tries += 1;
            transfer.retried();
            if self.tries > transfer.timeouts.retries {
                transfer.fail("the device kept refusing the file's blocks");
                return;
            }
        } else {
            self.tries += 1;
        }
        self.put(transfer, sent, packet, now);
    }

    fn acknowledged(&mut self, transfer: &mut Transfer, sent: Sent, now: Instant) {
        self.tries = 0;
        match sent {
            Sent::Header => {
                self.phase = SendPhase::DataRequest;
                self.waiting_since = now;
                transfer.deadline = now + transfer.timeouts.start;
            }
            Sent::EndOfBatch => transfer.end(TransferState::Done),
            Sent::Data(bytes) => {
                transfer.status.blocks += 1;
                transfer.status.file_bytes += bytes as u64;
                transfer.status.bytes += bytes as u64;
                transfer.changed = true;
                self.number = self.number.wrapping_add(1);
                self.next_block(transfer, now);
            }
            Sent::Eot => {
                self.files.remove(0);
                transfer.status.files_done += 1;
                transfer.changed = true;
                if transfer.protocol.batch() {
                    match self.files.first() {
                        Some(file) => {
                            transfer.status.file = Some(file.name.clone());
                            transfer.status.file_size = Some(file.size);
                        }
                        None => {
                            transfer.status.file = None;
                            transfer.status.file_size = None;
                        }
                    }
                    transfer.status.file_bytes = 0;
                    self.phase = SendPhase::HeaderRequest;
                    transfer.deadline = now + transfer.timeouts.start;
                } else {
                    transfer.end(TransferState::Done);
                }
            }
        }
    }

    /// Sends the file's next block, or EOT at its end.
    fn next_block(&mut self, transfer: &mut Transfer, now: Instant) {
        let large = self.crc
            && transfer.protocol != ModemProtocol::Xmodem
            && transfer.protocol != ModemProtocol::XmodemCrc;
        let Some(file) = self.files.first_mut() else {
            transfer.end(TransferState::Done);
            return;
        };
        let data = match file.read_block(if large { 1024 } else { 128 }) {
            Ok(data) => data,
            Err(error) => {
                let reason = format!("cannot read {}: {error}", file.name);
                transfer.fail(reason);
                return;
            }
        };
        if data.is_empty() {
            self.put(transfer, Sent::Eot, vec![EOT], now);
            return;
        }
        let bytes = data.len();
        let mut block = data;
        // What is left of a file fits a short block: the way lrzsz ends.
        block.resize(if bytes > 128 { 1024 } else { 128 }, SUB);
        let packet = packet(self.number, &block, self.crc);
        self.put(transfer, Sent::Data(bytes), packet, now);
    }

    fn timed_out(&mut self, transfer: &mut Transfer, now: Instant) {
        match std::mem::replace(&mut self.phase, SendPhase::DataRequest) {
            SendPhase::HeaderRequest | SendPhase::DataRequest => {
                transfer.fail(format!(
                    "the device did not ask for the file within {}; start its {} receive first",
                    seconds(transfer.timeouts.start),
                    transfer.protocol.name()
                ));
            }
            // lrzsz's `rb` (and receivers like it) end without
            // acknowledging the empty header: every file is through.
            SendPhase::Answer {
                sent: Sent::EndOfBatch,
                ..
            } => transfer.end(TransferState::Done),
            SendPhase::Answer { sent, packet } => {
                self.tries += 1;
                transfer.retried();
                if self.tries > transfer.timeouts.retries {
                    transfer.fail("the device stopped answering");
                } else {
                    self.put(transfer, sent, packet, now);
                }
            }
        }
    }
}

/// A file being received.
struct ReceiveFile {
    /// Where it ends up.
    path: PathBuf,
    /// Where it is written meanwhile: `path` itself for a new YMODEM
    /// file, a `.part` file beside it for XMODEM, so a file already at
    /// `path` stays as it is unless the transfer completes.
    writing: PathBuf,
    file: File,
    /// What the YMODEM header said, if anything.
    size: Option<u64>,
    written: u64,
    /// The last block, held back until the end of the file shows whether
    /// its padding must go (when the size is not known).
    last: Option<Vec<u8>>,
}

impl ReceiveFile {
    /// A new file at `path`; an error if there is one already.
    fn create(path: PathBuf, size: Option<u64>) -> io::Result<Self> {
        let file = create_new(&path)?;
        Ok(Self {
            writing: path.clone(),
            path,
            file,
            size,
            written: 0,
            last: None,
        })
    }

    /// A file that replaces `path` once complete, written beside it as
    /// `.<name>.part` (or `.<name>.2.part` and so on, if taken) meanwhile.
    fn replacing(path: PathBuf) -> io::Result<Self> {
        let folder = path.parent().unwrap_or(Path::new("."));
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file name"))?
            .to_string_lossy()
            .into_owned();
        let mut count = 1u32;
        let (writing, file) = loop {
            let part = if count == 1 {
                format!(".{name}.part")
            } else {
                format!(".{name}.{count}.part")
            };
            let writing = folder.join(part);
            match create_new(&writing) {
                Ok(file) => break (writing, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && count < 1000 => {
                    count += 1;
                }
                Err(error) => return Err(error),
            }
        };
        let size = None;
        Ok(Self {
            path,
            writing,
            file,
            size,
            written: 0,
            last: None,
        })
    }

    /// Takes a block's data; returns how many bytes of the file it held.
    fn take(&mut self, data: &[u8]) -> io::Result<u64> {
        match self.size {
            Some(size) => {
                let left = size.saturating_sub(self.written);
                let useful = data.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                self.file.write_all(&data[..useful])?;
                self.written += useful as u64;
                Ok(useful as u64)
            }
            None => {
                if let Some(last) = self.last.replace(data.to_vec()) {
                    self.file.write_all(&last)?;
                }
                self.written += data.len() as u64;
                Ok(data.len() as u64)
            }
        }
    }

    /// The end of the file: the held block without its padding, then the
    /// file in its place. On an error nothing of it is left.
    fn finish(mut self) -> Result<PathBuf, String> {
        let done = self.complete();
        drop(self.file);
        match done {
            Ok(()) => {
                if self.writing != self.path {
                    if let Err(error) = fs::rename(&self.writing, &self.path) {
                        let _ = fs::remove_file(&self.writing);
                        return Err(format!("cannot write {}: {error}", self.path.display()));
                    }
                }
                Ok(self.path)
            }
            Err(reason) => {
                let _ = fs::remove_file(&self.writing);
                Err(reason)
            }
        }
    }

    fn complete(&mut self) -> Result<(), String> {
        let failed = |error: io::Error| format!("cannot write {}: {error}", self.path.display());
        if let Some(size) = self.size {
            if self.written < size {
                return Err(format!(
                    "{} ended after {} of its {size} bytes",
                    self.path.file_name().unwrap_or_default().to_string_lossy(),
                    self.written
                ));
            }
        }
        if let Some(mut last) = self.last.take() {
            while last.last() == Some(&SUB) {
                last.pop();
            }
            self.file.write_all(&last).map_err(failed)?;
        }
        self.file.flush().map_err(failed)?;
        self.file.sync_all().map_err(failed)
    }

    /// Removes what was written of it.
    fn discard(self) {
        let writing = self.writing.clone();
        drop(self);
        let _ = fs::remove_file(writing);
    }
}

/// What a receiver expects next.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A YMODEM header (block 0).
    Header,
    /// The next data block, or EOT.
    Data,
    /// A YMODEM file's second EOT, after a NAK of the first.
    SecondEot,
}

struct Receiver {
    to: PathBuf,
    /// XMODEM: the file already created at `to`.
    xmodem_file: Option<ReceiveFile>,
    current: Option<ReceiveFile>,
    expect: Expect,
    crc: bool,
    /// The number of the next data block.
    number: u8,
    /// Bytes of the block coming in.
    buffer: Vec<u8>,
    /// Whether any block has arrived since the last request.
    started: bool,
    asking_since: Instant,
    /// Start requests made so far.
    requests: u32,
    /// Whether a block has begun to arrive since the first request: the
    /// sender has chosen the CRC or the checksum, so there is no falling
    /// back from one to the other any more.
    answered: bool,
    /// Discarding what arrives, since then, until the line is quiet; then
    /// the block is asked for again.
    purging: Option<Instant>,
    /// An EOT where a block would start, taken as one if the line stays
    /// quiet behind it.
    eot: bool,
    /// Retries in a row.
    tries: u32,
}

impl Drop for Receiver {
    /// A transfer dropped before it ran (refused because another one
    /// runs, or its session gone) leaves no file behind.
    fn drop(&mut self) {
        self.discard_unfinished();
    }
}

impl Receiver {
    fn new(protocol: ModemProtocol, to: PathBuf) -> Result<Self, TransferError> {
        let xmodem_file = if protocol.batch() {
            if !to.is_dir() {
                return Err(file_error(&to, "not a folder"));
            }
            None
        } else {
            if to.is_dir() {
                return Err(file_error(&to, "a folder, not a file"));
            }
            Some(ReceiveFile::replacing(to.clone()).map_err(|error| file_error(&to, error))?)
        };
        Ok(Self {
            to,
            xmodem_file,
            current: None,
            expect: if protocol.batch() {
                Expect::Header
            } else {
                Expect::Data
            },
            crc: protocol != ModemProtocol::Xmodem,
            number: 1,
            buffer: Vec::new(),
            started: false,
            asking_since: Instant::now(),
            requests: 0,
            answered: false,
            purging: None,
            eot: false,
            tries: 0,
        })
    }

    /// Asks the sender to start (or to send the next header).
    fn ask(&mut self, transfer: &mut Transfer, now: Instant) {
        if transfer.protocol != ModemProtocol::Ymodem
            && transfer.protocol != ModemProtocol::Xmodem
            && self.crc
            && !self.answered
            && self.requests >= CRC_REQUESTS
        {
            self.crc = false;
        }
        transfer.send(&[if self.crc { CRC_REQUEST } else { NAK }]);
        self.requests += 1;
        transfer.deadline = (now + REQUEST_INTERVAL.min(transfer.timeouts.reply))
            .min(self.asking_since + transfer.timeouts.start);
    }

    fn packet_length(&self, first: u8) -> usize {
        let data = if first == STX { 1024 } else { 128 };
        3 + data + if self.crc { 2 } else { 1 }
    }

    fn byte(&mut self, transfer: &mut Transfer, byte: u8, now: Instant) {
        if let Some(since) = self.purging {
            // The line is not quiet yet; noise that never stops is given
            // up on after the reply timeout.
            let quiet = Self::purge_quiet(transfer);
            transfer.deadline = (now + quiet).min(since + transfer.timeouts.reply + quiet);
            return;
        }
        if self.eot {
            // Not an EOT, then: a block that lost its SOH, or noise.
            self.eot = false;
            self.purge(transfer, now);
            return;
        }
        if self.buffer.is_empty() {
            if transfer.cancel_byte(byte) {
                return;
            }
            match byte {
                SOH | STX => {
                    self.answered = true;
                    self.buffer.push(byte);
                }
                EOT => {
                    self.eot = true;
                    transfer.deadline = now + EOT_QUIET.min(transfer.timeouts.reply);
                }
                // Where a block should start: one whose SOH was damaged.
                _ if self.started || self.answered => self.purge(transfer, now),
                // Noise before the first block.
                _ => {}
            }
            return;
        }
        self.buffer.push(byte);
        if self.buffer.len() == self.packet_length(self.buffer[0]) {
            let packet = std::mem::take(&mut self.buffer);
            self.packet(transfer, &packet, now);
        }
    }

    /// After a read: a block half in waits [`TransferTimeouts::reply`] for
    /// the rest.
    fn after_input(&mut self, transfer: &mut Transfer, now: Instant) {
        if !self.buffer.is_empty() {
            transfer.deadline = now + transfer.timeouts.reply;
        }
    }

    fn packet(&mut self, transfer: &mut Transfer, packet: &[u8], now: Instant) {
        let number = packet[1];
        let data_end = packet.len() - if self.crc { 2 } else { 1 };
        let data = &packet[3..data_end];
        let intact = packet[2] == !number
            && if self.crc {
                packet[data_end..] == crc16(data).to_be_bytes()
            } else {
                packet[data_end] == checksum(data)
            };
        if !intact {
            // Perhaps not even a block's length: what follows may be the
            // rest of it.
            self.purge(transfer, now);
            return;
        }
        self.started = true;
        transfer.running();
        match self.expect {
            Expect::Header if number == 0 => self.header(transfer, data, now),
            // Data with no header before it: nothing would be saved.
            Expect::Header => transfer.fail(format!(
                "the device sent block {number} where a YMODEM header was due; is it sending XMODEM?"
            )),
            Expect::Data | Expect::SecondEot if number == self.number => {
                self.data(transfer, data, now);
            }
            Expect::Data | Expect::SecondEot
                if number == self.number.wrapping_sub(1)
                    || (number == 0 && self.number == 1 && transfer.protocol.batch()) =>
            {
                // A block (or the header) again: our ACK was lost.
                self.accept_again(transfer, now);
                if number == 0 {
                    transfer.send(&[CRC_REQUEST]);
                }
            }
            Expect::Data | Expect::SecondEot => transfer.fail(format!(
                "the device sent block {number} where block {} was due",
                self.number
            )),
        }
    }

    /// Something other than a good block: discards what arrives until the
    /// line is quiet, then asks for the block again.
    fn purge(&mut self, transfer: &mut Transfer, now: Instant) {
        self.buffer.clear();
        self.eot = false;
        transfer.cancels = 0;
        self.purging = Some(now);
        transfer.deadline = now + Self::purge_quiet(transfer);
    }

    fn purge_quiet(transfer: &Transfer) -> Duration {
        PURGE_QUIET.min(transfer.timeouts.reply)
    }

    /// A damaged block: asks for it again.
    fn refuse(&mut self, transfer: &mut Transfer, now: Instant) {
        self.tries += 1;
        transfer.retried();
        if self.tries > transfer.timeouts.retries {
            transfer.fail("too many damaged blocks; is the line clean, and its speed right?");
            return;
        }
        transfer.send(&[NAK]);
        transfer.deadline = now + transfer.timeouts.reply;
    }

    fn accept_again(&mut self, transfer: &mut Transfer, now: Instant) {
        transfer.send(&[ACK]);
        transfer.deadline = now + transfer.timeouts.reply;
    }

    fn header(&mut self, transfer: &mut Transfer, data: &[u8], now: Instant) {
        self.tries = 0;
        let name_end = data
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(data.len());
        if name_end == 0 {
            // The empty header: the batch is over.
            transfer.send(&[ACK]);
            transfer.end(TransferState::Done);
            return;
        }
        let given = String::from_utf8_lossy(&data[..name_end]).into_owned();
        let size = data.get(name_end + 1..).and_then(|rest| {
            let digits: Vec<u8> = rest
                .iter()
                .copied()
                .take_while(u8::is_ascii_digit)
                .collect();
            std::str::from_utf8(&digits).ok()?.parse::<u64>().ok()
        });
        let name = safe_name(&given);
        let file = match free_file(&self.to, &name, size) {
            Ok(file) => file,
            Err(error) => {
                transfer.fail(format!(
                    "cannot write {name} in {}: {error}",
                    self.to.display()
                ));
                return;
            }
        };
        transfer.status.file = file
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        transfer.status.file_size = size;
        transfer.status.file_bytes = 0;
        transfer.changed = true;
        self.current = Some(file);
        self.number = 1;
        self.expect = Expect::Data;
        transfer.send(&[ACK, CRC_REQUEST]);
        transfer.deadline = now + transfer.timeouts.reply;
    }

    fn data(&mut self, transfer: &mut Transfer, data: &[u8], now: Instant) {
        self.tries = 0;
        let file = if transfer.protocol.batch() {
            self.current.as_mut()
        } else {
            self.xmodem_file.as_mut()
        };
        let Some(file) = file else {
            transfer.fail("a block came before the file's header");
            return;
        };
        match file.take(data) {
            Ok(bytes) => {
                transfer.status.blocks += 1;
                transfer.status.file_bytes += bytes;
                transfer.status.bytes += bytes;
                transfer.changed = true;
            }
            Err(error) => {
                let reason = format!("cannot write {}: {error}", file.path.display());
                transfer.fail(reason);
                return;
            }
        }
        self.number = self.number.wrapping_add(1);
        self.expect = Expect::Data;
        transfer.send(&[ACK]);
        transfer.deadline = now + transfer.timeouts.reply;
    }

    fn end_of_file(&mut self, transfer: &mut Transfer, now: Instant) {
        match self.expect {
            // An EOT again (our ACK was lost), or before any header.
            Expect::Header => {
                if self.started || transfer.status.files_done > 0 {
                    transfer.send(&[ACK]);
                }
            }
            Expect::Data if transfer.protocol.batch() => {
                // YMODEM: refuse the first EOT, to be sure of it.
                self.expect = Expect::SecondEot;
                transfer.send(&[NAK]);
                transfer.deadline = now + transfer.timeouts.reply;
            }
            Expect::Data | Expect::SecondEot => {
                let file = if transfer.protocol.batch() {
                    self.current.take()
                } else {
                    self.xmodem_file.take()
                };
                transfer.send(&[ACK]);
                if let Some(file) = file {
                    match file.finish() {
                        Ok(path) => {
                            transfer.status.saved.push(path);
                            transfer.status.files_done += 1;
                            transfer.changed = true;
                        }
                        Err(reason) => {
                            transfer.fail(reason);
                            return;
                        }
                    }
                }
                if transfer.protocol.batch() {
                    self.expect = Expect::Header;
                    self.started = false;
                    self.requests = 0;
                    self.asking_since = now;
                    transfer.status.file = None;
                    transfer.status.file_size = None;
                    transfer.status.file_bytes = 0;
                    self.ask(transfer, now);
                } else {
                    transfer.end(TransferState::Done);
                }
            }
        }
    }

    fn timed_out(&mut self, transfer: &mut Transfer, now: Instant) {
        if self.purging.take().is_some() {
            self.refuse(transfer, now);
            return;
        }
        if std::mem::take(&mut self.eot) {
            self.end_of_file(transfer, now);
            return;
        }
        self.buffer.clear();
        if !self.started {
            if now >= self.asking_since + transfer.timeouts.start {
                let what = if transfer.status.files_done > 0 {
                    "the device did not send the next file, nor end the batch"
                } else {
                    "the device did not start sending"
                };
                transfer.fail(format!(
                    "{what} within {}; start its {} send first",
                    seconds(transfer.timeouts.start),
                    transfer.protocol.name()
                ));
            } else {
                self.ask(transfer, now);
            }
            return;
        }
        self.tries += 1;
        transfer.retried();
        if self.tries > transfer.timeouts.retries {
            transfer.fail("the device stopped sending");
            return;
        }
        let ask = if self.expect == Expect::Header {
            CRC_REQUEST
        } else {
            NAK
        };
        transfer.send(&[ask]);
        transfer.deadline = now + transfer.timeouts.reply;
    }

    /// Removes the file cut short by a cancel or a failure.
    fn discard_unfinished(&mut self) {
        for file in [self.current.take(), self.xmodem_file.take()]
            .into_iter()
            .flatten()
        {
            file.discard();
        }
    }
}

/// A name the device gave, made safe as a file name here: its last part
/// only (no folders), without characters a file name cannot have.
fn safe_name(given: &str) -> String {
    let last = given.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let cleaned: String = last
        .chars()
        .map(|char| {
            if char.is_control() || "<>:\"|?*".contains(char) {
                '_'
            } else {
                char
            }
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return "received".to_string();
    }
    // Names Windows keeps for devices, with any extension: `NUL.txt` is
    // the null device there.
    let stem = cleaned.split('.').next().unwrap_or_default().trim_end();
    let reserved = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || matches!(
        stem.as_bytes(),
        [a, b, c, b'1'..=b'9']
            if [a, b, c].map(u8::to_ascii_uppercase) == *b"COM"
                || [a, b, c].map(u8::to_ascii_uppercase) == *b"LPT"
    );
    if reserved {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn create_new(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// A wait as the reasons say it: "60 seconds", "0.8 seconds".
fn seconds(wait: Duration) -> String {
    if wait.subsec_millis() == 0 {
        format!("{} seconds", wait.as_secs())
    } else {
        format!("{:.1} seconds", wait.as_secs_f64())
    }
}

/// Creates `name` in `folder`, or `name (2)` and so on if it is taken.
fn free_file(folder: &Path, name: &str, size: Option<u64>) -> io::Result<ReceiveFile> {
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    for count in 1u32.. {
        let candidate = if count == 1 {
            name.to_string()
        } else {
            format!("{stem} ({count}){extension}")
        };
        match ReceiveFile::create(folder.join(candidate), size) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && count < 1000 => {}
            other => return other,
        }
    }
    unreachable!("the loop returns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_is_the_xmodem_one() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
    }

    #[test]
    fn names_from_the_device_stay_in_the_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("C:\\boot\\ios.bin"), "ios.bin");
        assert_eq!(safe_name(".."), "received");
        assert_eq!(safe_name("a:b?.txt"), "a_b_.txt");
        assert_eq!(safe_name("NUL.txt"), "_NUL.txt");
        assert_eq!(safe_name("com3"), "_com3");
        assert_eq!(safe_name("COM10.bin"), "COM10.bin");
        assert_eq!(safe_name("console.log"), "console.log");
    }
}
