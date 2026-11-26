//! Agent forwarding: a session that forwards the SSH agent lets the device
//! reach it, and one that does not, does not. The agent is the in-process
//! test agent (see `support/agent.rs`); the user's own is never touched.
//! The scripted shell's `agent` command asks the forwarded agent for its
//! keys and prints how many it holds.

mod support;

use enodix_core::session::{
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshAgent, SshAuth,
    SshTarget, TerminalSize,
};
use russh::keys::{Algorithm, PrivateKey};
use support::agent::TestAgent;
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
