//! Edge testing of SSH sessions (#118): floods read at every pace, links
//! that stall or break without a word, devices that close or go silent at
//! every stage, long sessions in compressed time, odd banners and output.
//! Each test asserts what a user would see: the bytes, the states, why the
//! session ended and that it ended within a bound.

mod support;

use std::time::{Duration, Instant};

use enodix_core::logging::LogFolder;
use enodix_core::paste::{plan_paste, PasteSafety};
use enodix_core::session::{
    CloseReason, ConnectStage, OpenError, OutputOptions, SessionEvent, SessionEvents, SessionId,
    SessionManager, SessionState, SessionTarget, SshAuth, SshTarget, TerminalSize,
};
use russh::MethodKind;
use support::proxy::LinkProxy;
use support::raw::RawDevice;
use support::{
    hex, next_prompt, next_state, numbered_lines, output_ending_with, output_of_at_least,
    output_until_closed, unacknowledged_output, HangUpServer, Misbehaviour, Stage, TestSshServer,
    CTRL_C, FLOOD_LINE, INTERRUPTED, PASSWORD, PROMPT, THREE_ANSWERS, THREE_QUESTIONS,
    THREE_QUESTIONS_USERNAME, WAIT,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// A connect timeout short enough for tests about silence: keepalives go
/// out this often, and a link is lost after about three of them.
const SHORT: Duration = Duration::from_secs(1);

/// How long a session may take to notice a link that went silent with
/// [`SHORT`]: three timeouts, and room for a loaded machine.
const NOTICED: Duration = Duration::from_secs(12);

fn short(target: SshTarget) -> SshTarget {
    SshTarget {
        connect_timeout: SHORT,
        ..target
    }
}

async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

async fn open(manager: &SessionManager, target: SshTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    connected(&mut events).await;
    (id, events)
}

/// Why a session that never connects ends, within `bound`.
async fn failure_within(manager: &SessionManager, target: SshTarget, bound: Duration) -> OpenError {
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    let started = Instant::now();
    let (_, reason) = output_until_closed(&mut events, bound).await;
    match reason {
        CloseReason::Failed(error) => {
            assert!(started.elapsed() < bound);
            error
        }
        other => panic!("expected the session to fail, got {other:?}"),
    }
}

/// Reads a flood for `bytes` bytes, sleeping `pause` after each batch, and
/// checks that every byte is the flood's: nothing lost, doubled or out of
/// place. Returns how many bytes arrived.
async fn read_flood(events: &mut SessionEvents, bytes: usize, pause: Duration) -> usize {
    let mut received = 0;
    // Where in a flood line the next byte falls.
    let mut at = 0;
    while received < bytes {
        let event = tokio::time::timeout(WAIT, events.next())
            .await
            .expect("the flood stopped");
        match event {
            Some(SessionEvent::Output(batch)) => {
                for &byte in &batch {
                    assert_eq!(
                        byte, FLOOD_LINE[at],
                        "byte {received} of the flood is not the flood's"
                    );
                    at = (at + 1) % FLOOD_LINE.len();
                    received += 1;
                }
                events.acknowledge(batch.len());
                if !pause.is_zero() {
                    tokio::time::sleep(pause).await;
                }
            }
            other => panic!("expected flood output, got {other:?}"),
        }
    }
    received
}

/// Stops a flood with Ctrl+C and waits for the prompt.
async fn stop_flood(manager: &SessionManager, id: SessionId, events: &mut SessionEvents) {
    manager.write(id, vec![CTRL_C]).unwrap();
    output_ending_with(events, &[INTERRUPTED, PROMPT].concat()).await;
}

// Floods.

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_read_as_fast_as_it_comes_arrives_whole_and_stops_on_ctrl_c() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;

    manager.write(id, b"flood\r".to_vec()).unwrap();
    output_ending_with(&mut events, b"flood\r\n").await;
    read_flood(&mut events, 32 << 20, Duration::ZERO).await;
    stop_flood(&manager, id, &mut events).await;
}

/// Hundreds of megabytes: `cargo test -p enodix-core --test edge_ssh -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: 512 MiB through one session (about a minute)"]
async fn a_flood_of_hundreds_of_megabytes_arrives_whole() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;

    manager.write(id, b"flood\r".to_vec()).unwrap();
    output_ending_with(&mut events, b"flood\r\n").await;
    let started = Instant::now();
    read_flood(&mut events, 512 << 20, Duration::ZERO).await;
    eprintln!("512 MiB of flood in {:?}", started.elapsed());
    stop_flood(&manager, id, &mut events).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_read_slowly_arrives_whole_at_the_readers_pace() {
    let server = TestSshServer::start().await;
    let window = 64 * 1024;
    let manager = support::manager_with_output_options(OutputOptions {
        window,
        ..OutputOptions::default()
    });
    let (id, mut events) = open(&manager, server.target()).await;

    manager.write(id, b"flood\r".to_vec()).unwrap();
    output_ending_with(&mut events, b"flood\r\n").await;
    // A terminal taking 5 ms over each batch.
    read_flood(&mut events, 4 << 20, Duration::from_millis(5)).await;
    stop_flood(&manager, id, &mut events).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_flooding_session_whose_reader_stopped_acknowledging_ends_it_at_once() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    manager.write(id, b"flood\r".to_vec()).unwrap();
    // The reader stops: reading pauses with a window unacknowledged.
    unacknowledged_output(&mut events, Duration::from_millis(500)).await;

    let started = Instant::now();
    manager.close(id).unwrap();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;

    assert_eq!(reason, CloseReason::ClosedLocally);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    wait_until(
        || server.connections_alive() == 0,
        "the device's connection to end",
    )
    .await;
}

/// The reader of a paused flood goes away without closing (a window whose
/// page died): the session must still end, and its connection with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_session_whose_reader_goes_away_ends_and_lets_go_of_the_device() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    manager.write(id, b"flood\r".to_vec()).unwrap();
    unacknowledged_output(&mut events, Duration::from_millis(500)).await;

    drop(events);

    wait_until(
        || server.connections_alive() == 0,
        "the device's connection to end",
    )
    .await;
    assert_eq!(
        manager.write(id, b"x".to_vec()),
        Err(enodix_core::session::SessionError::NotFound)
    );
}

// Links that stall or break.

#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_goes_silent_mid_command_ends_the_session_as_lost_in_bounded_time() {
    let server = TestSshServer::start().await;
    let link = LinkProxy::to_ssh(&server).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, short(link.target())).await;
    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    output_of_at_least(&mut events, 100).await;

    link.stall();
    let started = Instant::now();
    let (_, reason) = output_until_closed(&mut events, NOTICED).await;

    assert_eq!(reason, CloseReason::ConnectionLost);
    eprintln!("a silent link noticed after {:?}", started.elapsed());
}

/// While its reader is paused the session cannot notice a dead link; once
/// reading resumes it does. A known limit, pinned (#133): russh's
/// connection loop hands each data packet to the channel's bounded queue
/// and waits there when it is full, sending no keepalive meanwhile, and it
/// reopens the SSH window as it decodes, not as the session reads, so
/// taking packets to keep it going would hold output without bound.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_goes_silent_while_output_is_paused_is_noticed_once_reading_resumes() {
    let server = TestSshServer::start().await;
    let link = LinkProxy::to_ssh(&server).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, short(link.target())).await;
    manager.write(id, b"flood\r".to_vec()).unwrap();
    let held = unacknowledged_output(&mut events, Duration::from_millis(300)).await;

    link.stall();
    tokio::time::sleep(Duration::from_millis(500)).await;
    events.acknowledge(held.len());
    let (_, reason) = output_until_closed(&mut events, NOTICED).await;

    assert_eq!(reason, CloseReason::ConnectionLost);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_cut_mid_command_ends_the_session_as_lost_after_what_had_arrived() {
    let server = TestSshServer::start().await;
    let link = LinkProxy::to_ssh(&server).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, short(link.target())).await;
    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let start = output_of_at_least(&mut events, 100).await;

    link.cut();
    let (rest, reason) = output_until_closed(&mut events, NOTICED).await;

    assert_eq!(reason, CloseReason::ConnectionLost);
    // What arrived is the start of the listing, in order.
    let listing = [b"lines 200000\r\n".as_slice(), &numbered_lines(200_000)].concat();
    let received = [start, rest].concat();
    assert!(
        listing.starts_with(&received),
        "output out of order or doubled"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_ends_its_shell_with_output_in_flight_delivers_all_of_it_first() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;

    manager.write(id, b"burst 100000\r".to_vec()).unwrap();
    let (output, reason) = output_until_closed(&mut events, WAIT).await;

    assert!(output == [b"burst 100000\r\n".as_slice(), &numbered_lines(100_000)].concat());
    assert_eq!(
        reason,
        CloseReason::RemoteExited {
            exit_status: Some(0)
        }
    );
}

// A device that closes, or goes silent, at every stage.

#[tokio::test]
async fn a_device_that_hangs_up_before_its_banner_says_its_lines_may_be_in_use() {
    let device = HangUpServer::start().await;
    let manager = support::manager();

    let error = failure_within(
        &manager,
        support::target_at("127.0.0.1", device.port()),
        WAIT,
    )
    .await;

    assert_eq!(error, OpenError::ClosedBeforeLogin);
}

/// A device that refuses the password and hangs up at once (its last
/// allowed try, as IOS after its retries): still a refused login, not a
/// connection cut short.
#[tokio::test]
async fn a_device_that_refuses_the_login_and_hangs_up_says_the_login_failed() {
    let server = TestSshServer::start_tuned(|config| config.max_auth_attempts = 2).await;
    let manager = support::manager();
    let target = SshTarget {
        auth: SshAuth::Password("wrong".into()),
        ..server.target()
    };

    let error = failure_within(&manager, target, WAIT).await;

    assert_eq!(error, OpenError::AuthenticationFailed);
}

#[tokio::test]
async fn a_device_that_hangs_up_during_the_key_exchange_says_so() {
    let device =
        RawDevice::sending_then_closing(vec![(Duration::ZERO, b"SSH-2.0-Device\r\n".to_vec())])
            .await;
    let manager = support::manager();

    let error = failure_within(
        &manager,
        support::target_at("127.0.0.1", device.port()),
        WAIT,
    )
    .await;

    let message = error.to_string();
    assert!(message.contains("closed the connection"), "{message}");
}

#[tokio::test]
async fn a_device_that_hangs_up_at_each_stage_of_the_login_ends_the_session_with_a_reason() {
    let manager = support::manager();
    for stage in [Stage::Login, Stage::ChannelOpen, Stage::ShellRequest] {
        let server = TestSshServer::start().await;
        server.quirks().misbehave_at(stage, Misbehaviour::Close);

        let error = failure_within(&manager, server.target(), WAIT).await;

        let message = error.to_string();
        assert_ne!(error, OpenError::AuthenticationFailed, "{stage:?}");
        assert!(
            message.contains("closed the connection"),
            "{stage:?}: {message}"
        );
    }
}

#[tokio::test]
async fn a_device_that_goes_silent_at_each_stage_times_out() {
    let manager = support::manager();
    // At once, so the timeouts overlap.
    let stages = [Stage::Login, Stage::ChannelOpen, Stage::ShellRequest].map(|stage| {
        let manager = &manager;
        async move {
            let server = TestSshServer::start().await;
            server.quirks().misbehave_at(stage, Misbehaviour::Stall);

            let error = failure_within(manager, short(server.target()), WAIT).await;

            let expected = match stage {
                Stage::Login => ConnectStage::Login,
                Stage::ChannelOpen => ConnectStage::Channel,
                _ => ConnectStage::Shell,
            };
            assert_eq!(
                error,
                OpenError::TimedOut {
                    after: SHORT,
                    stage: expected
                },
                "{stage:?}"
            );
        }
    });
    futures::future::join_all(stages).await;
}

#[tokio::test]
async fn a_device_that_goes_silent_during_the_key_exchange_times_out() {
    let device = RawDevice::sending(vec![(Duration::ZERO, b"SSH-2.0-Device\r\n".to_vec())]).await;
    let manager = support::manager();

    let error = failure_within(
        &manager,
        short(support::target_at("127.0.0.1", device.port())),
        WAIT,
    )
    .await;

    assert_eq!(
        error,
        OpenError::TimedOut {
            after: SHORT,
            stage: ConnectStage::KeyExchange
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_drops_the_connection_mid_output_ends_the_session_as_lost() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;

    // The listing is queued, then the connection dropped.
    manager.write(id, b"lines 1000\r".to_vec()).unwrap();
    output_ending_with(&mut events, PROMPT).await;
    manager.write(id, b"drop\r".to_vec()).unwrap();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;

    assert_eq!(reason, CloseReason::ConnectionLost);
}

// Sleep and wake, long sessions.

/// The computer sleeps: nothing crosses for a long pause, during which the
/// device times the session out. On waking the session says the
/// connection was lost, and reconnecting works.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_sleep_in_which_the_device_timed_the_session_out_it_is_lost_and_reconnects() {
    let server = TestSshServer::start_tuned(|config| {
        config.inactivity_timeout = Some(Duration::from_millis(300));
    })
    .await;
    let link = LinkProxy::to_ssh(&server).await;
    let manager = support::manager();
    // Keepalives far apart, as with the default timeout: the sleep is
    // over before the session would notice by itself.
    let (id, mut events) = open(&manager, link.target()).await;

    link.stall();
    wait_until(
        || server.connections_alive() == 0,
        "the device to time the session out",
    )
    .await;
    link.resume();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;

    assert_eq!(reason, CloseReason::ConnectionLost);
    let mut events = manager.reconnect(id, SIZE).unwrap();
    connected(&mut events).await;
}

/// A short sleep the device did not mind: the session carries on.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_short_sleep_the_session_carries_on() {
    let server = TestSshServer::start().await;
    let link = LinkProxy::to_ssh(&server).await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, link.target()).await;

    link.stall();
    manager.write(id, b"size\r".to_vec()).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    link.resume();

    output_ending_with(&mut events, PROMPT).await;
}

/// Hours of idle in compressed time: a keepalive every 400 ms for 5 s
/// (about twelve rounds), with the usual connect timeout so that the login
/// itself has all the time it needs under load (#74). A link in between
/// counts what crosses it: the session must have sent keepalives and the
/// server answered them, not merely stayed quiet. The bounds are far under
/// the twelve expected so that a loaded machine cannot fail them. The
/// session is lost after two unanswered keepalives, that is 1.2 s without a
/// byte from the server, which a slow machine does not reach.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_session_kept_alive_for_many_keepalive_rounds_stays_connected() {
    let server = TestSshServer::start().await;
    let link = LinkProxy::to_ssh(&server).await;
    let manager = support::manager();
    let target = SshTarget {
        keepalive_interval: Some(Duration::from_millis(400)),
        ..link.target()
    };
    let (id, mut events) = open(&manager, target).await;
    let (sent, answered) = (link.writes_to_server(), link.writes_to_client());

    let quiet = tokio::time::timeout(Duration::from_secs(5), events.next()).await;
    assert!(quiet.is_err(), "the idle session reported {quiet:?}");
    assert!(
        link.writes_to_server() >= sent + 5,
        "the session sent too few keepalives"
    );
    assert!(
        link.writes_to_client() >= answered + 5,
        "the server answered too few keepalives"
    );
    manager.write(id, b"size\r".to_vec()).unwrap();
    assert!(output_ending_with(&mut events, PROMPT)
        .await
        .starts_with(b"size\r\n80x24"));
}

/// A device that rekeys every 64 KiB and every 500 ms (as one would after
/// its gigabyte and its hour): output stays whole and in order across
/// dozens of key exchanges, and an idle session survives the timed ones.
#[tokio::test(flavor = "multi_thread")]
async fn output_stays_whole_and_in_order_across_many_rekeys() {
    let server = TestSshServer::start_tuned(|config| {
        config.limits = russh::Limits::new(64 * 1024, 64 * 1024, Duration::from_millis(500));
    })
    .await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;

    manager.write(id, b"lines 200000\r".to_vec()).unwrap();
    let output = output_ending_with(&mut events, PROMPT).await;
    assert!(
        output
            == [
                b"lines 200000\r\n".as_slice(),
                &numbered_lines(200_000),
                PROMPT
            ]
            .concat()
    );

    tokio::time::sleep(Duration::from_millis(1500)).await;
    manager.write(id, b"size\r".to_vec()).unwrap();
    assert!(output_ending_with(&mut events, PROMPT)
        .await
        .starts_with(b"size\r\n80x24"));
}

// Many sessions.

#[tokio::test(flavor = "multi_thread")]
async fn twenty_sessions_with_output_on_all_of_them_each_get_theirs_whole() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let mut sessions = Vec::new();
    for _ in 0..20 {
        sessions.push(open(&manager, server.target()).await);
    }

    let listing = [
        b"lines 20000\r\n".as_slice(),
        &numbered_lines(20_000),
        PROMPT,
    ]
    .concat();
    let mut running = Vec::new();
    for (id, mut events) in sessions {
        manager.write(id, b"lines 20000\r".to_vec()).unwrap();
        running.push(tokio::spawn(async move {
            let output = output_ending_with(&mut events, PROMPT).await;
            (output, events)
        }));
    }
    let mut still_open = Vec::new();
    for session in running {
        let (output, events) = session.await.unwrap();
        assert!(output == listing, "a session's output differs");
        still_open.push(events);
    }
    assert_eq!(server.connections_open(), 20);
}

// Odd banners, prompts and output.

/// The banner a device sends before the login (its legal warning), long
/// and full of control sequences: the login goes on. This session is
/// opened as jobs open theirs, without the banner (a terminal's shows it:
/// `login_banner.rs`).
#[tokio::test]
async fn a_long_login_banner_with_control_sequences_does_not_stop_the_login() {
    let server = TestSshServer::start().await;
    let banner = "\x1b[2J\x1b[1;31mAUTHORIZED ACCESS ONLY\x1b[0m\r\n".repeat(1_500);
    assert!(banner.len() > 50 * 1024);
    server.quirks().auth_banner(&banner);
    let manager = support::manager();

    open(&manager, server.target()).await;
}

/// A login banner larger than an SSH packet may be (256 KiB in russh, as
/// in OpenSSH): the session cannot log in, and must not say the login was
/// refused.
#[tokio::test]
async fn a_login_banner_of_a_megabyte_fails_without_blaming_the_login() {
    let server = TestSshServer::start().await;
    let banner = "\x1b[2J\x1b[1;31mAUTHORIZED ACCESS ONLY\x1b[0m\r\n".repeat(30_000);
    assert!(banner.len() > 1 << 20);
    server.quirks().auth_banner(&banner);
    let manager = support::manager();

    let error = failure_within(&manager, server.target(), WAIT).await;

    eprintln!("a megabyte banner: {error}");
    assert_ne!(error, OpenError::AuthenticationFailed);
}

#[tokio::test]
async fn a_megabyte_printed_before_the_shell_is_confirmed_arrives_whole_before_the_prompt() {
    let server = TestSshServer::start().await;
    let motd = b"\x1b[1mWelcome\x1b[0m to the device\r\n".repeat(40_000);
    server.quirks().output_before_shell(&motd);
    let manager = support::manager();
    let (_id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);

    let output = output_ending_with(&mut events, PROMPT).await;

    assert!(output == [motd.as_slice(), PROMPT].concat());
}

#[tokio::test]
async fn output_that_is_not_utf8_and_characters_split_across_reads_arrive_byte_for_byte() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    // Latin-1, a lone continuation byte, an é split in two, a wide CJK
    // character split in three, a combining accent apart from its letter.
    let pieces: [&[u8]; 9] = [
        b"caf\xe9 \x80 ",
        b"\xc3",
        b"\xa9 ",
        b"\xe8",
        b"\xa1",
        b"\xa8 ",
        b"e",
        "\u{301}".as_bytes(),
        b"\r\n",
    ];
    let line = format!(
        "raw {}\r",
        pieces.iter().map(|p| hex(p)).collect::<Vec<_>>().join(" ")
    );

    manager.write(id, line.clone().into_bytes()).unwrap();
    let output = output_ending_with(&mut events, PROMPT).await;

    let echo = [line.as_bytes(), b"\n"].concat();
    assert_eq!(output, [echo.as_slice(), &pieces.concat(), PROMPT].concat());
}

#[tokio::test]
async fn a_console_that_answers_a_byte_at_a_time_shows_each_byte_as_it_comes() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    manager.write(id, b"slowly 100 abcdef\r".to_vec()).unwrap();
    output_ending_with(&mut events, b"\r\n").await;

    // Delivered as it comes, not held for a batch: the first bytes arrive
    // while the rest are still on their way.
    let first = output_of_at_least(&mut events, 1).await;
    assert!(first.len() < 6, "{first:?} came in one piece");
    let rest = output_ending_with(&mut events, PROMPT).await;
    assert_eq!(
        [first, rest].concat(),
        [b"abcdef\r\n".as_slice(), PROMPT].concat()
    );
}

#[tokio::test]
async fn a_device_that_answers_the_login_after_seconds_is_waited_for() {
    let server = TestSshServer::start().await;
    server
        .quirks()
        .pause_at(Stage::Login, Duration::from_secs(2));
    server
        .quirks()
        .pause_at(Stage::ShellRequest, Duration::from_secs(1));
    let manager = support::manager();

    open(&manager, server.target()).await;
}

/// A login of three questions in a row, one round each; the lone password
/// question is answered with the stored password, the others reach the
/// user in order.
#[tokio::test]
async fn a_login_asking_three_questions_in_a_row_asks_the_user_each_in_turn() {
    let server = TestSshServer::start_offering(&[MethodKind::KeyboardInteractive]).await;
    let manager = support::manager();
    let target = SshTarget {
        username: THREE_QUESTIONS_USERNAME.into(),
        auth: SshAuth::Password(PASSWORD.into()),
        ..server.target()
    };
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    let first = next_prompt(&mut events).await;
    assert_eq!(first.questions[0].text, THREE_QUESTIONS[0]);
    assert!(first.questions[0].echo);
    manager.answer(id, vec![THREE_ANSWERS[0].into()]).unwrap();
    let third = next_prompt(&mut events).await;
    assert_eq!(third.questions[0].text, THREE_QUESTIONS[2]);
    assert!(!third.questions[0].echo);
    manager.answer(id, vec![THREE_ANSWERS[2].into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
}

// The log of a long session, and of odd output.

/// The log's one file of a quick connection called `name` in `logs`.
fn log_text(logs: &LogFolder, name: &str) -> String {
    let folder = logs.quick_log(name).folder();
    let files: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    std::fs::read_to_string(&files[0]).unwrap()
}

/// Characters split across reads, wide and combining ones, and bytes that
/// are not UTF-8, as the log keeps them.
#[tokio::test]
async fn the_log_keeps_characters_split_across_reads_whole() {
    let server = TestSshServer::start().await;
    let temp = TempDir::new().unwrap();
    let logs = LogFolder::new(temp.path());
    let manager = support::manager();
    let (id, mut events) = manager.open_logged(
        SessionTarget::Ssh(server.target()),
        SIZE,
        Some(logs.quick_log("odd")),
    );
    connected(&mut events).await;
    let pieces: [&[u8]; 8] = [
        b"\xc3",
        b"\xa9t\xc3",
        b"\xa9 \xe8",
        b"\xa1",
        b"\xa8 e",
        "\u{301} ".as_bytes(),
        b"caf\xe9",
        b"\r\n",
    ];
    let line = format!(
        "raw {}\r",
        pieces.iter().map(|p| hex(p)).collect::<Vec<_>>().join(" ")
    );
    manager.write(id, line.into_bytes()).unwrap();
    output_ending_with(&mut events, PROMPT).await;
    manager.close(id).unwrap();
    output_until_closed(&mut events, WAIT).await;

    let log = log_text(&logs, "odd");
    assert!(
        log.contains("été \u{8868} e\u{301} caf\u{fffd}\n"),
        "{log:?}"
    );
}

/// A long session's log: every line of a large listing, in order, once.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_sessions_log_has_every_line_in_order() {
    let server = TestSshServer::start().await;
    let temp = TempDir::new().unwrap();
    let logs = LogFolder::new(temp.path());
    let manager = support::manager();
    let (id, mut events) = manager.open_logged(
        SessionTarget::Ssh(server.target()),
        SIZE,
        Some(logs.quick_log("long")),
    );
    connected(&mut events).await;
    for _ in 0..4 {
        manager.write(id, b"lines 100000\r".to_vec()).unwrap();
        output_ending_with(&mut events, PROMPT).await;
    }
    manager.close(id).unwrap();
    output_until_closed(&mut events, WAIT).await;

    let log = log_text(&logs, "long");
    let listing: String = (1..=100_000).map(|n| format!("line {n}\n")).collect();
    let prompt = std::str::from_utf8(PROMPT).unwrap().trim_end();
    let one = format!("{prompt} lines 100000\n{listing}");
    assert_eq!(log, format!("{}{prompt}\n", one.repeat(4)));
}

// Pasting.

/// A hundred kilobytes pasted, line by line as the interface sends
/// it, into a device that takes input in its small window (16 KiB) and
/// echoes every byte as a message of its own: the echo comes back whole
/// and in order.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_paste_into_a_slow_device_comes_back_whole_and_in_order() {
    paste_into_a_slow_device(2_000, None).await;
}

/// A megabyte, the device also taking 1 ms (on Windows, a timer tick)
/// over each piece of input: `cargo test -p enodix-core --test edge_ssh
/// -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: a megabyte echoed a byte at a time by a slow device (about five minutes)"]
async fn a_megabyte_pasted_into_a_very_slow_device_comes_back_whole_and_in_order() {
    paste_into_a_slow_device(20_000, Some(Duration::from_millis(1))).await;
}

async fn paste_into_a_slow_device(lines: u32, delay: Option<Duration>) {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    // Plain characters (no Enter: the shell would run each line), with
    // the line breaks the paste path splits on taken as blanks.
    let text: String = (1..=lines)
        .map(|n| format!("interface Ethernet{n} description pasted line {n}\n"))
        .collect();
    let plan = plan_paste(&text, &PasteSafety::default(), false);
    if let Some(delay) = delay {
        server.quirks().read_input_slowly(delay);
    }

    for line in &plan.lines {
        let line = line.replace('\n', " ");
        manager.write(id, line.into_bytes()).unwrap();
    }
    // The echo keeps coming as long as it makes progress.
    let mut echo = Vec::new();
    while echo.len() < text.len() {
        echo.extend(output_of_at_least(&mut events, 1).await);
    }

    assert!(
        echo == text.replace('\n', " ").into_bytes(),
        "the echo differs"
    );
}

/// Closing while a paste waits for a device that has stopped reading
/// ends the session at once.
#[tokio::test(flavor = "multi_thread")]
async fn closing_while_a_paste_waits_for_a_device_that_stopped_reading_ends_at_once() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(&manager, server.target()).await;
    server.quirks().read_input_slowly(Duration::from_secs(3600));
    manager.write(id, b"x".to_vec()).unwrap();
    for _ in 0..1000 {
        manager.write(id, vec![b'y'; 1024]).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    manager.close(id).unwrap();
    let (_, reason) = output_until_closed(&mut events, WAIT).await;

    assert_eq!(reason, CloseReason::ClosedLocally);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

/// Waits until `done` holds, polling; panics after [`WAIT`].
async fn wait_until(done: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
