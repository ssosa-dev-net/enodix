//! Host certificates (#26): a server that presents an OpenSSH host
//! certificate signed by an authority of a `@cert-authority` line of the
//! known hosts connects without a host-key question; a certificate that is
//! not accepted falls back to the key inside it, as OpenSSH does, and the
//! question says why. A key of a `@revoked` line is refused outright. The
//! known hosts are in a temporary data folder.

mod support;

use std::ops::Range;

use enodix_core::known_hosts::{KnownHosts, FILE_NAME};
use enodix_core::session::{
    CloseReason, OpenError, SessionEvents, SessionManager, SessionState, SessionTarget,
    TerminalSize,
};
use russh::keys::ssh_key::certificate::CertType;
use russh::keys::{Algorithm, HashAlg, PrivateKey};
use support::{
    certificate, host_ca, next_host_key_question, next_state, output_ending_with, unix_now,
    TestSshServer, PROMPT,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// How the test server's host certificate is made.
struct Signed {
    cert_type: CertType,
    principals: &'static [&'static str],
    ca: &'static PrivateKey,
    valid: Range<u64>,
}

impl Signed {
    /// A host certificate for 127.0.0.1 by [`host_ca`], valid now.
    fn good() -> Self {
        let now = unix_now();
        Self {
            cert_type: CertType::Host,
            principals: &["127.0.0.1"],
            ca: host_ca(),
            valid: now - 60..now + 3600,
        }
    }
}

/// A test server presenting a host certificate made as `signed` says.
async fn server_with(signed: Signed) -> TestSshServer {
    TestSshServer::start_tuned(move |config| {
        let key = config.keys[0].public_key().clone();
        config.certificates.push(certificate(
            &key,
            signed.cert_type,
            signed.principals,
            signed.ca,
            signed.valid,
        ));
    })
    .await
}

/// A session manager whose known hosts hold `lines` only.
fn manager_with(lines: &str) -> (TempDir, SessionManager) {
    let data = TempDir::new().unwrap();
    std::fs::write(data.path().join(FILE_NAME), lines).unwrap();
    let manager = SessionManager::new(KnownHosts::in_data_folder(data.path()));
    (data, manager)
}

fn authority_line(patterns: &str, ca: &PrivateKey) -> String {
    format!(
        "@cert-authority {patterns} {}\n",
        ca.public_key().to_openssh().unwrap()
    )
}

async fn open(manager: &SessionManager, server: &TestSshServer) -> SessionEvents {
    let (_id, mut events) = manager.open(SessionTarget::Ssh(server.target()), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    events
}

fn fingerprint(server: &TestSshServer) -> String {
    server.host_key().fingerprint(HashAlg::Sha256).to_string()
}

#[tokio::test]
async fn a_host_certificate_of_a_trusted_authority_connects_without_a_question() {
    let server = server_with(Signed::good()).await;
    let (_data, manager) = manager_with(&authority_line("*", host_ca()));

    let mut events = open(&manager, &server).await;

    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

#[tokio::test]
async fn an_authority_for_other_hosts_does_not_count() {
    // The pattern does not name 127.0.0.1: no certificate is asked for,
    // and the plain key is asked about as on any first connection.
    let server = server_with(Signed::good()).await;
    let (_data, manager) = manager_with(&authority_line("*.lab,!127.0.0.1", host_ca()));

    let mut events = open(&manager, &server).await;

    let question = next_host_key_question(&mut events).await;
    assert_eq!(question.key.fingerprint, fingerprint(&server));
    assert_eq!(question.certificate, None);
}

#[tokio::test]
async fn a_certificate_of_another_authority_falls_back_to_its_key_and_says_why() {
    static STRANGER: std::sync::OnceLock<PrivateKey> = std::sync::OnceLock::new();
    let stranger =
        STRANGER.get_or_init(|| PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap());
    let server = server_with(Signed {
        ca: stranger,
        ..Signed::good()
    })
    .await;
    let (_data, manager) = manager_with(&authority_line("*", host_ca()));

    let mut events = open(&manager, &server).await;

    let question = next_host_key_question(&mut events).await;
    assert_eq!(question.key.fingerprint, fingerprint(&server));
    assert_eq!(
        question.certificate.as_deref(),
        Some("no trusted certificate authority signed it")
    );
    assert!(!question.changed());
}

#[tokio::test]
async fn an_expired_certificate_is_not_accepted() {
    let now = unix_now();
    let server = server_with(Signed {
        valid: now - 7200..now - 3600,
        ..Signed::good()
    })
    .await;
    let (_data, manager) = manager_with(&authority_line("*", host_ca()));

    let mut events = open(&manager, &server).await;

    let question = next_host_key_question(&mut events).await;
    assert_eq!(
        question.certificate.as_deref(),
        Some("it is not valid now: expired, not valid yet, or badly signed")
    );
}

#[tokio::test]
async fn a_certificate_for_another_host_is_not_accepted() {
    let server = server_with(Signed {
        principals: &["core-r1.lab"],
        ..Signed::good()
    })
    .await;
    let (_data, manager) = manager_with(&authority_line("*", host_ca()));

    let mut events = open(&manager, &server).await;

    let question = next_host_key_question(&mut events).await;
    assert_eq!(
        question.certificate.as_deref(),
        Some("it is not for 127.0.0.1")
    );
}

#[tokio::test]
async fn a_user_certificate_is_no_host_certificate() {
    let server = server_with(Signed {
        cert_type: CertType::User,
        ..Signed::good()
    })
    .await;
    let (_data, manager) = manager_with(&authority_line("*", host_ca()));

    let mut events = open(&manager, &server).await;

    let question = next_host_key_question(&mut events).await;
    assert_eq!(
        question.certificate.as_deref(),
        Some("it is not a host certificate")
    );
}

#[tokio::test]
async fn a_damaged_authority_line_for_the_host_fails_the_connection() {
    let server = server_with(Signed::good()).await;
    let (_data, manager) = manager_with("@cert-authority * ssh-ed25519 not-a-key\n");

    let mut events = open(&manager, &server).await;

    let state = next_state(&mut events).await;
    assert!(
        matches!(
            &state,
            SessionState::Closed(CloseReason::Failed(OpenError::KnownHosts(reason)))
                if reason.contains("certificate authority key")
        ),
        "{state:?}"
    );
}

fn revoked_line(key: &russh::keys::PublicKey) -> String {
    format!("@revoked * {}\n", key.to_openssh().unwrap())
}

fn refused_as_revoked(state: &SessionState) -> bool {
    matches!(
        state,
        SessionState::Closed(CloseReason::Failed(OpenError::KnownHosts(reason)))
            if reason.contains("revoked")
    )
}

#[tokio::test]
async fn a_revoked_host_key_is_refused_even_when_trusted() {
    let server = TestSshServer::start().await;
    let key = server.host_key().clone();
    let (_data, manager) = manager_with(&format!(
        "[127.0.0.1]:{} {}\n{}",
        server.target().port,
        key.to_openssh().unwrap(),
        revoked_line(&key),
    ));

    let mut events = open(&manager, &server).await;

    let state = next_state(&mut events).await;
    assert!(refused_as_revoked(&state), "{state:?}");
}

#[tokio::test]
async fn a_certificate_of_a_revoked_authority_is_refused() {
    let server = server_with(Signed::good()).await;
    let (_data, manager) = manager_with(&format!(
        "{}{}",
        authority_line("*", host_ca()),
        revoked_line(host_ca().public_key()),
    ));

    let mut events = open(&manager, &server).await;

    let state = next_state(&mut events).await;
    assert!(refused_as_revoked(&state), "{state:?}");
}

#[tokio::test]
async fn a_certificate_whose_key_is_revoked_is_refused() {
    let server = server_with(Signed::good()).await;
    let key = server.host_key().clone();
    let (_data, manager) = manager_with(&format!(
        "{}{}",
        authority_line("*", host_ca()),
        revoked_line(&key),
    ));

    let mut events = open(&manager, &server).await;

    let state = next_state(&mut events).await;
    assert!(refused_as_revoked(&state), "{state:?}");
}

#[test]
fn revoked_keys_are_listed_for_the_hosts_their_patterns_name() {
    let (data, _manager) = manager_with(&format!(
        "@revoked *.lab {}\n{}",
        host_ca().public_key().to_openssh().unwrap(),
        authority_line("*", host_ca()),
    ));
    let known = KnownHosts::in_data_folder(data.path());

    assert_eq!(known.revoked("r1.lab", 22).unwrap().len(), 1);
    assert_eq!(known.revoked("core1", 22).unwrap().len(), 0);
}

/// `r7.lab` hashed as `ssh-keygen -H` hashes it (HMAC-SHA1, salt 30 to 49).
const HASHED_R7: &str = "|1|Hh8gISIjJCUmJygpKissLS4vMDE=|MuYth0WRCeRDLwGkrEn6Tv8Vv4k=";

#[test]
fn authorities_are_listed_for_the_hosts_their_patterns_name() {
    let (data, _manager) = manager_with(&format!(
        "{}{}r9 {}\n{}",
        authority_line("*.lab,!r2.lab", host_ca()),
        authority_line("[10.0.0.*]:2222", host_ca()),
        host_ca().public_key().to_openssh().unwrap(),
        authority_line(HASHED_R7, host_ca()),
    ));
    let known = KnownHosts::in_data_folder(data.path());
    let count = |host: &str, port: u16| known.certificate_authorities(host, port).unwrap().len();

    assert_eq!(count("r1.lab", 22), 1);
    assert_eq!(count("R1.LAB", 22), 1);
    // The hashed line names r7.lab only, besides `*.lab`.
    assert_eq!(count("r7.lab", 22), 2);
    assert_eq!(count("r7.lab", 2222), 0);
    assert_eq!(count("r2.lab", 22), 0);
    assert_eq!(count("r1.lab", 2222), 0);
    assert_eq!(count("10.0.0.7", 2222), 1);
    assert_eq!(count("10.0.0.7", 22), 0);
    // A plain key line is no authority.
    assert_eq!(count("r9", 22), 0);
    // Reading them changes nothing in the file.
    known
        .remember("r3.lab", 22, host_ca().public_key())
        .unwrap();
    let text = std::fs::read_to_string(data.path().join(FILE_NAME)).unwrap();
    assert!(
        text.starts_with("@cert-authority *.lab,!r2.lab ssh-ed25519 "),
        "{text}"
    );
}
