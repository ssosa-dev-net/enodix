//! XMODEM and YMODEM against lrzsz (`sx`, `rx`, `sb`, `rb`), the
//! implementation most devices' boot loaders were tested with, through the
//! system's serial driver on a `socat` pseudo terminal pair (#47). Unix
//! only, and opt in, as it needs both programs installed:
//!
//! ```text
//! sudo apt install lrzsz socat
//! cargo test -p enodix-core --test serial_lrzsz -- --ignored
//! ```
//!
//! `serial_transfer.rs` covers the same ground (and the faults) against the
//! test device of `support::modem`, everywhere; this checks that both sides
//! agree with the reference.
#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use enodix_core::session::{
    ModemProtocol, SerialParameters, SerialTarget, SessionEvent, SessionManager, SessionState,
    SessionTarget, TerminalSize, TransferAction, TransferRequest, TransferState, TransferTimeouts,
};
use support::{known_hosts, next_state, WAIT};

/// A `socat` pseudo terminal pair: the app opens one end, lrzsz the other.
struct Pair {
    socat: Child,
    line: PathBuf,
    device: PathBuf,
}

impl Pair {
    fn new(folder: &Path) -> Self {
        let line = folder.join("line");
        let device = folder.join("device");
        let socat = Command::new("socat")
            .arg(format!("pty,raw,echo=0,link={}", line.display()))
            .arg(format!("pty,raw,echo=0,link={}", device.display()))
            .stderr(Stdio::null())
            .spawn()
            .expect("socat is not installed");
        for _ in 0..100 {
            if line.exists() && device.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            socat,
            line,
            device,
        }
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        let _ = self.socat.kill();
        let _ = self.socat.wait();
    }
}

/// Runs `program args` in `folder` on the device end while the session
/// transfers as `protocol` and `action` say; returns how it ended.
async fn transfer(
    folder: &Path,
    protocol: ModemProtocol,
    action: TransferAction,
    program: &str,
    args: &[&str],
) -> TransferState {
    let pair = Pair::new(folder);
    let manager = SessionManager::new(known_hosts());
    let target = SerialTarget {
        port: pair.line.display().to_string(),
        parameters: SerialParameters::default(),
    };
    let (id, mut events) = manager.open(
        SessionTarget::Serial(target),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    let device = fs::File::options()
        .read(true)
        .write(true)
        .open(&pair.device)
        .unwrap();
    let mut lrzsz = Command::new(program)
        .args(args)
        .current_dir(folder)
        .stdin(device.try_clone().unwrap())
        .stdout(device)
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("{program} (lrzsz) cannot start: {error}"));
    manager
        .start_transfer(
            id,
            TransferRequest {
                protocol,
                action,
                timeouts: TransferTimeouts::default(),
            },
        )
        .await
        .unwrap();
    let state = tokio::time::timeout(WAIT * 6, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Transfer(status)) if status.state.ended() => {
                    return status.state
                }
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                Some(_) => {}
                None => panic!("the session ended"),
            }
        }
    })
    .await
    .expect("the transfer did not end in time");
    assert!(lrzsz.wait().unwrap().success(), "{program} failed");
    state
}

fn image() -> Vec<u8> {
    (0..70_000u32)
        .map(|index| (index * 31 % 256) as u8)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs lrzsz and socat"]
async fn receives_what_lrzsz_sends() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path();
    let data = image();
    fs::write(folder.join("image.bin"), &data).unwrap();
    fs::write(folder.join("notes.txt"), b"hello\n").unwrap();

    for (protocol, args, name) in [
        (ModemProtocol::Xmodem1k, &["-k", "image.bin"][..], "1k.bin"),
        (ModemProtocol::XmodemCrc, &["image.bin"][..], "crc.bin"),
        (ModemProtocol::Xmodem, &["image.bin"][..], "sum.bin"),
    ] {
        let to = folder.join(name);
        let state = transfer(
            folder,
            protocol,
            TransferAction::Receive(to.clone()),
            "sx",
            args,
        )
        .await;
        assert_eq!(state, TransferState::Done, "{protocol:?}");
        assert_eq!(fs::read(&to).unwrap(), data, "{protocol:?}");
    }

    let inbox = folder.join("inbox");
    fs::create_dir(&inbox).unwrap();
    let state = transfer(
        folder,
        ModemProtocol::Ymodem,
        TransferAction::Receive(inbox.clone()),
        "sb",
        &["image.bin", "notes.txt"],
    )
    .await;
    assert_eq!(state, TransferState::Done);
    assert_eq!(fs::read(inbox.join("image.bin")).unwrap(), data);
    assert_eq!(fs::read(inbox.join("notes.txt")).unwrap(), b"hello\n");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs lrzsz and socat"]
async fn sends_what_lrzsz_receives() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path();
    let data = image();
    let image = folder.join("image.bin");
    let notes = folder.join("notes.txt");
    fs::write(&image, &data).unwrap();
    fs::write(&notes, b"hello\n").unwrap();

    for (protocol, args) in [
        (ModemProtocol::Xmodem1k, &["-c", "1k.bin"][..]),
        (ModemProtocol::Xmodem, &["-c", "crc.bin"][..]),
        (ModemProtocol::Xmodem, &["sum.bin"][..]),
    ] {
        let state = transfer(
            folder,
            protocol,
            TransferAction::Send(vec![image.clone()]),
            "rx",
            args,
        )
        .await;
        assert_eq!(state, TransferState::Done, "{protocol:?} {args:?}");
        // XMODEM pads the last block; the receiver keeps the padding.
        let received = fs::read(folder.join(args.last().unwrap())).unwrap();
        assert_eq!(&received[..data.len()], data.as_slice(), "{args:?}");
    }

    let outbox = folder.join("outbox");
    fs::create_dir(&outbox).unwrap();
    let state = transfer(
        &outbox,
        ModemProtocol::Ymodem,
        TransferAction::Send(vec![image, notes]),
        "rb",
        &[],
    )
    .await;
    assert_eq!(state, TransferState::Done);
    assert_eq!(fs::read(outbox.join("image.bin")).unwrap(), data);
    assert_eq!(fs::read(outbox.join("notes.txt")).unwrap(), b"hello\n");
}
