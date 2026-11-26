//! Character encodings (#24): a setting, inherited like the others, for
//! devices that send ISO 8859-1, Windows-1252 or CP437 (the line drawing of
//! old menus). The terminal and the log get UTF-8; what the user types goes
//! out in the device's encoding.

mod support;

use std::fs;

use enodix_core::logging::{LogFolder, SessionLog};
use enodix_core::session::{
    Encoding, OpenOptions, SessionEvents, SessionId, SessionManager, SessionState, SessionTarget,
    TerminalSize,
};
use enodix_core::workspace::{Effective, HostId, Origin, Workspace};
use support::telnet::{TestTelnetServer, PASSWORD_PROMPT, TELNET_PROMPT, USERNAME_PROMPT};
use support::{next_state, output_ending_with, PASSWORD, USERNAME};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

#[test]
fn utf_8_passes_through_unchanged() {
    let text = "héllo ╔═╗".as_bytes();

    assert_eq!(Encoding::Utf8.decode(text), text);
    assert_eq!(Encoding::Utf8.encode(text), text);
}

#[test]
fn iso_8859_1_bytes_become_utf_8_and_back() {
    assert_eq!(Encoding::Iso8859_1.decode(b"caf\xe9"), "café".as_bytes());
    assert_eq!(Encoding::Iso8859_1.encode("café".as_bytes()), b"caf\xe9");
}

#[test]
fn windows_1252_has_its_own_punctuation_where_iso_has_controls() {
    // 0x80 is the euro sign, 0x93 and 0x94 the curly quotes.
    assert_eq!(
        Encoding::Windows1252.decode(b"\x80 \x93x\x94"),
        "€ “x”".as_bytes()
    );
    assert_eq!(
        Encoding::Windows1252.encode("€ “x” é".as_bytes()),
        b"\x80 \x93x\x94 \xe9"
    );
}

#[test]
fn cp437_draws_boxes() {
    assert_eq!(Encoding::Cp437.decode(b"\xc9\xcd\xbb"), "╔═╗".as_bytes());
    assert_eq!(
        Encoding::Cp437.encode("╔═╗ é".as_bytes()),
        b"\xc9\xcd\xbb \x82"
    );
}

#[test]
fn ascii_and_escape_sequences_are_never_touched() {
    let sequence = b"\x1b[1;32mgreen\x1b[0m\r\n";
    for encoding in [Encoding::Iso8859_1, Encoding::Windows1252, Encoding::Cp437] {
        assert_eq!(encoding.decode(sequence), sequence);
        assert_eq!(encoding.encode(sequence), sequence);
    }
}

#[test]
fn a_character_the_encoding_lacks_is_sent_as_a_question_mark() {
    assert_eq!(Encoding::Iso8859_1.encode("a→b".as_bytes()), b"a?b");
}

#[test]
fn bytes_that_are_not_utf_8_are_sent_as_they_are() {
    assert_eq!(Encoding::Cp437.encode(b"\x1b[M\xff\xfe"), b"\x1b[M\xff\xfe");
}

#[test]
fn every_byte_survives_a_round_trip_in_every_encoding() {
    for encoding in [Encoding::Iso8859_1, Encoding::Windows1252, Encoding::Cp437] {
        let all: Vec<u8> = (0..=255).collect();
        let text = encoding.decode(&all);
        let back = encoding.encode(&text);
        // Windows-1252 leaves five bytes undefined (0x81, 0x8d, 0x8f, 0x90,
        // 0x9d): they stand for the controls of the same number.
        assert_eq!(back, all, "{encoding:?}");
    }
}

fn workspace() -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let write = |path: &str, text: &str| {
        let path = temp.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("Old/_folder.yaml", "encoding: cp437\n");
    write("Old/a.yaml", "address: 10.0.0.1\n");
    write("Old/b.yaml", "address: 10.0.0.2\nencoding: iso-8859-1\n");
    write("c.yaml", "address: 10.0.0.3\n");
    let workspace = Workspace::open(temp.path()).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[test]
fn the_encoding_inherits_like_the_other_settings() {
    let (_temp, workspace) = workspace();
    let encoding = |id: &str| {
        workspace
            .resolve_host(&HostId::new(id))
            .unwrap()
            .settings
            .encoding
    };

    assert_eq!(
        encoding("Old/a.yaml"),
        Effective {
            value: Encoding::Cp437,
            origin: Origin::Folder { path: "Old".into() }
        }
    );
    assert_eq!(
        encoding("Old/b.yaml"),
        Effective {
            value: Encoding::Iso8859_1,
            origin: Origin::Host
        }
    );
    assert_eq!(
        encoding("c.yaml"),
        Effective {
            value: Encoding::Utf8,
            origin: Origin::Default
        }
    );
}

#[test]
fn an_unknown_encoding_is_reported_for_that_file() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("a.yaml"),
        "address: 10.0.0.1\nencoding: ebcdic\n",
    )
    .unwrap();
    let workspace = Workspace::open(temp.path()).unwrap();

    let problems = workspace.problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].message.contains("encoding"),
        "{}",
        problems[0].message
    );
}

async fn logged_in(
    manager: &SessionManager,
    server: &TestTelnetServer,
    options: OpenOptions,
) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open_with(SessionTarget::Telnet(server.target()), SIZE, options);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, USERNAME_PROMPT).await;
    manager
        .write(id, format!("{USERNAME}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, PASSWORD_PROMPT).await;
    manager
        .write(id, format!("{PASSWORD}\r").into_bytes())
        .unwrap();
    output_ending_with(&mut events, TELNET_PROMPT).await;
    (id, events)
}

fn with(encoding: Encoding) -> OpenOptions {
    OpenOptions {
        encoding,
        ..OpenOptions::default()
    }
}

#[tokio::test]
async fn the_terminal_gets_utf_8_from_a_device_that_sends_latin_1() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server, with(Encoding::Iso8859_1)).await;

    manager.write(id, b"latin1\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, TELNET_PROMPT).await;

    assert_eq!(
        output,
        [
            b"latin1\r\n".as_slice(),
            "café\r\n".as_bytes(),
            TELNET_PROMPT
        ]
        .concat()
    );
}

#[tokio::test]
async fn a_cp437_menu_arrives_as_box_drawing() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server, with(Encoding::Cp437)).await;

    manager.write(id, b"box\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, TELNET_PROMPT).await;

    assert_eq!(
        output,
        [b"box\r\n".as_slice(), "╔═╗\r\n".as_bytes(), TELNET_PROMPT].concat()
    );
}

#[tokio::test]
async fn what_is_typed_reaches_the_device_in_its_encoding() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let (id, mut events) = logged_in(&manager, &server, with(Encoding::Iso8859_1)).await;

    manager.write(id, "hex é".as_bytes().to_vec()).unwrap();
    manager.write(id, b"\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, TELNET_PROMPT).await;
    let text = String::from_utf8_lossy(&output);

    // The device received one byte, 0xe9, not the two of UTF-8.
    assert!(text.contains("e9 \r\n"), "{text}");
    assert!(!text.contains("c3 a9"), "{text}");
}

#[tokio::test]
async fn the_log_is_utf_8_too() {
    let server = TestTelnetServer::start().await;
    let manager = support::manager();
    let log_root = TempDir::new().unwrap();
    let logs = LogFolder::new(log_root.path());
    let log: SessionLog = logs.quick_log("device");
    let folder = log.folder();
    let options = OpenOptions {
        log: Some(log),
        encoding: Encoding::Iso8859_1,
        ..OpenOptions::default()
    };
    let (id, mut events) = logged_in(&manager, &server, options).await;
    manager.write(id, b"latin1\r".to_vec()).unwrap();
    output_ending_with(&mut events, TELNET_PROMPT).await;
    manager.close(id).unwrap();
    loop {
        if matches!(next_state(&mut events).await, SessionState::Closed(_)) {
            break;
        }
    }

    let file = fs::read_dir(folder)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = fs::read_to_string(file).unwrap();
    assert!(text.contains("café"), "{text}");
}
