//! Agent forwarding: a session that forwards the SSH agent lets the device
//! reach it, and one that does not, does not. The agent is the in-process
//! test agent (see `support/agent.rs`); the user's own is never touched.
//! The scripted shell's `agent` command asks the forwarded agent for its
//! keys and prints how many it holds.
//!
//! A workspace's `agent_forwarding` setting only asks: a terminal session
//! to such a host forwards the agent once the user approves it on this
//! computer, for the tab or always for that host (kept in the data folder,
//! keyed by the workspace, the host and its address and port).

mod support;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use enodix_core::configs::ConfigStore;
use enodix_core::credentials::Profiles;
use enodix_core::job::{HostState, JobOptions, LoginBackup, LoginBackupOutcome, SessionLogin};
use enodix_core::session::{
    AgentApprovals, AgentForwardingDecision, AgentForwardingQuestion, HostSession, HostWay,
    JumpHost, OpenOptions, SessionEvent, SessionEvents, SessionId, SessionManager, SessionState,
    SessionTarget, SshAgent, SshAuth, SshTarget, TerminalSize,
};
use enodix_core::workspace::HostId;
use russh::keys::{Algorithm, PrivateKey};
use support::agent::TestAgent;
use support::jobs::{folder, host, profile, run, snippet, Lab};
use support::{next_state, output_ending_with, TestSshServer, AGENT_KEYS, PROMPT, USERNAME, WAIT};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

async fn connected(
    manager: &SessionManager,
    server: &TestSshServer,
    agent: &TestAgent,
    agent_forwarding: bool,
) -> (SessionId, SessionEvents) {
    let target = SshTarget {
        username: USERNAME.into(),
        auth: SshAuth::Agent(SshAgent::At(agent.path().to_path_buf())),
        agent_forwarding,
        ..server.target()
    };
    connected_to(manager, target).await
}

/// Opens by the target alone, saying in so many words that the session may
/// forward the agent without asking: these tests exercise the mechanics.
fn unasked() -> OpenOptions {
    OpenOptions {
        forward_agent_without_asking: true,
        ..OpenOptions::default()
    }
}

async fn connected_to(manager: &SessionManager, target: SshTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open_with(SessionTarget::Ssh(target), SIZE, unasked());
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    (id, events)
}

fn random_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

#[tokio::test]
async fn a_forwarded_agent_answers_the_device_with_its_keys() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone(), random_key()]).await;
    let manager = support::manager();
    let (id, mut events) = connected(&manager, &server, &agent, true).await;
    assert!(manager.agent_forwarded(id).unwrap());

    manager.write(id, b"agent\r".to_vec()).unwrap();

    output_ending_with(&mut events, format!("{AGENT_KEYS}2\r\n").as_bytes()).await;
}

#[tokio::test]
async fn the_agent_is_not_forwarded_unless_asked() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let manager = support::manager();
    let (id, mut events) = connected(&manager, &server, &agent, false).await;
    assert!(!manager.agent_forwarded(id).unwrap());

    manager.write(id, b"agent\r".to_vec()).unwrap();

    let output = output_ending_with(&mut events, PROMPT).await;
    assert!(String::from_utf8_lossy(&output).contains("agent: not forwarded\r\n"));
}

#[tokio::test]
async fn a_second_ask_for_the_agent_is_answered_too() {
    // Each time the device reaches the agent it opens a channel of its own.
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let manager = support::manager();
    let (id, mut events) = connected(&manager, &server, &agent, true).await;

    for _ in 0..2 {
        manager.write(id, b"agent\r".to_vec()).unwrap();
        output_ending_with(&mut events, format!("{AGENT_KEYS}1\r\n").as_bytes()).await;
    }
}

#[tokio::test]
async fn a_server_reaching_for_an_agent_it_was_not_given_is_refused() {
    // `agent unasked` opens an agent channel although the client asked for
    // no forward, as a hostile server would.
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let manager = support::manager();
    let (id, mut events) = connected(&manager, &server, &agent, false).await;

    manager.write(id, b"agent unasked\r".to_vec()).unwrap();

    output_ending_with(&mut events, b"agent: refused\r\n").await;
    assert_eq!(server.agent_requests(), 0);
}

#[tokio::test]
async fn the_agent_is_forwarded_to_the_device_and_never_to_its_jump_host() {
    let bastion = TestSshServer::start().await;
    let device = TestSshServer::start().await;
    let agent = TestAgent::start(&[device.client_key().clone()]).await;
    let bastion_target = bastion.target();
    let target = SshTarget {
        username: USERNAME.into(),
        auth: SshAuth::Agent(SshAgent::At(agent.path().to_path_buf())),
        agent_forwarding: true,
        jump_hosts: vec![JumpHost {
            name: "bastion".into(),
            host: bastion_target.host,
            port: bastion_target.port,
            username: bastion_target.username,
            auth: bastion_target.auth,
            legacy_crypto: false,
            connect_timeout: bastion_target.connect_timeout,
        }],
        ..device.target()
    };
    let manager = support::manager();
    let (id, mut events) = connected_to(&manager, target).await;

    manager.write(id, b"agent\r".to_vec()).unwrap();

    output_ending_with(&mut events, format!("{AGENT_KEYS}1\r\n").as_bytes()).await;
    assert_eq!(device.agent_requests(), 1);
    assert_eq!(bastion.agent_requests(), 0);
}

#[tokio::test]
async fn a_job_on_a_host_that_forwards_the_agent_does_not_forward_it() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        ("r1.yaml", host(&device, "frr", "agent_forwarding: true\n")),
    ]);
    lab.put(profile("lab", USERNAME));
    let plan = lab.plan(&folder(""), &snippet("Show"));

    let ran = run(
        &support::manager(),
        plan,
        JobOptions::default(),
        Duration::from_secs(30),
    )
    .await;

    let r1 = ran.result("r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert_eq!(device.agent_requests(), 0);
}

// --- A target alone never forwards unless the caller says so ---

fn asking_target(server: &TestSshServer, agent: &TestAgent) -> SshTarget {
    SshTarget {
        username: USERNAME.into(),
        auth: SshAuth::Agent(SshAgent::At(agent.path().to_path_buf())),
        agent_forwarding: true,
        ..server.target()
    }
}

#[tokio::test]
async fn a_target_asking_for_the_agent_does_not_forward_it_by_default() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let manager = support::manager();
    let (id, mut events) = manager.open_with(
        SessionTarget::Ssh(asking_target(&server, &agent)),
        SIZE,
        OpenOptions::default(),
    );

    logged_in_after_connecting(&mut events).await;

    assert!(!manager.agent_forwarded(id).unwrap());
    device_reaches(&manager, id, &mut events, None).await;
    assert_eq!(server.agent_requests(), 0);

    // Neither a reconnect nor a duplicate forwards it.
    closed(&manager, id, &mut events).await;
    let mut events = manager.reconnect(id, SIZE).unwrap();
    logged_in_after_connecting(&mut events).await;
    assert!(!manager.agent_forwarded(id).unwrap());
    let (copy, mut copied) = manager.duplicate(id, SIZE).unwrap();
    logged_in_after_connecting(&mut copied).await;
    assert!(!manager.agent_forwarded(copy).unwrap());
    assert_eq!(server.agent_requests(), 0);
}

#[tokio::test]
async fn a_target_asking_for_the_agent_forwards_it_when_the_options_allow_it() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let manager = support::manager();
    let (id, mut events) = manager.open_with(
        SessionTarget::Ssh(asking_target(&server, &agent)),
        SIZE,
        unasked(),
    );

    logged_in_after_connecting(&mut events).await;

    assert!(manager.agent_forwarded(id).unwrap());
    device_reaches(&manager, id, &mut events, Some(1)).await;
    assert_eq!(server.agent_requests(), 1);

    // A reconnect and a duplicate keep it; a second channel follows.
    closed(&manager, id, &mut events).await;
    let mut events = manager.reconnect(id, SIZE).unwrap();
    logged_in_after_connecting(&mut events).await;
    assert_eq!(server.agent_requests(), 2);
    let (copy, mut copied) = manager.duplicate(id, SIZE).unwrap();
    logged_in_after_connecting(&mut copied).await;
    assert!(manager.agent_forwarded(copy).unwrap());
    assert_eq!(server.agent_requests(), 3);
}

#[tokio::test]
async fn a_workspace_host_asks_even_when_the_options_allow_forwarding() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let lab = asking_lab(&server);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());

    let (_id, mut events) = manager.open_host(session_to(&lab, "r1.yaml", &agent), SIZE, unasked());

    agent_question(&mut events).await;
    assert_eq!(server.connections(), 0);
}

async fn logged_in_after_connecting(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

// --- The user approves forwarding to a workspace host ---

/// A workspace whose host `r1.yaml` (on `server`) asks to forward the agent.
fn asking_lab(server: &TestSshServer) -> Lab {
    Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        ("r1.yaml", host(server, "frr", "agent_forwarding: true\n")),
    ])
}

/// A manager keeping its approvals in the data folder `data`, as the app's.
fn manager_in(data: &Path) -> SessionManager {
    support::manager().with_agent_approvals(AgentApprovals::in_data_folder(data))
}

/// The terminal session to the host `id` as the app works it out, logging
/// in with the test agent (so that it is the agent forwarded, never the
/// user's).
fn session_to(lab: &Lab, id: &str, agent: &TestAgent) -> HostSession {
    let mut session = Profiles::new(&lab.store, &lab.workspace)
        .host_session(&HostId::new(id), HostWay::Ssh { typed: None })
        .unwrap();
    let SessionTarget::Ssh(target) = &mut session.target else {
        panic!("an SSH session");
    };
    target.auth = SshAuth::Agent(SshAgent::At(agent.path().to_path_buf()));
    session
}

fn open_host(
    manager: &SessionManager,
    lab: &Lab,
    id: &str,
    agent: &TestAgent,
) -> (SessionId, SessionEvents) {
    manager.open_host(session_to(lab, id, agent), SIZE, OpenOptions::default())
}

/// Waits for the question whether to forward the agent, asked once the
/// session starts connecting.
async fn agent_question(events: &mut SessionEvents) -> AgentForwardingQuestion {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    match tokio::time::timeout(WAIT, events.next()).await {
        Ok(Some(SessionEvent::AgentForwarding(question))) => question,
        other => panic!("expected the agent forwarding question, got {other:?}"),
    }
}

/// Waits until the session (answered, or asking nothing) shows its prompt.
async fn logged_in(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

/// Checks what the device reaches through the forwarded agent, as the
/// scripted shell's `agent` command prints it: `keys` keys, or (`None`)
/// no agent at all.
async fn device_reaches(
    manager: &SessionManager,
    id: SessionId,
    events: &mut SessionEvents,
    keys: Option<usize>,
) {
    manager.write(id, b"agent\r".to_vec()).unwrap();
    match keys {
        Some(keys) => {
            output_ending_with(events, format!("{AGENT_KEYS}{keys}\r\n").as_bytes()).await;
        }
        None => {
            let output = output_ending_with(events, PROMPT).await;
            let output = String::from_utf8_lossy(&output);
            assert!(output.contains("agent: not forwarded\r\n"), "{output:?}");
        }
    }
}

async fn closed(manager: &SessionManager, id: SessionId, events: &mut SessionEvents) {
    manager.close(id).unwrap();
    loop {
        if let SessionState::Closed(_) = next_state(events).await {
            return;
        }
    }
}

#[tokio::test]
async fn a_host_asking_for_forwarding_asks_before_anything_reaches_the_device() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let lab = asking_lab(&server);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());

    let (id, mut events) = open_host(&manager, &lab, "r1.yaml", &agent);

    let question = agent_question(&mut events).await;
    assert_eq!(
        question,
        AgentForwardingQuestion {
            name: "r1".into(),
            host: "127.0.0.1".into(),
            port: server.port(),
        }
    );
    // Unanswered, nothing goes on: no connection, no agent request.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.connections(), 0);
    assert_eq!(server.agent_requests(), 0);
    assert!(!manager.agent_forwarded(id).unwrap());
}

#[tokio::test]
async fn declining_connects_without_forwarding_the_agent() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let lab = asking_lab(&server);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());
    let (id, mut events) = open_host(&manager, &lab, "r1.yaml", &agent);
    agent_question(&mut events).await;

    manager
        .decide_agent_forwarding(id, AgentForwardingDecision::WithoutForwarding)
        .unwrap();

    logged_in(&mut events).await;
    device_reaches(&manager, id, &mut events, None).await;
    assert_eq!(server.agent_requests(), 0);
    assert!(!manager.agent_forwarded(id).unwrap());
    // Nothing was remembered in the data folder.
    assert!(!data.path().join("agent_forwarding.json").exists());
}

#[tokio::test]
async fn closing_the_question_closes_the_session_without_forwarding() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let lab = asking_lab(&server);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());
    let (id, mut events) = open_host(&manager, &lab, "r1.yaml", &agent);
    agent_question(&mut events).await;

    manager.close(id).unwrap();

    assert!(matches!(
        next_state(&mut events).await,
        SessionState::Closed(_)
    ));
    assert_eq!(server.connections(), 0);
    assert_eq!(server.agent_requests(), 0);
}

#[tokio::test]
async fn forwarding_for_this_session_lasts_for_the_tab_and_is_not_remembered() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let lab = asking_lab(&server);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());
    let (id, mut events) = open_host(&manager, &lab, "r1.yaml", &agent);
    agent_question(&mut events).await;

    manager
        .decide_agent_forwarding(id, AgentForwardingDecision::ForwardThisSession)
        .unwrap();

    logged_in(&mut events).await;
    assert!(manager.agent_forwarded(id).unwrap());
    device_reaches(&manager, id, &mut events, Some(1)).await;
    assert_eq!(server.agent_requests(), 1);

    // A reconnect of the same tab keeps the answer: no question.
    closed(&manager, id, &mut events).await;
    let mut events = manager.reconnect(id, SIZE).unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    logged_in(&mut events).await;
    assert_eq!(server.agent_requests(), 2);

    // A duplicated tab follows the tab it copies.
    let (copy, mut copied) = manager.duplicate(id, SIZE).unwrap();
    assert_eq!(next_state(&mut copied).await, SessionState::Connecting);
    logged_in(&mut copied).await;
    assert!(manager.agent_forwarded(copy).unwrap());
    assert_eq!(server.agent_requests(), 3);

    // Another tab, and the app started again, ask again.
    let (_other, mut other) = open_host(&manager, &lab, "r1.yaml", &agent);
    agent_question(&mut other).await;
    let again = manager_in(data.path());
    let (_id, mut events) = open_host(&again, &lab, "r1.yaml", &agent);
    agent_question(&mut events).await;
}

#[tokio::test]
async fn always_forwarding_is_remembered_for_that_host_at_that_address() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let mut lab = Lab::new(&[
        ("r1.yaml", host(&server, "frr", "agent_forwarding: true\n")),
        ("r2.yaml", host(&server, "frr", "agent_forwarding: true\n")),
    ]);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());
    let (id, mut events) = open_host(&manager, &lab, "r1.yaml", &agent);
    agent_question(&mut events).await;

    manager
        .decide_agent_forwarding(id, AgentForwardingDecision::AlwaysForward)
        .unwrap();

    logged_in(&mut events).await;
    device_reaches(&manager, id, &mut events, Some(1)).await;

    // The app started again: no question, the agent is forwarded.
    let again = manager_in(data.path());
    let (id, mut events) = open_host(&again, &lab, "r1.yaml", &agent);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    logged_in(&mut events).await;
    assert!(again.agent_forwarded(id).unwrap());
    device_reaches(&again, id, &mut events, Some(1)).await;

    // Another host at the same address asks.
    let (_r2, mut r2) = open_host(&again, &lab, "r2.yaml", &agent);
    agent_question(&mut r2).await;

    // So does r1 once its address (here its port) changes.
    let moved = TestSshServer::start().await;
    fs::write(
        lab.root().join("r1.yaml"),
        host(&moved, "frr", "agent_forwarding: true\n"),
    )
    .unwrap();
    assert!(lab.workspace.refresh().unwrap());
    let (_r1, mut r1) = open_host(&again, &lab, "r1.yaml", &agent);
    let question = agent_question(&mut r1).await;
    assert_eq!(question.port, moved.port());
}

#[tokio::test]
async fn a_job_on_a_host_always_forwarded_to_does_not_forward_the_agent() {
    let device = TestSshServer::start_device("r1").await;
    let agent = TestAgent::start(&[device.client_key().clone()]).await;
    let lab = asking_lab(&device);
    let data = TempDir::new().unwrap();
    let manager = manager_in(data.path());
    let (id, mut events) = open_host(&manager, &lab, "r1.yaml", &agent);
    agent_question(&mut events).await;
    manager
        .decide_agent_forwarding(id, AgentForwardingDecision::AlwaysForward)
        .unwrap();
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, &device.prompt()).await;
    assert_eq!(device.agent_requests(), 1);

    let plan = lab.plan(&folder(""), &snippet("Show"));
    let ran = run(
        &manager,
        plan,
        JobOptions::default(),
        Duration::from_secs(30),
    )
    .await;

    let r1 = ran.result("r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert_eq!(device.agent_requests(), 1);
}

#[tokio::test]
async fn the_backup_on_login_channel_does_not_ask_for_the_agent() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[(
        "r1.yaml",
        host(
            &device,
            "eos",
            "agent_forwarding: true\nbackup_on_login: true\n",
        ),
    )]);
    let profiles = Profiles::new(&lab.store, &lab.workspace);
    let r1 = HostId::new("r1.yaml");
    let configs = ConfigStore::new(lab.temp.path().join("data").join("configs"));
    let backup = LoginBackup::plan(
        &lab.workspace,
        &profiles,
        &r1,
        SessionLogin::Profile,
        configs,
    )
    .expect("the backup applies");
    // Opened by its target, which forwards the agent (allowed explicitly).
    let target = profiles.profile_target(&r1).unwrap();
    assert!(target.agent_forwarding);
    let manager = support::manager();
    let (_id, mut events) = manager.open_with(
        SessionTarget::Ssh(target),
        SIZE,
        OpenOptions {
            when_connected: Some(Arc::new(backup)),
            ..unasked()
        },
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);

    let outcome = tokio::time::timeout(WAIT * 3, async {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => events.acknowledge(bytes.len()),
                Some(SessionEvent::Backup(outcome)) => return outcome,
                other => panic!("expected the backup outcome, got {other:?}"),
            }
        }
    })
    .await
    .expect("no backup outcome arrived");

    assert!(
        matches!(outcome, LoginBackupOutcome::Saved { .. }),
        "{outcome:?}"
    );
    // One login, two channels: only the terminal's shell asked.
    assert_eq!(device.logins(), 1);
    assert_eq!(device.agent_requests(), 1);
}
