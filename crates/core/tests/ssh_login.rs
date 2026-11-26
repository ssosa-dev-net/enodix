//! Login tests: SSH agent and keyboard-interactive authentication against
//! the in-process test server. The agents are in-process too (see
//! `support/agent.rs`), holding throwaway keys; the user's own agent and
//! keys are never touched.

mod support;

use enodix_core::session::{
    AgentError, CloseReason, LoginPrompt, LoginQuestion, OpenError, SessionEvents, SessionId,
    SessionManager, SessionState, SessionTarget, SshAgent, SshAuth, SshTarget, TerminalSize,
};
use russh::keys::{Algorithm, PrivateKey};
use russh::MethodKind;
use support::agent::TestAgent;
use support::{
    next_prompt, next_state, output_ending_with, TestSshServer, CERT_USERNAME, CODE_INSTRUCTIONS,
    CODE_NAME, CODE_QUESTION, KEY_THEN_PASSWORD_USERNAME, ONE_TIME_CODE, OTP_USERNAME, PASSWORD,
    PASSWORD_QUESTION, PROMPT, RSA_CLIENT_KEY, USERNAME,
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
        auth: SshAuth::Agent(SshAgent::At(path.clone())),
        ..support::target_at("127.0.0.1", 9)
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

/// The password question asked after a key the server accepted as a first
/// step.
fn password_prompt() -> LoginPrompt {
    LoginPrompt {
        name: String::new(),
        instructions: String::new(),
        questions: vec![LoginQuestion {
            text: PASSWORD_QUESTION.into(),
            echo: false,
        }],
    }
}

/// A key profile for the server's client key, in `temp`.
fn key_file_auth(server: &TestSshServer, temp: &tempfile::TempDir) -> SshAuth {
    let key = temp.path().join("id_ed25519");
    server.write_client_key(&key, None);
    SshAuth::PrivateKey {
        key_file: key,
        passphrase: None,
    }
}

#[tokio::test]
async fn a_server_that_wants_a_password_after_the_key_asks_for_it() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let temp = tempfile::TempDir::new().unwrap();
    let auth = key_file_auth(&server, &temp);
    let (id, mut events) = open(&manager, &server, KEY_THEN_PASSWORD_USERNAME, auth);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    assert_eq!(next_prompt(&mut events).await, password_prompt());
    manager.answer(id, vec![PASSWORD.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn a_password_after_an_agent_key_is_asked_for_too() {
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    let manager = support::manager();
    let (id, mut events) = open(
        &manager,
        &server,
        KEY_THEN_PASSWORD_USERNAME,
        agent_at(&agent),
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    assert_eq!(next_prompt(&mut events).await, password_prompt());
    manager.answer(id, vec![PASSWORD.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
}

#[tokio::test]
async fn a_wrong_password_after_the_key_fails_authentication() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let temp = tempfile::TempDir::new().unwrap();
    let auth = key_file_auth(&server, &temp);
    let (id, mut events) = open(&manager, &server, KEY_THEN_PASSWORD_USERNAME, auth);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    next_prompt(&mut events).await;

    manager.answer(id, vec!["wrong".into()]).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

#[tokio::test]
async fn a_refused_key_is_not_followed_by_a_password_question() {
    // The key is not the one the server knows: no first step, so the
    // password method offered is not a second one, and nothing is asked.
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let key = temp.path().join("id_ed25519");
    TestSshServer::start().await.write_client_key(&key, None);
    let auth = SshAuth::PrivateKey {
        key_file: key,
        passphrase: None,
    };

    let state = state_after_connecting(&server, KEY_THEN_PASSWORD_USERNAME, auth).await;

    assert_eq!(
        state,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

/// A key file of the server's client key in `temp`, with `certificate`
/// beside it if given.
fn key_with_certificate(
    server: &TestSshServer,
    temp: &tempfile::TempDir,
    certificate: Option<&russh::keys::Certificate>,
) -> SshAuth {
    let key_file = temp.path().join("id_ed25519");
    server.write_client_key(&key_file, None);
    if let Some(certificate) = certificate {
        support::write_certificate_beside(&key_file, certificate);
    }
    SshAuth::PrivateKey {
        key_file,
        passphrase: None,
    }
}

#[tokio::test]
async fn a_key_file_with_its_certificate_beside_it_logs_in_by_the_certificate() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let certificate = support::user_certificate(
        server.client_key().public_key(),
        CERT_USERNAME,
        support::user_ca(),
    );
    let auth = key_with_certificate(&server, &temp, Some(&certificate));

    let state = state_after_connecting(&server, CERT_USERNAME, auth).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn a_certificate_accepted_as_a_first_step_is_followed_by_the_password() {
    let server = TestSshServer::start().await;
    let manager = support::manager();
    let temp = tempfile::TempDir::new().unwrap();
    // A key the server does not know: only its certificate can be the
    // first step.
    let key = random_key();
    let key_file = temp.path().join("id_ed25519");
    let text = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .unwrap();
    std::fs::write(&key_file, text.as_bytes()).unwrap();
    let certificate = support::user_certificate(
        key.public_key(),
        KEY_THEN_PASSWORD_USERNAME,
        support::user_ca(),
    );
    support::write_certificate_beside(&key_file, &certificate);
    let auth = SshAuth::PrivateKey {
        key_file,
        passphrase: None,
    };
    let (id, mut events) = open(&manager, &server, KEY_THEN_PASSWORD_USERNAME, auth);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);

    assert_eq!(next_prompt(&mut events).await, password_prompt());
    manager.answer(id, vec![PASSWORD.into()]).unwrap();

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn without_its_certificate_the_key_alone_is_refused_where_only_certificates_count() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let auth = key_with_certificate(&server, &temp, None);

    let state = state_after_connecting(&server, CERT_USERNAME, auth).await;

    assert_eq!(
        state,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

#[tokio::test]
async fn a_certificate_the_server_does_not_trust_falls_back_to_the_key() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    // Signed by an authority the server does not know.
    let stranger = random_key();
    let certificate =
        support::user_certificate(server.client_key().public_key(), USERNAME, &stranger);
    let auth = key_with_certificate(&server, &temp, Some(&certificate));

    let state = state_after_connecting(&server, USERNAME, auth).await;

    // USERNAME knows the key itself.
    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn a_certificate_beside_the_key_file_that_is_for_another_key_is_not_used() {
    let server = TestSshServer::start().await;
    let temp = tempfile::TempDir::new().unwrap();
    let other = random_key();
    let certificate =
        support::user_certificate(other.public_key(), CERT_USERNAME, support::user_ca());
    let auth = key_with_certificate(&server, &temp, Some(&certificate));

    let state = state_after_connecting(&server, CERT_USERNAME, auth).await;

    assert_eq!(
        state,
        SessionState::Closed(CloseReason::Failed(OpenError::AuthenticationFailed))
    );
}

#[tokio::test]
async fn a_certificate_held_by_the_agent_logs_in() {
    let server = TestSshServer::start().await;
    let key = random_key();
    let certificate =
        support::user_certificate(key.public_key(), CERT_USERNAME, support::user_ca());
    let agent = TestAgent::start_with_certificate(key, certificate).await;

    let state = state_after_connecting(&server, CERT_USERNAME, agent_at(&agent)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn a_security_key_held_by_the_agent_logs_in() {
    // As a YubiKey's key in OpenSSH's agent: the agent lists an
    // sk-ssh-ed25519@openssh.com key and signs as the device does; the
    // server knows only the security key.
    let server = TestSshServer::start().await;
    let agent = TestAgent::start_as_security_key(server.client_key().clone()).await;

    let state =
        state_after_connecting(&server, support::SECURITY_KEY_USERNAME, agent_at(&agent)).await;

    assert_eq!(state, SessionState::Connected);
}

#[tokio::test]
async fn a_security_key_is_not_taken_for_the_plain_key() {
    // The plain Ed25519 key with the same public point is not the
    // security key: the server refuses it for the security-key user.
    let server = TestSshServer::start().await;
    let agent = TestAgent::start(&[server.client_key().clone()]).await;

    let state =
        state_after_connecting(&server, support::SECURITY_KEY_USERNAME, agent_at(&agent)).await;

    assert_eq!(state, agent_failure(AgentError::KeysRefused));
}
