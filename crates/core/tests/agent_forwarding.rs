//! Agent forwarding: a session that forwards the SSH agent lets the device
//! reach it, and one that does not, does not. The agent is the in-process
//! test agent (see `support/agent.rs`); the user's own is never touched.
//! The scripted shell's `agent` command asks the forwarded agent for its
//! keys and prints how many it holds.

mod support;

use std::time::Duration;

use enodix_core::job::{HostState, JobOptions};
use enodix_core::session::{
    JumpHost, SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAgent,
    SshAuth, SshTarget, TerminalSize,
};
use russh::keys::{Algorithm, PrivateKey};
use support::agent::TestAgent;
use support::jobs::{folder, host, profile, run, snippet, Lab};
use support::{next_state, output_ending_with, TestSshServer, AGENT_KEYS, PROMPT, USERNAME};

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

async fn connected_to(manager: &SessionManager, target: SshTarget) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
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
