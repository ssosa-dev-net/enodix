//! Login tests: SSH agent and keyboard-interactive authentication against
//! the in-process test server. The agents are in-process too (see
//! `support/agent.rs`), holding throwaway keys; the user's own agent and
//! keys are never touched.

mod support;

use netterm_core::session::{
    AgentError, CloseReason, LoginPrompt, LoginQuestion, OpenError, SessionEvents, SessionId,
    SessionManager, SessionState, SessionTarget, SshAgent, SshAuth, SshTarget, TerminalSize,
};
use russh::keys::{Algorithm, PrivateKey};
use russh::MethodKind;
use support::agent::TestAgent;
use support::{
    next_prompt, next_state, output_ending_with, TestSshServer, CODE_INSTRUCTIONS, CODE_NAME,
    CODE_QUESTION, ONE_TIME_CODE, OTP_USERNAME, PASSWORD, PASSWORD_QUESTION, PROMPT,
    RSA_CLIENT_KEY, USERNAME,
};

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

fn open(
    manager: &SessionManager,
    server: &TestSshServer,
    username: &str,
    auth: SshAuth,
) -> (SessionId, SessionEvents) {
    let target = SshTarget {
        username: username.into(),
        auth,
        ..server.target()
    };
    manager.open(SessionTarget::Ssh(target), SIZE)
}

/// The state that follows `Connecting` for a login that asks nothing.
async fn state_after_connecting(
    server: &TestSshServer,
    username: &str,
    auth: SshAuth,
) -> SessionState {
    let manager = support::manager();
    let (_id, mut events) = open(&manager, server, username, auth);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let state = next_state(&mut events).await;
    if state == SessionState::Connected {
        output_ending_with(&mut events, PROMPT).await;
    }
    state
}

fn agent_at(agent: &TestAgent) -> SshAuth {
    SshAuth::Agent(SshAgent::At(agent.path().to_path_buf()))
}

fn random_key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

fn agent_failure(error: AgentError) -> SessionState {
    SessionState::Closed(CloseReason::Failed(OpenError::Agent(error)))
}

#[tokio::test]
async fn a_key_held_by_the_agent_logs_in() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;

    let state = state_after_connecting(&server, USERNAME, agent_at(&agent)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn an_rsa_key_held_by_the_agent_logs_in() {
    let server = TestSshServer::start().await;
    let rsa = russh::keys::load_secret_key(RSA_CLIENT_KEY, None).unwrap();
    let agent = TestAgent::start(&[rsa]).await;

    let state = state_after_connecting(&server, USERNAME, agent_at(&agent)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn the_agent_key_the_server_accepts_is_found_among_others() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[random_key(), random_key(), server.client_key().clone()]).await;

    let state = state_after_connecting(&server, USERNAME, agent_at(&agent)).await;

    assert_eq!(state, SessionState::Connected);
}

#[cfg(windows)]
#[tokio::test]
async fn a_key_held_by_pageant_logs_in() {
    let server = TestSshServer::start().await;
    let Some(_pageant) = TestAgent::start_as_pageant(&[server.client_key().clone()]).await else {
        eprintln!("skipped: Pageant is running, and the test does not use the user's agent");
        return;
    };

    let state = state_after_connecting(&server, USERNAME, SshAuth::Agent(SshAgent::Pageant)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn an_agent_that_is_not_running_is_reported_before_connecting() {
    // No server: the agent is asked first, so nothing goes over the network.
    let path = support::agent::unused_path();
    let manager = support::manager();
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port: 9,
        username: USERNAME.into(),
        auth: SshAuth::Agent(SshAgent::At(path.clone())),
        jump_hosts: Vec::new(),
    };

    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);

    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    let state = next_state(&mut events).await;
    let SessionState::Closed(reason) = &state else {
        panic!("expected the session to close, got {state:?}");
    };
    assert!(
        matches!(
            reason,
            CloseReason::Failed(OpenError::Agent(AgentError::NotRunning(_)))
        ),
        "got {reason:?}"
    );
    assert!(reason.offers_password_login());
    assert_eq!(
        reason.to_string(),
        format!(
            "no SSH agent is running (the agent at {}: not running)",
            path.display()
        )
    );
}

#[tokio::test]
async fn an_agent_without_keys_is_reported() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[]).await;

    let state = state_after_connecting(&server, USERNAME, agent_at(&agent)).await;

    assert_eq!(state, agent_failure(AgentError::NoKeys));
    assert_eq!(
        format!("{}", OpenError::Agent(AgentError::NoKeys)),
        "the SSH agent holds no keys"
    );
}

#[tokio::test]
async fn an_agent_whose_keys_the_server_refuses_is_reported() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[random_key()]).await;

    let state = state_after_connecting(&server, USERNAME, agent_at(&agent)).await;

    assert_eq!(state, agent_failure(AgentError::KeysRefused));
    let SessionState::Closed(reason) = state else {
        unreachable!()
    };
    assert!(reason.offers_password_login());
    assert_eq!(
        reason.to_string(),
        "the server accepted none of the SSH agent's keys"
    );
}

#[tokio::test]
async fn a_refused_agent_is_reported_even_where_the_server_could_ask_questions() {
    // The server offers keyboard-interactive to this user, but the agent
    // was what the profile asked for: its failure is the news.
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[random_key()]).await;

    let state = state_after_connecting(&server, OTP_USERNAME, agent_at(&agent)).await;

    assert_eq!(state, agent_failure(AgentError::KeysRefused));
}

#[tokio::test]
async fn a_wrong_password_fails_without_turning_into_a_question() {
    let server = TestSshServer::start().await;

    let state =
        state_after_connecting(&server, OTP_USERNAME, SshAuth::Password("wrong".into())).await;

    assert_eq!(
        state,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

#[tokio::test]
async fn other_failures_do_not_offer_a_password_login() {
    let server = TestSshServer::start().await;

    let state = state_after_connecting(&server, USERNAME, SshAuth::Password("wrong".into())).await;

    let SessionState::Closed(reason) = state else {
        panic!("expected the session to close, got {state:?}");
    };
    assert_eq!(reason, CloseReason::Failed(OpenError::AuthenticationFailed));
    assert!(!reason.offers_password_login());
}

/// The code round's prompt, as the user is shown it.
fn code_prompt() -> LoginPrompt {
    LoginPrompt {
        name: CODE_NAME.into(),
        instructions: CODE_INSTRUCTIONS.into(),
        questions: vec![LoginQuestion {
            text: CODE_QUESTION.into(),
            echo: true,
        }],
    }
}

#[tokio::test]
async fn a_one_time_code_asked_after_the_password_is_shown_and_answered() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(
        &manager,
        &server,
        OTP_USERNAME,
        SshAuth::Password(PASSWORD.into()),
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    assert_eq!(next_prompt(&mut events).await, code_prompt());
    manager.answer(id, vec![ONE_TIME_CODE.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn every_round_of_keyboard_interactive_questions_reaches_the_user() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    // A key the server refuses: the server then offers keyboard-interactive,
    // which asks for the password and then for a code.
    let temp = tempfile::TempDir::new().unwrap();
    let key = temp.path().join("id_ed25519");
    TestSshServer::start().await.write_client_key(&key, None);
    let auth = SshAuth::PrivateKey {
        key_file: key,
        passphrase: None,
    };
    let (id, mut events) = open(&manager, &server, OTP_USERNAME, auth);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    let first = next_prompt(&mut events).await;
    assert_eq!(
        first.questions,
        vec![LoginQuestion {
            text: PASSWORD_QUESTION.into(),
            echo: false,
        }]
    );
    manager.answer(id, vec![PASSWORD.into()]).unwrap();
    assert_eq!(next_prompt(&mut events).await, code_prompt());
    manager.answer(id, vec![ONE_TIME_CODE.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn a_wrong_answer_fails_authentication() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(
        &manager,
        &server,
        OTP_USERNAME,
        SshAuth::Password(PASSWORD.into()),
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    next_prompt(&mut events).await;

    manager.answer(id, vec!["000000".into()]).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

#[tokio::test]
async fn closing_while_a_question_waits_ends_the_session() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let (id, mut events) = open(
        &manager,
        &server,
        OTP_USERNAME,
        SshAuth::Password(PASSWORD.into()),
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    next_prompt(&mut events).await;

    manager.close(id).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
}

#[tokio::test]
async fn a_server_that_only_offers_keyboard_interactive_gets_the_password_without_asking() {
    let server = TestSshServer::start_offering(&[MethodKind::KeyboardInteractive]).await;
    let manager = support::manager();
    let (id, mut events) = open(
        &manager,
        &server,
        OTP_USERNAME,
        SshAuth::Password(PASSWORD.into()),
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    // The password question was answered with the target's password; only
    // the code is left to the user.
    assert_eq!(next_prompt(&mut events).await, code_prompt());
    manager.answer(id, vec![ONE_TIME_CODE.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
}
