//! XMODEM and YMODEM file transfers on a serial session (#47), against a
//! device that speaks the protocols (`support::modem`) on a virtual serial
//! port pair (`support::serial`): clean transfers both ways in each
//! variant, a damaged block that is sent again, a cancel from either side,
//! a device that falls silent, and the terminal getting the line back,
//! all through the public API only.

mod support;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use enodix_core::session::{
    ModemProtocol, OutputOptions, SerialParameters, SerialTarget, SessionEvent, SessionEvents,
    SessionId, SessionManager, SessionState, SessionTarget, TerminalSize, TransferAction,
    TransferDirection, TransferError, TransferRequest, TransferState, TransferStatus,
    TransferTimeouts,
};
use support::modem::{DropByte, Ended, Faults, ModemDevice, Role, Seen, CAN, EOT, NAK, SUB};
use support::serial::{VirtualDevice, VirtualSerialPorts};
use support::{known_hosts, next_state, output_ending_with, WAIT};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Short waits, so a device that falls silent fails a test quickly.
const QUICK: TransferTimeouts = TransferTimeouts {
    start: Duration::from_secs(5),
    reply: Duration::from_millis(300),
    retries: 3,
};

struct Console {
    _ports: VirtualSerialPorts,
    device: VirtualDevice,
    manager: SessionManager,
    id: SessionId,
    events: SessionEvents,
    folder: TempDir,
}

/// A session on `COM3`, at the device's prompt.
async fn console() -> Console {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM3", "USB Serial Port");
    let manager = SessionManager::with_serial_ports(
        known_hosts(),
        OutputOptions::default(),
        Arc::new(ports.clone()),
    );
    let target = SerialTarget {
        port: "COM3".into(),
        parameters: SerialParameters::default(),
    };
    let (id, mut events) = manager.open(SessionTarget::Serial(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    device.send(b"\r\nrommon 1 >").unwrap();
    output_ending_with(&mut events, b"rommon 1 >").await;
    Console {
        _ports: ports,
        device,
        manager,
        id,
        events,
        folder: tempfile::tempdir().unwrap(),
    }
}

/// Bytes that look like a file, never ending in the padding byte.
fn contents(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|index| ((index * 7 + usize::from(seed)) % 251) as u8)
        .map(|byte| if byte == SUB { 0x55 } else { byte })
        .collect()
}

/// What a transfer reported until it ended, and any terminal output
/// meanwhile.
struct Reported {
    statuses: Vec<TransferStatus>,
    /// Terminal output from before the transfer was in place (its first
    /// status): what the device sent before the user started it, which
    /// is the terminal's, as a device already asking to start does.
    before: Vec<u8>,
    /// Terminal output once the transfer was in place: none is promised.
    output: Vec<u8>,
}

impl Reported {
    fn last(&self) -> &TransferStatus {
        self.statuses.last().unwrap()
    }
}

async fn until_ended(events: &mut SessionEvents) -> Reported {
    let mut reported = Reported {
        statuses: Vec::new(),
        before: Vec::new(),
        output: Vec::new(),
    };
    tokio::time::timeout(WAIT * 2, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Transfer(status)) => {
                    let ended = status.state.ended();
                    reported.statuses.push(status);
                    if ended {
                        return;
                    }
                }
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    if reported.statuses.is_empty() {
                        reported.before.extend_from_slice(&bytes);
                    } else {
                        reported.output.extend_from_slice(&bytes);
                    }
                }
                other => panic!("expected the transfer's progress, got {other:?}"),
            }
        }
    })
    .await
    .expect("the transfer did not end in time");
    reported
}

async fn start(console: &Console, protocol: ModemProtocol, action: TransferAction) {
    start_with(console, protocol, action, TransferTimeouts::default()).await;
}

async fn start_with(
    console: &Console,
    protocol: ModemProtocol,
    action: TransferAction,
    timeouts: TransferTimeouts,
) {
    console
        .manager
        .start_transfer(
            console.id,
            TransferRequest {
                protocol,
                action,
                timeouts,
            },
        )
        .await
        .unwrap();
}

fn write_file(console: &Console, name: &str, data: &[u8]) -> PathBuf {
    let path = console.folder.path().join(name);
    fs::write(&path, data).unwrap();
    path
}

/// The terminal has the line again: the device's output reaches it.
async fn terminal_is_back(console: &mut Console) {
    console.device.send(b"\r\nrommon 2 >").unwrap();
    output_ending_with(&mut console.events, b"rommon 2 >").await;
}

fn received(seen: &Seen) -> &[(String, Vec<u8>)] {
    match &seen.ended {
        Ended::Done(files) => files,
        other => panic!("the device's side did not finish: {other:?}"),
    }
}

#[tokio::test]
async fn xmodem_sends_a_file_with_the_checksum_a_device_asks_for() {
    let mut console = console().await;
    let data = contents(1000, 1);
    let file = write_file(&console, "config.txt", &data);
    let device = ModemDevice::spawn(
        &console.device,
        Role::ReceiveXmodem,
        Faults {
            checksum_only: true,
            ..Faults::default()
        },
    );
    start(
        &console,
        ModemProtocol::Xmodem,
        TransferAction::Send(vec![file]),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    let mut expected = data.clone();
    expected.resize(1024, SUB);
    assert_eq!(received(&seen), [(String::new(), expected)]);
    assert!(!seen.crc);
    assert_eq!(seen.block_sizes, vec![128; 8]);
    let last = reported.last();
    assert_eq!(last.state, TransferState::Done);
    assert_eq!(last.direction, TransferDirection::Send);
    assert_eq!((last.bytes, last.blocks, last.retries), (1000, 8, 0));
    assert_eq!(last.file.as_deref(), Some("config.txt"));
    assert_eq!(last.file_size, Some(1000));
    assert_eq!(last.files_done, 1);
    // Progress as it went: block by block.
    let blocks: Vec<u64> = reported
        .statuses
        .iter()
        .map(|status| status.blocks)
        .collect();
    assert!(blocks.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(blocks.contains(&4), "progress block by block: {blocks:?}");
    // The device asks from the moment it is spawned: a request that comes
    // before the transfer is in place is the terminal's, as on a real line
    // where the user starts the send once the device is asking. Nothing
    // else may reach the terminal, and nothing once the transfer runs.
    assert!(
        reported.before.iter().all(|&byte| byte == NAK),
        "the terminal saw {:?} before the transfer",
        reported.before
    );
    assert!(
        reported.output.is_empty(),
        "the terminal saw {:?}",
        reported.output
    );
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn xmodem_1k_sends_large_blocks_with_a_crc_and_a_short_last_one() {
    let mut console = console().await;
    let data = contents(2100, 2);
    let file = write_file(&console, "image.bin", &data);
    let device = ModemDevice::spawn(&console.device, Role::ReceiveXmodem, Faults::default());
    start(
        &console,
        ModemProtocol::Xmodem1k,
        TransferAction::Send(vec![file]),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert!(seen.crc);
    // 2100 = 1024 + 1024 + 52: the end fits a 128-byte block.
    assert_eq!(seen.block_sizes, vec![1024, 1024, 128]);
    let mut expected = data.clone();
    expected.resize(2176, SUB);
    assert_eq!(received(&seen)[0].1, expected);
    assert_eq!(reported.last().state, TransferState::Done);
    assert_eq!(reported.last().bytes, 2100);
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn a_block_the_device_refuses_is_sent_again() {
    let mut console = console().await;
    let data = contents(600, 3);
    let file = write_file(&console, "config.txt", &data);
    let device = ModemDevice::spawn(
        &console.device,
        Role::ReceiveXmodem,
        Faults {
            damage_block: Some(2),
            ..Faults::default()
        },
    );
    start(
        &console,
        ModemProtocol::XmodemCrc,
        TransferAction::Send(vec![file]),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert_eq!(&received(&seen)[0].1[..600], data.as_slice());
    assert_eq!(reported.last().state, TransferState::Done);
    assert_eq!(reported.last().retries, 1);
    assert_eq!(reported.last().blocks, 5);
}

#[tokio::test]
async fn xmodem_receives_a_file_and_a_damaged_block_is_asked_for_again() {
    let mut console = console().await;
    let data = contents(3000, 4);
    let to = console.folder.path().join("running-config");
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: data.clone(),
            large: true,
        },
        Faults {
            damage_block: Some(2),
            ..Faults::default()
        },
    );
    start(
        &console,
        ModemProtocol::Xmodem1k,
        TransferAction::Receive(to.clone()),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert_eq!(seen.ended, Ended::Done(Vec::new()));
    assert!(seen.crc);
    // The padding of the last block is gone.
    assert_eq!(fs::read(&to).unwrap(), data);
    let last = reported.last();
    assert_eq!(last.state, TransferState::Done);
    assert_eq!(last.direction, TransferDirection::Receive);
    assert_eq!(last.retries, 1);
    assert_eq!(last.blocks, 3);
    assert_eq!(last.saved, vec![to]);
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn xmodem_crc_falls_back_to_the_checksum_for_an_old_sender() {
    let mut console = console().await;
    let data = contents(300, 5);
    let to = console.folder.path().join("old.txt");
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: data.clone(),
            large: false,
        },
        Faults {
            ignore_crc_requests: true,
            ..Faults::default()
        },
    );
    start_with(
        &console,
        ModemProtocol::XmodemCrc,
        TransferAction::Receive(to.clone()),
        QUICK,
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert!(!seen.crc);
    assert_eq!(reported.last().state, TransferState::Done);
    assert_eq!(fs::read(&to).unwrap(), data);
}

#[tokio::test]
async fn xmodem_crc_keeps_the_crc_once_the_sender_answered_in_it() {
    // The first block, sent in CRC, is cut short three times: asking for
    // it again, the session stays with the CRC the sender chose.
    let mut console = console().await;
    let data = contents(300, 5);
    let to = console.folder.path().join("flaky.txt");
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: data.clone(),
            large: false,
        },
        Faults {
            drop_byte: Some(DropByte {
                block: 1,
                index: 50,
                times: 3,
                resend_after: None,
            }),
            ..Faults::default()
        },
    );
    start_with(
        &console,
        ModemProtocol::XmodemCrc,
        TransferAction::Receive(to.clone()),
        QUICK,
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert_eq!(reported.last().state, TransferState::Done);
    assert!(seen.crc);
    assert_eq!(fs::read(&to).unwrap(), data);
}

/// A binary file of four 128-byte blocks whose third one begins with
/// `start`: bytes that are control bytes where a block would start.
fn binary_with(start: &[u8]) -> Vec<u8> {
    let mut data = contents(4 * 128 - 20, 10);
    data[256..256 + start.len()].copy_from_slice(start);
    data
}

/// Receives `data` with XMODEM-CRC from a device that misbehaves as
/// `faults` says, with the usual timeouts; what was reported, and what the
/// device saw.
async fn receive_xmodem(console: &mut Console, data: &[u8], faults: Faults) -> (Reported, Seen) {
    let to = console.folder.path().join("image.bin");
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: data.to_vec(),
            large: false,
        },
        faults,
    );
    start(
        console,
        ModemProtocol::XmodemCrc,
        TransferAction::Receive(to.clone()),
    )
    .await;
    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    (reported, seen)
}

#[tokio::test]
async fn a_block_whose_start_is_damaged_is_not_read_as_an_end_or_a_cancel() {
    // The SOH of block 3 arrives damaged, so its number, complement and
    // data come where a block would start: an EOT or two CANs in them are
    // the file's, not the end of the file or the device cancelling.
    for start in [&[EOT][..], &[CAN, CAN][..]] {
        let mut console = console().await;
        let data = binary_with(start);
        let (reported, seen) = receive_xmodem(
            &mut console,
            &data,
            Faults {
                damage_start: Some(3),
                ..Faults::default()
            },
        )
        .await;
        assert_eq!(reported.last().state, TransferState::Done, "{start:?}");
        assert_eq!(seen.ended, Ended::Done(Vec::new()));
        assert_eq!(
            fs::read(console.folder.path().join("image.bin")).unwrap(),
            data
        );
        assert_eq!((reported.last().blocks, reported.last().retries), (4, 1));
        terminal_is_back(&mut console).await;
    }
}

#[tokio::test]
async fn a_block_without_its_start_is_not_read_as_an_end() {
    // Block 4 loses its SOH: its number, 4, is an EOT where a block would
    // start, with the rest of the block right behind it.
    let mut console = console().await;
    let data = binary_with(&[]);
    let (reported, seen) = receive_xmodem(
        &mut console,
        &data,
        Faults {
            drop_byte: Some(DropByte {
                block: 4,
                index: 0,
                times: 1,
                resend_after: None,
            }),
            ..Faults::default()
        },
    )
    .await;
    assert_eq!(reported.last().state, TransferState::Done);
    assert_eq!(seen.ended, Ended::Done(Vec::new()));
    assert_eq!(
        fs::read(console.folder.path().join("image.bin")).unwrap(),
        data
    );
    assert_eq!(reported.last().retries, 1);
}

#[tokio::test]
async fn a_block_cut_short_and_sent_again_is_asked_for_once_the_line_is_quiet() {
    // Block 3 loses a byte and the device, not waiting for an answer,
    // sends it again: the first byte of the second copy completes the
    // first, and the rest of the copy must not be read as control bytes.
    for start in [&[EOT][..], &[CAN, CAN][..]] {
        let mut console = console().await;
        let data = binary_with(start);
        let (reported, seen) = receive_xmodem(
            &mut console,
            &data,
            Faults {
                drop_byte: Some(DropByte {
                    block: 3,
                    index: 60,
                    times: 1,
                    resend_after: Some(Duration::from_millis(200)),
                }),
                ..Faults::default()
            },
        )
        .await;
        assert_eq!(reported.last().state, TransferState::Done, "{start:?}");
        assert_eq!(seen.ended, Ended::Done(Vec::new()));
        assert_eq!(
            fs::read(console.folder.path().join("image.bin")).unwrap(),
            data
        );
        assert_eq!(reported.last().blocks, 4);
    }
}

#[tokio::test]
async fn a_ymodem_block_whose_start_is_damaged_is_not_read_as_a_cancel() {
    let mut console = console().await;
    let mut data = contents(4000, 11);
    data[2048..2050].copy_from_slice(&[CAN, CAN]);
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendYmodem {
            files: vec![("flash.bin".into(), data.clone())],
        },
        Faults {
            damage_start: Some(3),
            ..Faults::default()
        },
    );
    start(
        &console,
        ModemProtocol::Ymodem,
        TransferAction::Receive(console.folder.path().to_path_buf()),
    )
    .await;
    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert_eq!(reported.last().state, TransferState::Done);
    assert_eq!(seen.ended, Ended::Done(Vec::new()));
    assert_eq!(
        fs::read(console.folder.path().join("flash.bin")).unwrap(),
        data
    );
}

#[tokio::test]
async fn a_ymodem_receive_refuses_a_device_sending_xmodem() {
    let mut console = console().await;
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: contents(1000, 12),
            large: true,
        },
        Faults::default(),
    );
    start_with(
        &console,
        ModemProtocol::Ymodem,
        TransferAction::Receive(console.folder.path().to_path_buf()),
        QUICK,
    )
    .await;
    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert!(
        matches!(&reported.last().state, TransferState::Failed { reason } if reason.contains("XMODEM")),
        "{:?}",
        reported.last().state
    );
    // Told at its first block, not after the whole file.
    assert_eq!(seen.ended, Ended::CancelledBySession);
    assert_eq!(seen.blocks, 0);
    assert_eq!(fs::read_dir(console.folder.path()).unwrap().count(), 0);
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn ymodem_sends_a_batch_with_each_name_and_size() {
    let mut console = console().await;
    let first = contents(5000, 6);
    let second = contents(10, 7);
    let files = vec![
        write_file(&console, "c2960-lanbase.bin", &first),
        write_file(&console, "vlan.dat", &second),
    ];
    let device = ModemDevice::spawn(&console.device, Role::ReceiveYmodem, Faults::default());
    start(&console, ModemProtocol::Ymodem, TransferAction::Send(files)).await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert_eq!(
        received(&seen),
        [
            ("c2960-lanbase.bin".to_string(), first.clone()),
            ("vlan.dat".to_string(), second.clone()),
        ]
    );
    let sizes: Vec<_> = seen
        .headers
        .iter()
        .map(|(name, field)| (name.as_str(), field.split(' ').next().unwrap()))
        .collect();
    assert_eq!(sizes, [("c2960-lanbase.bin", "5000"), ("vlan.dat", "10")]);
    let last = reported.last();
    assert_eq!(last.state, TransferState::Done);
    assert_eq!((last.files_done, last.files_total), (2, Some(2)));
    assert_eq!(last.bytes, 5010);
    // The progress named each file in turn.
    let names: Vec<_> = reported
        .statuses
        .iter()
        .filter_map(|status| status.file.as_deref())
        .collect();
    assert!(names.contains(&"c2960-lanbase.bin") && names.contains(&"vlan.dat"));
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn ymodem_receives_a_batch_into_a_folder_without_replacing_anything() {
    let mut console = console().await;
    let first = contents(1500, 8);
    // Ends in padding bytes that are part of the file: the size keeps them.
    let mut second = contents(200, 9);
    second.extend_from_slice(&[SUB, SUB]);
    fs::write(console.folder.path().join("startup-config"), b"mine").unwrap();
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendYmodem {
            files: vec![
                ("startup-config".into(), first.clone()),
                ("../../escape.bin".into(), second.clone()),
            ],
        },
        Faults::default(),
    );
    start(
        &console,
        ModemProtocol::Ymodem,
        TransferAction::Receive(console.folder.path().to_path_buf()),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    let seen = device.await.unwrap();
    assert_eq!(seen.ended, Ended::Done(Vec::new()));
    let folder = console.folder.path();
    assert_eq!(fs::read(folder.join("startup-config")).unwrap(), b"mine");
    assert_eq!(fs::read(folder.join("startup-config (2)")).unwrap(), first);
    assert_eq!(fs::read(folder.join("escape.bin")).unwrap(), second);
    let last = reported.last();
    assert_eq!(last.state, TransferState::Done);
    assert_eq!(
        last.saved,
        vec![folder.join("startup-config (2)"), folder.join("escape.bin")]
    );
    assert_eq!(last.bytes, 1702);
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn cancelling_tells_the_device_and_gives_the_terminal_back_with_what_was_typed() {
    let mut console = console().await;
    let to = console.folder.path().join("big.bin");
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: contents(200_000, 10),
            large: true,
        },
        Faults::default(),
    );
    start(
        &console,
        ModemProtocol::Xmodem1k,
        TransferAction::Receive(to.clone()),
    )
    .await;

    // Typed while the transfer runs: held, not mixed into the protocol.
    console
        .manager
        .write(console.id, b"show version\r".to_vec())
        .unwrap();
    let blocks_moving = tokio::time::timeout(WAIT, async {
        loop {
            match console.events.next().await {
                Some(SessionEvent::Transfer(status)) if status.blocks >= 2 => return,
                Some(SessionEvent::Transfer(_)) => {}
                other => panic!("expected the transfer's progress, got {other:?}"),
            }
        }
    })
    .await;
    assert!(blocks_moving.is_ok(), "no blocks went through");
    console.manager.cancel_transfer(console.id).unwrap();

    let reported = until_ended(&mut console.events).await;
    assert_eq!(
        reported.last().state,
        TransferState::Cancelled { by_device: false }
    );
    let seen = device.await.unwrap();
    assert_eq!(seen.ended, Ended::CancelledBySession);
    assert!(
        seen.after.ends_with(b"show version\r"),
        "the device got {:?} after the cancel",
        String::from_utf8_lossy(&seen.after)
    );
    // The file cut short is not left behind.
    assert!(!to.exists());
    assert!(reported.last().saved.is_empty());
    terminal_is_back(&mut console).await;
}

/// The files in the test folder, sorted: nothing left over.
fn files_in(console: &Console) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(console.folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn an_xmodem_receive_replaces_a_file_only_once_it_completes() {
    let mut console = console().await;
    let to = console.folder.path().join("startup-config");
    fs::write(&to, b"the config of yesterday").unwrap();

    // Cancelled: the file there stays as it was.
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: contents(200_000, 13),
            large: true,
        },
        Faults::default(),
    );
    start(
        &console,
        ModemProtocol::Xmodem1k,
        TransferAction::Receive(to.clone()),
    )
    .await;
    tokio::time::timeout(WAIT, async {
        loop {
            match console.events.next().await {
                Some(SessionEvent::Transfer(status)) if status.blocks >= 2 => return,
                Some(SessionEvent::Transfer(_)) => {}
                other => panic!("expected the transfer's progress, got {other:?}"),
            }
        }
    })
    .await
    .expect("no blocks went through");
    console.manager.cancel_transfer(console.id).unwrap();
    let reported = until_ended(&mut console.events).await;
    assert_eq!(
        reported.last().state,
        TransferState::Cancelled { by_device: false }
    );
    assert_eq!(device.await.unwrap().ended, Ended::CancelledBySession);
    assert_eq!(fs::read(&to).unwrap(), b"the config of yesterday");
    assert_eq!(files_in(&console), ["startup-config"]);

    // Completed: it is replaced.
    let data = contents(700, 14);
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: data.clone(),
            large: false,
        },
        Faults::default(),
    );
    start(
        &console,
        ModemProtocol::XmodemCrc,
        TransferAction::Receive(to.clone()),
    )
    .await;
    let reported = until_ended(&mut console.events).await;
    assert_eq!(reported.last().state, TransferState::Done);
    assert_eq!(device.await.unwrap().ended, Ended::Done(Vec::new()));
    assert_eq!(fs::read(&to).unwrap(), data);
    assert_eq!(files_in(&console), ["startup-config"]);
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn a_ymodem_file_shorter_than_its_size_is_not_kept() {
    let mut console = console().await;
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendYmodem {
            files: vec![("short.bin".into(), contents(300, 16))],
        },
        Faults {
            overstate_size: 1000,
            ..Faults::default()
        },
    );
    start(
        &console,
        ModemProtocol::Ymodem,
        TransferAction::Receive(console.folder.path().to_path_buf()),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    drop(device);
    assert!(
        matches!(&reported.last().state, TransferState::Failed { reason } if reason.contains("short.bin ended after 1024 of its 1300 bytes")),
        "{:?}",
        reported.last().state
    );
    assert!(reported.last().saved.is_empty());
    assert!(files_in(&console).is_empty(), "{:?}", files_in(&console));
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn a_device_that_cancels_ends_the_transfer() {
    let mut console = console().await;
    let to = console.folder.path().join("cut.bin");
    let device = ModemDevice::spawn(
        &console.device,
        Role::SendXmodem {
            data: contents(5000, 11),
            large: false,
        },
        Faults {
            cancel_after: Some(3),
            ..Faults::default()
        },
    );
    start(
        &console,
        ModemProtocol::Xmodem,
        TransferAction::Receive(to.clone()),
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    assert_eq!(device.await.unwrap().ended, Ended::Cancelled);
    assert_eq!(
        reported.last().state,
        TransferState::Cancelled { by_device: true }
    );
    assert_eq!(reported.last().blocks, 3);
    assert!(!to.exists());
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn sending_to_a_device_that_falls_silent_gives_up_after_the_retries() {
    let mut console = console().await;
    let file = write_file(&console, "image.bin", &contents(5000, 12));
    let device = ModemDevice::spawn(
        &console.device,
        Role::ReceiveXmodem,
        Faults {
            silent_after: Some(2),
            ..Faults::default()
        },
    );
    start_with(
        &console,
        ModemProtocol::Xmodem,
        TransferAction::Send(vec![file]),
        QUICK,
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    assert_eq!(device.await.unwrap().ended, Ended::Silent);
    let last = reported.last();
    assert!(
        matches!(&last.state, TransferState::Failed { reason } if reason.contains("stopped answering")),
        "{:?}",
        last.state
    );
    assert_eq!(last.blocks, 2);
    assert_eq!(last.retries, u64::from(QUICK.retries) + 1);
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn receiving_from_a_device_that_never_starts_gives_up() {
    let mut console = console().await;
    let to = console.folder.path().join("nothing.bin");
    start_with(
        &console,
        ModemProtocol::Ymodem,
        TransferAction::Receive(console.folder.path().to_path_buf()),
        TransferTimeouts {
            start: Duration::from_millis(800),
            ..QUICK
        },
    )
    .await;

    let reported = until_ended(&mut console.events).await;
    assert!(
        matches!(&reported.last().state, TransferState::Failed { reason } if reason.contains("did not start sending")),
        "{:?}",
        reported.last().state
    );
    // It asked with C (CRC) until it gave up, then cancelled.
    let asked = console.device.received_ending_with(&[0x08; 8]).await;
    assert!(asked.starts_with(b"CC"), "{asked:?}");
    assert!(!to.exists());
    terminal_is_back(&mut console).await;
}

#[tokio::test]
async fn transfers_that_cannot_start_say_why() {
    let console = console().await;
    let request = |protocol, action| TransferRequest {
        protocol,
        action,
        timeouts: QUICK,
    };
    let one = write_file(&console, "a.txt", b"a");
    let two = write_file(&console, "b.txt", b"b");
    assert_eq!(
        console
            .manager
            .start_transfer(
                console.id,
                request(
                    ModemProtocol::Xmodem,
                    TransferAction::Send(vec![one.clone(), two])
                ),
            )
            .await,
        Err(TransferError::OneFileOnly("XMODEM"))
    );
    assert!(matches!(
        console
            .manager
            .start_transfer(
                console.id,
                request(
                    ModemProtocol::Ymodem,
                    TransferAction::Send(vec![console.folder.path().join("missing.bin")]),
                ),
            )
            .await,
        Err(TransferError::File { .. })
    ));
    assert!(matches!(
        console
            .manager
            .start_transfer(
                console.id,
                request(ModemProtocol::Ymodem, TransferAction::Receive(one.clone())),
            )
            .await,
        Err(TransferError::File { .. })
    ));
    // One at a time.
    console
        .manager
        .start_transfer(
            console.id,
            request(
                ModemProtocol::Xmodem,
                TransferAction::Send(vec![one.clone()]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        console
            .manager
            .start_transfer(
                console.id,
                request(ModemProtocol::Xmodem, TransferAction::Send(vec![one])),
            )
            .await,
        Err(TransferError::Running)
    );
}

#[tokio::test]
async fn a_receive_refused_while_another_runs_leaves_no_file_behind() {
    let console = console().await;
    let request = |to: PathBuf| TransferRequest {
        protocol: ModemProtocol::Xmodem,
        action: TransferAction::Receive(to),
        timeouts: QUICK,
    };
    let folder = console.folder.path();
    console
        .manager
        .start_transfer(console.id, request(folder.join("first.bin")))
        .await
        .unwrap();
    assert_eq!(
        console
            .manager
            .start_transfer(console.id, request(folder.join("second.bin")))
            .await,
        Err(TransferError::Running)
    );
    let names: Vec<_> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [".first.bin.part"]);
}

#[tokio::test]
async fn telnet_sessions_do_not_transfer_files() {
    let server = support::telnet::TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = manager.open(SessionTarget::Telnet(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    let folder = tempfile::tempdir().unwrap();
    assert_eq!(
        manager
            .start_transfer(
                id,
                TransferRequest {
                    protocol: ModemProtocol::Xmodem,
                    action: TransferAction::Receive(folder.path().join("x")),
                    timeouts: QUICK,
                },
            )
            .await,
        Err(TransferError::Unsupported)
    );
}
