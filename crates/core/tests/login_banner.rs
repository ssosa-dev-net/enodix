//! The SSH login banner (#134): the text a device sends before the login
//! (`banner login` on IOS, `/etc/issue.net` on Linux) is a terminal
//! session's first output, with its line ends made right for a terminal,
//! before any login question, also when the login then fails, again on
//! every connection, logged like any output; a jump host's is named. A
//! session opened without `login_banner` (as jobs open theirs) shows none.

mod support;

use std::fs;

use netterm_core::known_hosts::KnownHosts;
use netterm_core::logging::LogFolder;
use netterm_core::session::{
    CloseReason, HostKeyDecision, JumpHost, LoginPrompt, OpenError, OpenOptions, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAuth, SshTarget,
    TerminalSize,
};
use support::{
    next_state, output_ending_with, TestSshServer, CODE_QUESTION, ONE_TIME_CODE, OTP_USERNAME,
    PASSWORD, PROMPT, USERNAME, WAIT,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A banner as devices write them: bare line ends, a control sequence, a
/// line already ended for a terminal, and no line end at the very end.
const BANNER: &str = "AUTHORIZED ACCESS ONLY\n\nDisconnect now if you are not\n\x1b[1mauthorized\x1b[0m.\r\nAll access is logged.";

/// [`BANNER`] as the terminal is given it.
const SHOWN: &[u8] = b"AUTHORIZED ACCESS ONLY\r\n\r\nDisconnect now if you are not\r\n\x1b[1mauthorized\x1b[0m.\r\nAll access is logged.\r\n";

/// Opens a terminal session to `target`: one that shows login banners.
fn open_terminal(manager: &SessionManager, target: SshTarget) -> (SessionId, SessionEvents) {
    manager.open_with(SessionTarget::Ssh(target), SIZE, terminal())
}

fn terminal() -> OpenOptions {
    OpenOptions {
        login_banner: true,
        ..OpenOptions::default()
    }
}

/// What the session showed while it was being opened, and how the
/// opening ended: the first event that is not output (a question, or a
/// state).
async fn shown_while_opening(events: &mut SessionEvents) -> (Vec<u8>, SessionEvent) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    let mut shown = Vec::new();
    let event = tokio::time::timeout(WAIT, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    shown.extend_from_slice(&bytes);
                }
                Some(SessionEvent::Algorithms(_)) => {}
                Some(event) => return event,
                None => panic!("the events ended while the session was opening"),
            }
        }
    })
    .await
    .expect("timed out while the session was opening");
    (shown, event)
}

fn prompt(event: SessionEvent) -> LoginPrompt {
    match event {
        SessionEvent::Prompt(prompt) => prompt,
        other => panic!("expected a login prompt, got {other:?}"),
    }
}

fn failure(event: SessionEvent) -> OpenError {
    match event {
        SessionEvent::State(SessionState::Closed(CloseReason::Failed(error))) => error,
        other => panic!("expected the login to fail, got {other:?}"),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn jump_host(name: &str, server: &TestSshServer) -> JumpHost {
    let target = server.target();
    JumpHost {
        name: name.into(),
        host: target.host,
        port: target.port,
        username: target.username,
        auth: target.auth,
        legacy_crypto: false,
        connect_timeout: target.connect_timeout,
    }
}

#[tokio::test]
async fn the_banner_is_a_terminal_sessions_first_output_with_its_line_ends_made_right() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();

    let (_id, mut events) = open_terminal(&manager, server.target());
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), text(SHOWN));
    assert!(
        matches!(event, SessionEvent::State(SessionState::Connected)),
        "{event:?}"
    );
    // Then the shell, as without a banner.
    assert_eq!(output_ending_with(&mut events, PROMPT).await, PROMPT);
}

/// The long banner of #118, full of control sequences, arrives whole and
/// untouched (its line ends were already a terminal's).
#[tokio::test]
async fn a_long_banner_with_control_sequences_is_shown_whole() {
    let server = TestSshServer::start().await;
    let banner = "\x1b[2J\x1b[1;31mAUTHORIZED ACCESS ONLY\x1b[0m\r\n".repeat(1_500);
    server.quirks().auth_banner(&banner);
    let manager = support::manager();

    let (_id, mut events) = open_terminal(&manager, server.target());
    let (shown, event) = shown_while_opening(&mut events).await;

    assert!(shown == banner.as_bytes(), "the banner was changed");
    assert!(
        matches!(event, SessionEvent::State(SessionState::Connected)),
        "{event:?}"
    );
}

#[tokio::test]
async fn without_the_option_the_banner_is_left_out_as_jobs_need() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();

    let (_id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), "");
    assert!(
        matches!(event, SessionEvent::State(SessionState::Connected)),
        "{event:?}"
    );
    assert_eq!(output_ending_with(&mut events, PROMPT).await, PROMPT);
}

#[tokio::test]
async fn a_device_without_a_banner_shows_nothing_before_its_shell() {
    let server = TestSshServer::start().await;
    let manager = support::manager();

    let (_id, mut events) = open_terminal(&manager, server.target());
    let (shown, _) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), "");
}

/// The banner is often the notice the user agrees to by logging in: it is
/// shown before the password is asked for.
#[tokio::test]
async fn the_banner_comes_before_the_password_question() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();
    let target = SshTarget {
        auth: SshAuth::Prompt,
        ..server.target()
    };

    let (id, mut events) = open_terminal(&manager, target);
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), text(SHOWN));
    let asked = prompt(event);
    assert_eq!(asked.questions.len(), 1, "{asked:?}");
    assert_eq!(asked.questions[0].text, "Password: ");
    manager.answer(id, vec![PASSWORD.into()]).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    assert_eq!(output_ending_with(&mut events, PROMPT).await, PROMPT);
}

#[tokio::test]
async fn the_banner_comes_before_a_keyboard_interactive_question() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();
    let target = SshTarget {
        username: OTP_USERNAME.into(),
        ..server.target()
    };

    let (id, mut events) = open_terminal(&manager, target);
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), text(SHOWN));
    let asked = prompt(event);
    assert_eq!(asked.questions[0].text, CODE_QUESTION);
    manager.answer(id, vec![ONE_TIME_CODE.into()]).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
}

/// A banner often says why a login is refused (who may log in).
#[tokio::test]
async fn the_banner_is_shown_for_a_login_that_fails() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();
    let target = SshTarget {
        auth: SshAuth::Password("wrong".into()),
        ..server.target()
    };

    let (_id, mut events) = open_terminal(&manager, target);
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), text(SHOWN));
    assert_eq!(failure(event), OpenError::AuthenticationFailed);
}

#[tokio::test]
async fn a_reconnect_shows_the_banner_again() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();
    let (id, mut events) = open_terminal(&manager, server.target());
    shown_while_opening(&mut events).await;
    output_ending_with(&mut events, PROMPT).await;
    manager.close(id).unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );

    let mut events = manager.reconnect(id, SIZE).unwrap();
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(text(&shown), text(SHOWN));
    assert!(
        matches!(event, SessionEvent::State(SessionState::Connected)),
        "{event:?}"
    );

    // A duplicate is a connection of its own: it shows it too.
    let (_copy, mut copied) = manager.duplicate(id, SIZE).unwrap();
    let (shown, _) = shown_while_opening(&mut copied).await;
    assert_eq!(text(&shown), text(SHOWN));
}

/// A second channel on the session's connection logs in to nothing: it
/// has no banner of its own.
#[tokio::test]
async fn a_second_channel_shows_no_banner() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();
    let (id, mut events) = open_terminal(&manager, server.target());
    shown_while_opening(&mut events).await;

    let (_channel, mut second) = manager.open_channel(id, SIZE).unwrap();
    let (shown, event) = shown_while_opening(&mut second).await;

    assert_eq!(text(&shown), "");
    assert!(
        matches!(event, SessionEvent::State(SessionState::Connected)),
        "{event:?}"
    );
}

#[tokio::test]
async fn the_banner_is_in_the_session_log() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let log_root = TempDir::new().unwrap();
    let log = LogFolder::new(log_root.path()).quick_log("127.0.0.1");
    let manager = support::manager();
    let (id, mut events) = manager.open_with(
        SessionTarget::Ssh(server.target()),
        SIZE,
        OpenOptions {
            log: Some(log.clone()),
            ..terminal()
        },
    );
    shown_while_opening(&mut events).await;
    output_ending_with(&mut events, PROMPT).await;
    manager.write(id, b"exit\r".to_vec()).unwrap();
    loop {
        if let SessionState::Closed(_) = next_state(&mut events).await {
            break;
        }
    }

    let files: Vec<_> = fs::read_dir(log.folder())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(
        fs::read_to_string(&files[0]).unwrap(),
        "AUTHORIZED ACCESS ONLY\n\nDisconnect now if you are not\nauthorized.\n\
         All access is logged.\ntest-ssh$ exit\n"
    );
}

/// A jump host's banner is the jump host's: named on a dim line, in
/// connect order, and the device's after it says whose it is too.
#[tokio::test]
async fn a_jump_hosts_banner_comes_first_and_is_named() {
    let bastion = TestSshServer::start().await;
    bastion.quirks().auth_banner("Bastion of the site\n");
    let router = TestSshServer::start().await;
    router.quirks().auth_banner("Router r1\n");
    let manager = support::manager();
    let target = SshTarget {
        jump_hosts: vec![jump_host("Site/bastion.yaml", &bastion)],
        ..router.target()
    };

    let (_id, mut events) = open_terminal(&manager, target);
    let (shown, event) = shown_while_opening(&mut events).await;

    assert_eq!(
        text(&shown),
        "\x1b[2m--- login banner of the jump host Site/bastion.yaml ---\x1b[0m\r\n\
         Bastion of the site\r\n\
         \x1b[2m--- login banner of the device ---\x1b[0m\r\n\
         Router r1\r\n"
    );
    assert!(
        matches!(event, SessionEvent::State(SessionState::Connected)),
        "{event:?}"
    );
    assert_eq!(bastion.forwarded(), 1);
}

/// With only the jump host sending one, the device has nothing to name.
#[tokio::test]
async fn a_jump_hosts_banner_alone_is_named() {
    let bastion = TestSshServer::start().await;
    bastion.quirks().auth_banner("Bastion of the site");
    let router = TestSshServer::start().await;
    let manager = support::manager();
    let target = SshTarget {
        jump_hosts: vec![jump_host("bastion", &bastion)],
        ..router.target()
    };

    let (_id, mut events) = open_terminal(&manager, target);
    let (shown, _) = shown_while_opening(&mut events).await;

    assert_eq!(
        text(&shown),
        "\x1b[2m--- login banner of the jump host bastion ---\x1b[0m\r\nBastion of the site\r\n"
    );
}

/// The device's host key is checked after the jump host's login, so the
/// jump host's banner is shown before the device's key question; and the
/// jump host's own password question comes after its banner.
#[tokio::test]
async fn a_jump_hosts_banner_comes_before_its_question_and_the_devices_host_key() {
    let bastion = TestSshServer::start().await;
    bastion.quirks().auth_banner("Bastion of the site\n");
    let router = TestSshServer::start().await;
    let known_hosts = KnownHosts::in_memory();
    known_hosts
        .remember("127.0.0.1", bastion.port(), bastion.host_key())
        .unwrap();
    let manager = SessionManager::new(known_hosts);
    let target = SshTarget {
        jump_hosts: vec![JumpHost {
            auth: SshAuth::Prompt,
            ..jump_host("bastion", &bastion)
        }],
        ..router.target()
    };

    let (id, mut events) = open_terminal(&manager, target);
    let (shown, event) = shown_while_opening(&mut events).await;
    assert_eq!(
        text(&shown),
        "\x1b[2m--- login banner of the jump host bastion ---\x1b[0m\r\nBastion of the site\r\n"
    );
    let asked = prompt(event);
    assert_eq!(asked.name, "Jump host bastion");
    manager.answer(id, vec![PASSWORD.into()]).unwrap();

    match events.next().await {
        Some(SessionEvent::HostKey(question)) => assert_eq!(question.port, router.port()),
        other => panic!("expected the device's host-key question, got {other:?}"),
    }
    manager
        .decide_host_key(id, HostKeyDecision::TrustOnce)
        .unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
}

/// The banner arrives with the username already given: a login asking
/// for both cannot show it before the answers (the device sends it once
/// asked for a login, and the username cannot change after that).
#[tokio::test]
async fn a_login_asking_for_the_username_shows_the_banner_after_the_answers() {
    let server = TestSshServer::start().await;
    server.quirks().auth_banner(BANNER);
    let manager = support::manager();
    let target = SshTarget {
        username: String::new(),
        auth: SshAuth::Prompt,
        ..server.target()
    };

    let (id, mut events) = open_terminal(&manager, target);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let asked = support::next_prompt(&mut events).await;
    assert_eq!(asked.questions.len(), 2, "{asked:?}");
    manager
        .answer(id, vec![USERNAME.into(), PASSWORD.into()])
        .unwrap();
    let shown = output_ending_with(&mut events, SHOWN).await;
    assert_eq!(text(&shown), text(SHOWN));
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
}
