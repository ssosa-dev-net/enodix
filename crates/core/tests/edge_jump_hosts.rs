//! Edge testing of jump hosts (#118): a two-hop chain (core bastion, site
//! bastion, router) whose every link runs through a [`LinkProxy`], so each
//! hop can go silent or drop on its own, during the login to the device
//! and during a session; a jump host that refuses to forward; several
//! sessions through one jump host when it goes.

mod support;

use std::time::{Duration, Instant};

use enodix_core::session::{
    CloseReason, ConnectStage, JumpHost, OpenError, SessionEvents, SessionId, SessionManager,
    SessionState, SessionTarget, SshAuth, SshTarget, TerminalSize,
};
use support::proxy::LinkProxy;
use support::{
    next_state, output_ending_with, output_until_closed, Misbehaviour, Stage, TestSshServer,
    PASSWORD, PROMPT, USERNAME, WAIT,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// Every hop's connect timeout: keepalives go out this often.
const SHORT: Duration = Duration::from_secs(1);

/// How long a session may take to notice a hop that went silent: about
/// three timeouts, with room for a loaded machine.
const NOTICED: Duration = Duration::from_secs(12);

const HOPS: [&str; 3] = ["core", "site", "router"];

/// core → site → router, each behind a link of its own.
struct Chain {
    servers: Vec<TestSshServer>,
    links: Vec<LinkProxy>,
}

impl Chain {
    async fn start() -> Self {
        let mut servers = Vec::new();
        let mut links = Vec::new();
        for _ in HOPS {
            let server = TestSshServer::start().await;
            links.push(LinkProxy::to_ssh(&server).await);
            servers.push(server);
        }
        Self { servers, links }
    }

    fn jump(&self, hop: usize) -> JumpHost {
        JumpHost {
            name: HOPS[hop].into(),
            host: "127.0.0.1".into(),
            port: self.links[hop].port(),
            username: USERNAME.into(),
            auth: SshAuth::Password(PASSWORD.into()),
            legacy_crypto: false,
            connect_timeout: SHORT,
        }
    }

    /// The router, through core and site.
    fn target(&self) -> SshTarget {
        SshTarget {
            jump_hosts: vec![self.jump(0), self.jump(1)],
            connect_timeout: SHORT,
            ..self.links[2].target()
        }
    }
}

async fn open(manager: &SessionManager, target: SshTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    (id, events)
}

/// Opens a session through the chain and checks it works.
async fn through(chain: &Chain, manager: &SessionManager) -> (SessionId, SessionEvents) {
    let (id, mut events) = open(manager, chain.target()).await;
    manager.write(id, b"size\r".to_vec()).unwrap();
    assert!(output_ending_with(&mut events, PROMPT)
        .await
        .starts_with(b"size\r\n80x24"));
    (id, events)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hop_of_the_chain_going_silent_during_a_session_ends_it_as_lost_in_bounded_time() {
    each_hop(|hop| async move {
        let chain = Chain::start().await;
        let manager = support::manager();
        let (_id, mut events) = through(&chain, &manager).await;

        chain.links[hop].stall();
        let started = Instant::now();
        let (_, reason) = output_until_closed(&mut events, NOTICED).await;

        assert_eq!(reason, CloseReason::ConnectionLost, "{}", HOPS[hop]);
        eprintln!(
            "{} silent: noticed after {:?}",
            HOPS[hop],
            started.elapsed()
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hop_of_the_chain_dropping_during_a_session_ends_it_as_lost() {
    each_hop(|hop| async move {
        let chain = Chain::start().await;
        let manager = support::manager();
        let (_id, mut events) = through(&chain, &manager).await;

        chain.links[hop].cut();
        let (_, reason) = output_until_closed(&mut events, NOTICED).await;

        assert_eq!(reason, CloseReason::ConnectionLost, "{}", HOPS[hop]);
    })
    .await;
}

/// While the session logs in to the router (which takes its time), a hop
/// goes silent or drops: the session fails within a bound, and says so.
#[tokio::test(flavor = "multi_thread")]
async fn a_hop_going_silent_or_dropping_during_the_login_to_the_device_fails_the_session() {
    for stall in [true, false] {
        each_hop(|hop| async move {
            let chain = Chain::start().await;
            chain.servers[2]
                .quirks()
                .misbehave_at(Stage::Login, Misbehaviour::Pause(Duration::from_secs(30)));
            let manager = support::manager();
            let (_id, mut events) = manager.open(SessionTarget::Ssh(chain.target()), SIZE);
            assert_eq!(next_state(&mut events).await, SessionState::Connecting);
            // The router has been reached and is being logged in to.
            wait_until(
                || chain.servers[2].connections_alive() == 1,
                "the router's login",
            )
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;

            if stall {
                chain.links[hop].stall();
            } else {
                chain.links[hop].cut();
            }
            let (_, reason) = output_until_closed(&mut events, NOTICED).await;

            let CloseReason::Failed(error) = reason else {
                panic!("expected a failure, got {reason:?}");
            };
            let how = if stall { "silent" } else { "dropped" };
            eprintln!("{} {how} during the login: {error}", HOPS[hop]);
            assert_ne!(
                error,
                OpenError::AuthenticationFailed,
                "{} {how}",
                HOPS[hop]
            );
            match (stall, hop) {
                // Silence ends the login's wait first (one timeout, where
                // noticing a silent hop takes three); the jump hosts are
                // then asked, and the first that does not answer is named
                // (#133).
                (true, 0 | 1) => assert!(
                    matches!(&error, OpenError::JumpHost { name, error }
                        if name == HOPS[hop] && error.to_string().contains("stopped answering")),
                    "{error:?}"
                ),
                // Every jump host answers: the device is the one silent.
                (true, _) => assert_eq!(
                    error,
                    OpenError::TimedOut {
                        after: SHORT,
                        stage: ConnectStage::Login
                    }
                ),
                // A jump host that dropped is named.
                (false, 0 | 1) => assert!(
                    matches!(&error, OpenError::JumpHost { name, .. } if name == HOPS[hop]),
                    "{error:?}"
                ),
                (false, _) => assert!(
                    error
                        .to_string()
                        .contains("closed the connection during the login"),
                    "{error}"
                ),
            }
        })
        .await;
    }
}

/// While the session logs in to the second jump host (site), the first
/// (core) goes silent: the failure names core, not site, which only timed
/// out because the link through core did.
#[tokio::test(flavor = "multi_thread")]
async fn a_jump_host_going_silent_while_the_next_one_logs_in_is_named() {
    let chain = Chain::start().await;
    chain.servers[1]
        .quirks()
        .misbehave_at(Stage::Login, Misbehaviour::Pause(Duration::from_secs(30)));
    let manager = support::manager();
    let (_id, mut events) = manager.open(SessionTarget::Ssh(chain.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    wait_until(
        || chain.servers[1].connections_alive() == 1,
        "the site's login",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    chain.links[0].stall();
    let started = Instant::now();
    let (_, reason) = output_until_closed(&mut events, NOTICED).await;

    let CloseReason::Failed(error) = reason else {
        panic!("expected a failure, got {reason:?}");
    };
    eprintln!(
        "core silent during the site's login, after {:?}: {error}",
        started.elapsed()
    );
    assert!(
        matches!(&error, OpenError::JumpHost { name, error }
            if name == "core" && error.to_string().contains("stopped answering")),
        "{error:?}"
    );
}

/// Each jump host of the chain refusing to forward: the failure names the
/// one that refused and says why.
#[tokio::test(flavor = "multi_thread")]
async fn a_jump_host_that_refuses_to_forward_is_named_with_the_reason() {
    for hop in 0..2 {
        let chain = Chain::start().await;
        chain.servers[hop].quirks().refuse_forwarding();
        let manager = support::manager();
        let (_id, mut events) = manager.open(SessionTarget::Ssh(chain.target()), SIZE);
        assert_eq!(next_state(&mut events).await, SessionState::Connecting);

        let (_, reason) = output_until_closed(&mut events, WAIT).await;

        let message = reason.to_string();
        eprintln!("{} refuses: {message}", HOPS[hop]);
        assert!(
            message.contains(&format!("the jump host {} could not reach it", HOPS[hop])),
            "{message}"
        );
        assert!(message.contains("does not allow forwarding"), "{message}");
    }
}

/// Sessions through one jump host each have their own connection to it:
/// one closing leaves the others; the jump host going silent ends them
/// all (a second channel on one of them too), each within the bound.
#[tokio::test(flavor = "multi_thread")]
async fn sessions_through_one_jump_host_all_end_when_it_goes_and_not_before() {
    let chain = Chain::start().await;
    let manager = support::manager();
    let mut sessions = Vec::new();
    for _ in 0..3 {
        sessions.push(through(&chain, &manager).await);
    }
    let (second, mut second_events) = manager.open_channel(sessions[1].0, SIZE).unwrap();
    assert_eq!(
        next_state(&mut second_events).await,
        SessionState::Connecting
    );
    assert_eq!(
        next_state(&mut second_events).await,
        SessionState::Connected
    );
    output_ending_with(&mut second_events, PROMPT).await;

    let (first, mut first_events) = sessions.remove(0);
    manager.close(first).unwrap();
    output_until_closed(&mut first_events, WAIT).await;
    for (id, events) in &mut sessions {
        manager.write(*id, b"size\r".to_vec()).unwrap();
        output_ending_with(events, PROMPT).await;
    }
    manager.write(second, b"size\r".to_vec()).unwrap();
    output_ending_with(&mut second_events, PROMPT).await;

    chain.links[0].stall();
    for (_, events) in &mut sessions {
        let (_, reason) = output_until_closed(events, NOTICED).await;
        assert_eq!(reason, CloseReason::ConnectionLost);
    }
    let (_, reason) = output_until_closed(&mut second_events, NOTICED).await;
    assert_eq!(reason, CloseReason::ConnectionLost);
}

/// Plays `case` for each hop of the chain at once (each on a chain of its
/// own), so the waits for silence overlap.
async fn each_hop<F: std::future::Future<Output = ()>>(case: impl Fn(usize) -> F) {
    futures::future::join_all((0..HOPS.len()).map(case)).await;
}

/// Waits until `done` holds, polling; panics after [`WAIT`].
async fn wait_until(done: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
