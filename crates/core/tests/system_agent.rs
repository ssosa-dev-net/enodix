//! The system's SSH agent (`SshAgent::System`, what agent profiles use) is
//! the one `SSH_AUTH_SOCK` names, on Linux as on Windows (where only a
//! named pipe counts). The variable is set, for this process only, to an
//! in-process test agent, so the user's own agent is never reached. It is
//! the one test of this binary: changing the environment while other
//! tests run would be a race.

mod support;

use enodix_core::session::{
    SessionState, SessionTarget, SshAgent, SshAuth, SshTarget, TerminalSize,
};
use support::agent::TestAgent;
use support::{next_state, output_ending_with, TestSshServer, PROMPT};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

#[tokio::test]
async fn the_system_agent_is_the_one_ssh_auth_sock_names() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    std::env::set_var("SSH_AUTH_SOCK", agent.path());

    let manager = support::manager();
    let target = SshTarget {
        auth: SshAuth::Agent(SshAgent::System),
        ..server.target()
    };
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}
