//! Host-key checking: trust on first use, remembered keys, and changed keys,
//! against in-process test servers with different host keys. The known keys
//! live in a temporary data folder, as the app keeps them in the per-user
//! application data folder.

mod support;

use enodix_core::known_hosts::{HostKey, HostKeyStatus, KnownHosts, FILE_NAME};
use enodix_core::session::{
    CloseReason, HostKeyDecision, HostKeyQuestion, OpenError, SessionEvents, SessionId,
    SessionManager, SessionState, SessionTarget, TerminalSize,
};
use russh::keys::{Algorithm, PrivateKey};
use support::{next_host_key_question, next_state, output_ending_with, TestSshServer, PROMPT};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

async fn open(manager: &SessionManager, server: &TestSshServer) -> (SessionId, SessionEvents) {
    let (id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    (id, events)
}

/// Waits for the session to connect and its shell to print a prompt.
async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

fn known_hosts_text(data: &TempDir) -> String {
    std::fs::read_to_string(data.path().join(FILE_NAME)).unwrap()
}

#[tokio::test]
async fn the_first_connection_shows_the_key_type_and_fingerprint() {
    let server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));

    let (_id, mut events) = open(&manager, &server).await;
    let question = next_host_key_question(&mut events).await;

    assert_eq!(
        question,
        HostKeyQuestion {
            host: "127.0.0.1".into(),
            port: server.port(),
            key: HostKey {
                algorithm: "ssh-ed25519".into(),
                fingerprint: server
                    .host_key()
                    .fingerprint(russh::keys::HashAlg::Sha256)
                    .to_string(),
            },
            stored: Vec::new(),
        }
    );
    assert!(question.key.fingerprint.starts_with("SHA256:"));
    assert!(!question.changed());
}

#[tokio::test]
async fn a_key_accepted_and_remembered_is_not_asked_about_again() {
    let server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));

    let (id, mut events) = open(&manager, &server).await;
    next_host_key_question(&mut events).await;
    manager
        .decide_host_key(id, HostKeyDecision::TrustAndRemember)
        .unwrap();
    connected(&mut events).await;

    // The key is in the data folder, in OpenSSH's known_hosts format.
    let text = known_hosts_text(&data);
    assert!(
        text.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 ", server.port())),
        "{text}"
    );

    // A later launch reads it back and connects without asking.
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));
    let (_id, mut events) = open(&manager, &server).await;
    connected(&mut events).await;
}

#[tokio::test]
async fn a_key_accepted_once_connects_but_is_asked_about_next_time() {
    let server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));

    let (id, mut events) = open(&manager, &server).await;
    next_host_key_question(&mut events).await;
    manager
        .decide_host_key(id, HostKeyDecision::TrustOnce)
        .unwrap();
    connected(&mut events).await;
    assert!(!data.path().join(FILE_NAME).exists());

    let (_id, mut events) = open(&manager, &server).await;
    assert!(!next_host_key_question(&mut events).await.changed());
}

#[tokio::test]
async fn cancelling_on_first_use_refuses_the_connection_and_remembers_nothing() {
    let server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());
    let manager = SessionManager::new(known_hosts.clone());

    let (id, mut events) = open(&manager, &server).await;
    next_host_key_question(&mut events).await;
    manager
        .decide_host_key(id, HostKeyDecision::Cancel)
        .unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::HostKeyNotTrusted))
    );
    assert_eq!(
        known_hosts
            .check("127.0.0.1", server.port(), server.host_key())
            .unwrap(),
        HostKeyStatus::Unknown
    );
}

#[tokio::test]
async fn closing_while_the_host_key_question_waits_ends_the_session() {
    let server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));

    let (id, mut events) = open(&manager, &server).await;
    next_host_key_question(&mut events).await;
    manager.close(id).unwrap();

    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::ClosedLocally)
    );
    assert!(!data.path().join(FILE_NAME).exists());
}

/// Connects to `server` once and remembers its key.
async fn remember(manager: &SessionManager, server: &TestSshServer) {
    let (id, mut events) = open(manager, server).await;
    next_host_key_question(&mut events).await;
    manager
        .decide_host_key(id, HostKeyDecision::TrustAndRemember)
        .unwrap();
    connected(&mut events).await;
    manager.close(id).unwrap();
}

#[tokio::test]
async fn a_changed_key_blocks_the_connection_and_shows_both_fingerprints() {
    let mut server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());
    let manager = SessionManager::new(known_hosts.clone());
    remember(&manager, &server).await;
    let old_key = server.host_key().clone();

    // Another server, with another key, answers on the same address.
    server.replace_with_new_host_key().await;
    assert_ne!(server.host_key().key_data(), old_key.key_data());
    let (id, mut events) = open(&manager, &server).await;
    let question = next_host_key_question(&mut events).await;

    assert!(question.changed());
    assert_eq!(question.key, HostKey::of(server.host_key()));
    assert_eq!(question.stored, vec![HostKey::of(&old_key)]);
    assert_ne!(question.key.fingerprint, question.stored[0].fingerprint);

    // Nothing proceeds without a decision: no state change, no output.
    let waited = tokio::time::timeout(std::time::Duration::from_millis(300), events.next()).await;
    assert!(waited.is_err(), "the session went on undecided: {waited:?}");

    manager
        .decide_host_key(id, HostKeyDecision::Cancel)
        .unwrap();
    assert_eq!(
        next_state(&mut events).await,
        SessionState::Closed(CloseReason::Failed(OpenError::HostKeyChanged))
    );
    // The stored key stays.
    assert_eq!(
        known_hosts
            .check("127.0.0.1", server.port(), &old_key)
            .unwrap(),
        HostKeyStatus::Trusted
    );
}

#[tokio::test]
async fn a_changed_key_accepted_once_keeps_the_stored_key() {
    let mut server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));
    remember(&manager, &server).await;
    server.replace_with_new_host_key().await;

    let (id, mut events) = open(&manager, &server).await;
    next_host_key_question(&mut events).await;
    manager
        .decide_host_key(id, HostKeyDecision::TrustOnce)
        .unwrap();
    connected(&mut events).await;

    let (_id, mut events) = open(&manager, &server).await;
    assert!(next_host_key_question(&mut events).await.changed());
}

#[tokio::test]
async fn replacing_a_changed_key_stores_the_new_one_only() {
    let mut server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());
    let manager = SessionManager::new(known_hosts.clone());
    remember(&manager, &server).await;
    let old_key = server.host_key().clone();
    server.replace_with_new_host_key().await;

    let (id, mut events) = open(&manager, &server).await;
    next_host_key_question(&mut events).await;
    manager
        .decide_host_key(id, HostKeyDecision::TrustAndRemember)
        .unwrap();
    connected(&mut events).await;

    // The next connection does not ask, and the old key is gone.
    let (_id, mut events) = open(&manager, &server).await;
    connected(&mut events).await;
    assert_eq!(known_hosts_text(&data).lines().count(), 1);
    assert!(matches!(
        known_hosts
            .check("127.0.0.1", server.port(), &old_key)
            .unwrap(),
        HostKeyStatus::Changed { .. }
    ));
}

#[tokio::test]
async fn an_unreadable_known_hosts_file_fails_the_connection_instead_of_asking() {
    let server = TestSshServer::start().await;
    let data = TempDir::new().unwrap();
    // A folder where the file should be: it cannot be read.
    std::fs::create_dir(data.path().join(FILE_NAME)).unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));

    let (_id, mut events) = open(&manager, &server).await;

    match next_state(&mut events).await {
        SessionState::Closed(CloseReason::Failed(OpenError::KnownHosts(reason))) => {
            assert!(reason.contains(FILE_NAME), "{reason}");
        }
        other => panic!("expected a known-hosts failure, got {other:?}"),
    }
}

fn random_key() -> russh::keys::PublicKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .unwrap()
        .public_key()
        .clone()
}

#[test]
fn keys_are_per_host_and_port_and_host_names_ignore_case() {
    let known_hosts = KnownHosts::in_memory();
    let key = random_key();
    known_hosts
        .remember("Router1.Example.net", 22, &key)
        .unwrap();

    assert_eq!(
        known_hosts.check("router1.example.net", 22, &key).unwrap(),
        HostKeyStatus::Trusted
    );
    assert_eq!(
        known_hosts
            .check("router1.example.net", 2222, &key)
            .unwrap(),
        HostKeyStatus::Unknown
    );
    assert_eq!(
        known_hosts.check("router2.example.net", 22, &key).unwrap(),
        HostKeyStatus::Unknown
    );
    let other = random_key();
    assert_eq!(
        known_hosts
            .check("router1.example.net", 22, &other)
            .unwrap(),
        HostKeyStatus::Changed {
            stored: vec![HostKey::of(&key)]
        }
    );
}

#[test]
fn a_damaged_entry_for_the_host_is_an_error_not_an_unknown_host() {
    let data = TempDir::new().unwrap();
    std::fs::write(
        data.path().join(FILE_NAME),
        "[10.0.0.2]:830 ssh-ed25519 AAAA-truncated\n",
    )
    .unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());
    let key = random_key();

    let error = known_hosts.check("10.0.0.2", 830, &key).unwrap_err();
    assert!(error.to_string().contains("[10.0.0.2]:830"), "{error}");
    // Other hosts are not affected.
    assert_eq!(
        known_hosts.check("10.0.0.3", 830, &key).unwrap(),
        HostKeyStatus::Unknown
    );
}

#[test]
fn the_file_uses_the_openssh_format_and_keeps_lines_it_does_not_change() {
    let data = TempDir::new().unwrap();
    let file = data.path().join(FILE_NAME);
    let other = random_key().to_openssh().unwrap();
    let old = random_key();
    let original = format!(
        "# kept\n\
         switch1,10.0.0.1 {other}\n\
         router1,[10.0.0.2]:830 {old_line}\n\
         |1|hashed= {other}\n\
         not a key\n",
        old_line = old.to_openssh().unwrap(),
    );
    std::fs::write(&file, &original).unwrap();
    let known_hosts = KnownHosts::in_data_folder(data.path());

    // Port 22 is written as the bare name, like OpenSSH; others in brackets.
    assert_eq!(
        known_hosts
            .check("10.0.0.1", 22, &other.parse().unwrap())
            .unwrap(),
        HostKeyStatus::Trusted
    );
    assert_eq!(
        known_hosts.check("10.0.0.2", 830, &old).unwrap(),
        HostKeyStatus::Trusted
    );

    let new = random_key();
    known_hosts.remember("10.0.0.2", 830, &new).unwrap();

    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            "# kept".to_owned(),
            format!("switch1,10.0.0.1 {other}"),
            // The other name on that line keeps the old key.
            format!("router1 {}", old.to_openssh().unwrap()),
            format!("|1|hashed= {other}"),
            "not a key".to_owned(),
            format!("[10.0.0.2]:830 {}", new.to_openssh().unwrap()),
        ]
    );
    assert_eq!(
        known_hosts.check("10.0.0.2", 830, &new).unwrap(),
        HostKeyStatus::Trusted
    );
    assert_eq!(
        known_hosts.check("router1", 22, &old).unwrap(),
        HostKeyStatus::Trusted
    );
}
