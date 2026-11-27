//! A serial device that speaks XMODEM and YMODEM, for the transfer tests
//! (`serial_transfer.rs`): the far end of a [`VirtualDevice`]'s line, as a
//! router's ROMMON or a switch's boot loader would be.
//!
//! It is written apart from the core's engine, from the protocol
//! descriptions (Ward Christensen's XMODEM, Chuck Forsberg's YMODEM), so a
//! mistake in one is not repeated in the other. It runs on a blocking
//! thread ([`ModemDevice::spawn`]) and can misbehave on purpose
//! ([`Faults`]): damage a block once, cancel, or fall silent.
//!
//! [`VirtualDevice`]: super::serial::VirtualDevice

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use super::serial::VirtualDevice;
use super::WAIT;

pub const SOH: u8 = 0x01;
pub const STX: u8 = 0x02;
pub const EOT: u8 = 0x04;
pub const ACK: u8 = 0x06;
pub const NAK: u8 = 0x15;
pub const CAN: u8 = 0x18;
pub const SUB: u8 = 0x1A;

/// How the device misbehaves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Faults {
    /// Damages the data block with this number (counting from 1, across
    /// the transfer) the first time it goes either way: sending, one of its
    /// bytes is flipped; receiving, it is answered with a NAK as if it came
    /// in damaged.
    pub damage_block: Option<u64>,
    /// Sends CAN CAN after this many data blocks.
    pub cancel_after: Option<u64>,
    /// Stops answering (and sending) after this many data blocks.
    pub silent_after: Option<u64>,
    /// Receiving, asks only with NAK (checksum), never `C`, and ignores a
    /// `C` from the session: a sender's checksum fall back.
    pub checksum_only: bool,
    /// Sending, answers only a NAK request: an old sender that knows
    /// nothing of CRC.
    pub ignore_crc_requests: bool,
}

/// How the device's side of a transfer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// Every file went through: the files it received (name, data), or
    /// nothing when it sent.
    Done(Vec<(String, Vec<u8>)>),
    /// The session sent CAN CAN.
    CancelledBySession,
    /// The device cancelled (`Faults::cancel_after`).
    Cancelled,
    /// The device fell silent (`Faults::silent_after`).
    Silent,
}

/// What the device saw of a transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub ended: Ended,
    /// Data blocks taken (receiving) or acknowledged (sending).
    pub blocks: u64,
    /// The size of each data block, in order (128 or 1024).
    pub block_sizes: Vec<usize>,
    /// Whether the blocks carried a CRC (else the checksum).
    pub crc: bool,
    /// The YMODEM headers, as sent or received (name, size field).
    pub headers: Vec<(String, String)>,
    /// What the device received after the transfer ended, for a short
    /// while (typed input held during it, a cancel's backspaces).
    pub after: Vec<u8>,
}

/// The device end of a line, speaking the protocols.
pub struct ModemDevice {
    line: TcpStream,
    pending: VecDeque<u8>,
    faults: Faults,
    blocks: u64,
    block_sizes: Vec<usize>,
    damaged: bool,
    crc: bool,
    headers: Vec<(String, String)>,
}

/// What the device does.
#[derive(Debug, Clone)]
pub enum Role {
    /// Receives one file with XMODEM, asking with `C` unless
    /// [`Faults::checksum_only`].
    ReceiveXmodem,
    /// Receives a YMODEM batch.
    ReceiveYmodem,
    /// Sends `data` with XMODEM, in 1 KiB blocks if `large`.
    SendXmodem { data: Vec<u8>, large: bool },
    /// Sends a YMODEM batch of (name, data).
    SendYmodem { files: Vec<(String, Vec<u8>)> },
}

impl ModemDevice {
    /// Plays `role` on `device`'s line on a thread of its own; the handle
    /// gives what it saw.
    pub fn spawn(
        device: &VirtualDevice,
        role: Role,
        faults: Faults,
    ) -> tokio::task::JoinHandle<Seen> {
        let line = device.line().expect("the device has no line");
        line.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let mut device = ModemDevice {
                line,
                pending: VecDeque::new(),
                faults,
                blocks: 0,
                block_sizes: Vec::new(),
                damaged: false,
                crc: false,
                headers: Vec::new(),
            };
            let ended = match role {
                Role::ReceiveXmodem => device.receive(false),
                Role::ReceiveYmodem => device.receive(true),
                Role::SendXmodem { data, large } => device.send_xmodem(&data, large),
                Role::SendYmodem { files } => device.send_ymodem(&files),
            };
            let after = if ended == Ended::Silent {
                Vec::new()
            } else {
                device.drain(Duration::from_millis(500))
            };
            Seen {
                ended,
                blocks: device.blocks,
                block_sizes: device.block_sizes,
                crc: device.crc,
                headers: device.headers,
                after,
            }
        })
    }

    fn write(&mut self, bytes: &[u8]) {
        self.line.write_all(bytes).expect("the device cannot write");
    }

    /// The next byte, waiting at most `limit`.
    fn byte(&mut self, limit: Duration) -> Option<u8> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(byte) = self.pending.pop_front() {
                return Some(byte);
            }
            if Instant::now() >= deadline {
                return None;
            }
            let mut buffer = [0; 2048];
            match self.line.read(&mut buffer) {
                Ok(0) => return None,
                Ok(count) => self.pending.extend(&buffer[..count]),
                Err(error) if is_timeout(&error) => {}
                Err(error) => panic!("the device cannot read: {error}"),
            }
        }
    }

    fn bytes(&mut self, count: usize) -> Vec<u8> {
        (0..count)
            .map(|_| self.byte(WAIT).expect("the block was cut short"))
            .collect()
    }

    /// Everything that arrives within `limit`.
    fn drain(&mut self, limit: Duration) -> Vec<u8> {
        let deadline = Instant::now() + limit;
        let mut received = Vec::new();
        while let Some(byte) = self.byte(deadline.saturating_duration_since(Instant::now())) {
            received.push(byte);
        }
        received
    }

    /// Whether the fault plan says to stop before data block `next`.
    fn stop_before(&mut self, next: u64) -> Option<Ended> {
        if self.faults.cancel_after == Some(next - 1) {
            self.write(&[CAN, CAN, CAN]);
            return Some(Ended::Cancelled);
        }
        if self.faults.silent_after == Some(next - 1) {
            return Some(Ended::Silent);
        }
        None
    }

    fn should_damage(&mut self, number: u64) -> bool {
        if !self.damaged && self.faults.damage_block == Some(number) {
            self.damaged = true;
            true
        } else {
            false
        }
    }

    // Receiving.

    /// Receives one XMODEM file, or a YMODEM batch.
    fn receive(&mut self, batch: bool) -> Ended {
        let ask = if self.faults.checksum_only { NAK } else { b'C' };
        self.crc = ask == b'C';
        let mut files = Vec::new();
        loop {
            // Ask for the file (or the next header) until something comes.
            let first = loop {
                self.write(&[ask]);
                if let Some(byte) = self.byte(Duration::from_millis(500)) {
                    break byte;
                }
            };
            self.pending.push_front(first);
            let (name, size) = if batch {
                let Some((number, data)) = self.block() else {
                    return Ended::CancelledBySession;
                };
                assert_eq!(number, 0, "a batch starts with its header");
                let name_end = data.iter().position(|&byte| byte == 0).unwrap();
                let name = String::from_utf8(data[..name_end].to_vec()).unwrap();
                self.write(&[ACK]);
                if name.is_empty() {
                    return Ended::Done(files);
                }
                let rest = &data[name_end + 1..];
                let field_end = rest.iter().position(|&byte| byte == 0).unwrap();
                let field = String::from_utf8(rest[..field_end].to_vec()).unwrap();
                let size: usize = field.split(' ').next().unwrap().parse().unwrap();
                self.headers.push((name.clone(), field));
                self.write(b"C");
                (name, Some(size))
            } else {
                (String::new(), None)
            };
            let mut data = Vec::new();
            let mut number: u8 = 1;
            let mut eots = 0;
            loop {
                if eots == 0 {
                    if let Some(ended) = self.stop_before(self.blocks + 1) {
                        return ended;
                    }
                }
                match self.byte(WAIT).expect("the session stopped sending") {
                    EOT if batch && eots == 0 => {
                        eots = 1;
                        self.write(&[NAK]);
                    }
                    EOT => {
                        self.write(&[ACK]);
                        break;
                    }
                    first => {
                        self.pending.push_front(first);
                        let Some((got, block)) = self.block() else {
                            return Ended::CancelledBySession;
                        };
                        if got == number.wrapping_sub(1) {
                            self.write(&[ACK]);
                            continue;
                        }
                        assert_eq!(got, number, "blocks out of order");
                        if self.should_damage(self.blocks + 1) {
                            self.write(&[NAK]);
                            continue;
                        }
                        self.blocks += 1;
                        self.block_sizes.push(block.len());
                        data.extend_from_slice(&block);
                        number = number.wrapping_add(1);
                        self.write(&[ACK]);
                    }
                }
            }
            if let Some(size) = size {
                data.truncate(size);
            }
            files.push((name, data));
            if !batch {
                return Ended::Done(files);
            }
        }
    }

    /// Reads one block: its number and data, checked. `None` if the
    /// session cancelled instead.
    fn block(&mut self) -> Option<(u8, Vec<u8>)> {
        loop {
            let first = self.byte(WAIT).expect("the session sent no block");
            let size = match first {
                SOH => 128,
                STX => 1024,
                CAN => {
                    assert_eq!(self.byte(WAIT), Some(CAN), "a lone CAN");
                    return None;
                }
                // A start request still on its way, or noise.
                _ => continue,
            };
            let header = self.bytes(2);
            let data = self.bytes(size);
            assert_eq!(header[0], !header[1], "a block number and its complement");
            if self.crc {
                let check = self.bytes(2);
                assert_eq!(
                    u16::from_be_bytes([check[0], check[1]]),
                    crc16(&data),
                    "the block's CRC"
                );
            } else {
                let check = self.bytes(1)[0];
                assert_eq!(check, checksum(&data), "the block's checksum");
            }
            return Some((header[0], data));
        }
    }

    // Sending.

    /// Waits for the receiver's request: `C` (CRC) or NAK (checksum).
    /// `None` if it cancelled.
    fn request(&mut self) -> Option<bool> {
        let deadline = Instant::now() + WAIT;
        let mut cans = 0;
        loop {
            assert!(Instant::now() < deadline, "the session never asked");
            match self.byte(WAIT)? {
                b'C' if !self.faults.ignore_crc_requests => return Some(true),
                NAK => return Some(false),
                CAN => {
                    cans += 1;
                    if cans == 2 {
                        return None;
                    }
                }
                _ => cans = 0,
            }
        }
    }

    /// Sends a block until it is acknowledged. `false` if the session
    /// cancelled.
    fn put(&mut self, number: u8, data: &[u8], counted: bool) -> bool {
        let damage = counted && self.should_damage(self.blocks + 1);
        let mut packet = vec![if data.len() == 1024 { STX } else { SOH }, number, !number];
        packet.extend_from_slice(data);
        if self.crc {
            packet.extend_from_slice(&crc16(data).to_be_bytes());
        } else {
            packet.push(checksum(data));
        }
        let mut first = true;
        loop {
            let mut sent = packet.clone();
            if damage && first {
                sent[10] ^= 0xFF;
            }
            first = false;
            self.write(&sent);
            let mut cans = 0;
            loop {
                match self
                    .byte(WAIT)
                    .expect("the session never answered the block")
                {
                    ACK => {
                        if counted {
                            self.blocks += 1;
                            self.block_sizes.push(data.len());
                        }
                        return true;
                    }
                    NAK => break,
                    CAN => {
                        cans += 1;
                        if cans == 2 {
                            return false;
                        }
                    }
                    // A start request repeated before the block arrived.
                    _ => {}
                }
            }
        }
    }

    /// Sends `data` in blocks from block 1, then EOT (twice for YMODEM).
    fn put_file(&mut self, data: &[u8], large: bool, batch: bool) -> Option<Ended> {
        let size = if large && self.crc { 1024 } else { 128 };
        for (index, chunk) in data.chunks(size).enumerate() {
            if let Some(ended) = self.stop_before(self.blocks + 1) {
                return Some(ended);
            }
            let mut block = chunk.to_vec();
            block.resize(size, SUB);
            let number = (index as u64 + 1) as u8;
            if !self.put(number, &block, true) {
                return Some(Ended::CancelledBySession);
            }
        }
        self.write(&[EOT]);
        let mut answer = self.byte(WAIT).expect("no answer to EOT");
        if batch {
            assert_eq!(answer, NAK, "YMODEM refuses the first EOT");
            self.write(&[EOT]);
            answer = self.byte(WAIT).expect("no answer to the second EOT");
        }
        assert_eq!(answer, ACK, "EOT acknowledged");
        None
    }

    fn send_xmodem(&mut self, data: &[u8], large: bool) -> Ended {
        let Some(crc) = self.request() else {
            return Ended::CancelledBySession;
        };
        self.crc = crc;
        self.put_file(data, large, false)
            .unwrap_or(Ended::Done(Vec::new()))
    }

    fn send_ymodem(&mut self, files: &[(String, Vec<u8>)]) -> Ended {
        for (name, data) in files {
            let Some(crc) = self.request() else {
                return Ended::CancelledBySession;
            };
            self.crc = crc;
            let field = data.len().to_string();
            let mut header = name.as_bytes().to_vec();
            header.push(0);
            header.extend_from_slice(field.as_bytes());
            header.resize(128, 0);
            self.headers.push((name.clone(), field));
            if !self.put(0, &header, false) {
                return Ended::CancelledBySession;
            }
            if self.request().is_none() {
                return Ended::CancelledBySession;
            }
            if let Some(ended) = self.put_file(data, true, true) {
                return ended;
            }
        }
        if self.request().is_none() {
            return Ended::CancelledBySession;
        }
        if !self.put(0, &[0; 128], false) {
            return Ended::CancelledBySession;
        }
        Ended::Done(Vec::new())
    }
}

/// CRC-16/XMODEM, bit by bit.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u32 = 0;
    for &byte in data {
        for bit in (0..8).rev() {
            let input = u32::from((byte >> bit) & 1);
            let top = (crc >> 15) & 1;
            crc = (crc << 1) & 0xFFFF;
            if top ^ input == 1 {
                crc ^= 0x1021;
            }
        }
    }
    crc as u16
}

pub fn checksum(data: &[u8]) -> u8 {
    (data.iter().map(|&byte| u32::from(byte)).sum::<u32>() % 256) as u8
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}
