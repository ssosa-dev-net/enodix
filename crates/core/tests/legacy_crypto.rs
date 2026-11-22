//! The legacy-crypto switch: with it off, sessions offer only modern
//! algorithms; with it on, also the outdated ones old equipment needs, so
//! they reach test servers restricted to those. A failed negotiation says
//! what the device offered and whether the switch would help, and the
//! negotiated algorithms mark a session that uses outdated ones.

mod support;

use std::fs;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::session::{
    offered_algorithms, AlgorithmKind, CloseReason, NegotiatedAlgorithms, NegotiationFailure,
    OpenError, SessionEvents, SessionManager, SessionState, SessionTarget, SshTarget, TerminalSize,
    UNSUPPORTED_ALGORITHMS,
};
use enodix_core::workspace::{HostId, Workspace};
use russh::keys::Algorithm;
use russh::{cipher, kex, mac, Preferred};
use support::algorithms::{old_device, FakeDevice, Offer};
use support::{
    manager, next_algorithms, next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT,
    USERNAME,
};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

/// The outdated algorithms the switch is for, as the spec lists them:
/// SHA-1 key exchanges, SHA-1 RSA and DSA host keys, CBC ciphers, 3DES and
/// SHA-1 MACs.
const LEGACY: &[&str] = &[
    "diffie-hellman-group1-sha1",
    "diffie-hellman-group14-sha1",
    "diffie-hellman-group-exchange-sha1",
    "ssh-rsa",
    "ssh-dss",
    "aes128-cbc",
    "aes192-cbc",
    "aes256-cbc",
    "3des-cbc",
    "hmac-sha1",
    "hmac-sha1-etm@openssh.com",
];

/// Opens a session; it lasts as long as the manager given back.
async fn open(target: SshTarget) -> (SessionManager, SessionEvents) {
    let manager = manager();
    let (_id, mut events) = manager.open(SessionTarget::Ssh(target), SIZE);
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    (manager, events)
}

fn target(port: u16, legacy_crypto: bool) -> SshTarget {
    SshTarget {
        port,
        legacy_crypto,
        ..support_target()
    }
}

fn support_target() -> SshTarget {
    support::target_at("127.0.0.1", 0)
}

/// Connects, and gives the algorithms of the device's connection.
async fn connects(target: SshTarget) -> NegotiatedAlgorithms {
    let (_manager, mut events) = open(target).await;
    let mut algorithms = next_algorithms(&mut events).await;
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
    assert_eq!(algorithms.len(), 1);
    algorithms.pop().unwrap()
}

async fn close_reason(target: SshTarget) -> CloseReason {
    let (_manager, mut events) = open(target).await;
    match next_state(&mut events).await {
        SessionState::Closed(reason) => reason,
        other => panic!("expected the session to fail, got {other:?}"),
    }
}

async fn negotiation_fails(target: SshTarget) -> NegotiationFailure {
    match close_reason(target).await {
        CloseReason::Failed(OpenError::Negotiation(failure)) => failure,
        other => panic!("expected a failed negotiation, got {other:?}"),
    }
}

#[tokio::test]
async fn with_the_switch_off_only_modern_algorithms_are_offered() {
    let server = TestSshServer::start().await;

    let device = connects(target(server.port(), false)).await;

    let offer = server.client_offer().expect("the client's offer");
    for name in offer.all() {
        assert!(!LEGACY.contains(&name), "{name} was offered: {offer:?}");
        assert!(!UNSUPPORTED_ALGORITHMS.contains(&name), "{name}: {offer:?}");
    }
    assert!(offer.kex.contains(&"curve25519-sha256".to_owned()));
    assert!(offer.host_key.contains(&"ssh-ed25519".to_owned()));
    assert!(offer.cipher.contains(&"aes256-ctr".to_owned()));
    assert!(offer.mac.contains(&"hmac-sha2-256".to_owned()));
    assert!(device.legacy.is_empty(), "{device:?}");
}

#[tokio::test]
async fn with_the_switch_on_legacy_algorithms_are_offered_after_the_modern_ones() {
    let server = TestSshServer::start().await;

    let device = connects(target(server.port(), true)).await;

    let offer = server.client_offer().expect("the client's offer");
    for name in LEGACY {
        assert!(offer.all().contains(name), "{name} not offered: {offer:?}");
    }
    for list in [&offer.kex, &offer.host_key, &offer.cipher, &offer.mac] {
        let first_legacy = list.iter().position(|n| LEGACY.contains(&n.as_str()));
        let last_modern = list
            .iter()
            .rposition(|n| !LEGACY.contains(&n.as_str()) && !is_extension(n));
        assert!(last_modern < first_legacy, "legacy before modern: {list:?}");
    }
    // A device that speaks modern algorithms still gets them.
    assert!(device.legacy.is_empty(), "{device:?}");
    assert_eq!(device.key_exchange, offer.kex[0]);
}

/// Not algorithms: what the client supports.
fn is_extension(name: &str) -> bool {
    name.starts_with("ext-info-") || name.starts_with("kex-strict-")
}

#[tokio::test]
async fn the_offered_lists_are_public_and_match_what_is_sent() {
    let server = TestSshServer::start().await;
    for legacy_crypto in [false, true] {
        connects(target(server.port(), legacy_crypto)).await;
        let offer = server.client_offer().unwrap();
        let offered = offered_algorithms(legacy_crypto);
        let kex: Vec<String> = offer.kex.into_iter().filter(|n| !is_extension(n)).collect();
        assert_eq!(offered.key_exchange, kex);
        assert_eq!(offered.host_key, offer.host_key);
        assert_eq!(offered.cipher, offer.cipher);
        assert_eq!(offered.mac, offer.mac);
    }
}

#[test]
fn documented_unsupported_algorithms_are_never_offered() {
    let offered = offered_algorithms(true);
    for list in [
        &offered.key_exchange,
        &offered.host_key,
        &offered.cipher,
        &offered.mac,
    ] {
        for name in list {
            assert!(!UNSUPPORTED_ALGORITHMS.contains(&name.as_str()), "{name}");
        }
    }
    assert!(UNSUPPORTED_ALGORITHMS.contains(&"hmac-md5"));
}

/// A server that offers only `legacy` for one kind of algorithm, and its
/// defaults for the others (a CTR cipher for MAC cases: an AEAD cipher
/// needs no MAC).
fn restricted_to(kind: AlgorithmKind, legacy: &str) -> Preferred {
    let defaults = Preferred::default();
    match kind {
        AlgorithmKind::KeyExchange => Preferred {
            kex: vec![kex::Name::try_from(legacy).unwrap()].into(),
            ..defaults
        },
        AlgorithmKind::HostKey => Preferred {
            key: vec![Algorithm::new(legacy).unwrap()].into(),
            ..defaults
        },
        AlgorithmKind::Cipher => Preferred {
            cipher: vec![cipher::Name::try_from(legacy).unwrap()].into(),
            ..defaults
        },
        AlgorithmKind::Mac => Preferred {
            cipher: vec![cipher::AES_128_CTR].into(),
            mac: vec![mac::Name::try_from(legacy).unwrap()].into(),
            ..defaults
        },
    }
}

/// With the switch off, a server restricted to the outdated `name` (of
/// `kind`) refuses: the failure names the kind, lists what the server
/// offered and says the switch would help. With it on, the session
/// connects and is marked as using `name`.
async fn reaches_only_with_the_switch(kind: AlgorithmKind, name: &str) {
    let server = TestSshServer::start_with_algorithms(restricted_to(kind, name)).await;

    let failure = negotiation_fails(target(server.port(), false)).await;
    assert_eq!(failure.kind, kind);
    let offered = &failure.offered;
    let list = match kind {
        AlgorithmKind::KeyExchange => &offered.key_exchange,
        AlgorithmKind::HostKey => &offered.host_key,
        AlgorithmKind::Cipher => &offered.cipher,
        AlgorithmKind::Mac => &offered.mac,
    };
    assert_eq!(list, &[name.to_owned()]);
    for list in [
        &offered.key_exchange,
        &offered.host_key,
        &offered.cipher,
        &offered.mac,
    ] {
        assert!(!list.is_empty(), "{offered:?}");
    }
    assert!(!failure.legacy_crypto);
    assert!(failure.legacy_crypto_would_help());
    let message = OpenError::Negotiation(failure.clone()).to_string();
    assert!(message.contains("turning on legacy crypto"), "{message}");
    assert!(message.contains(name), "{message}");

    let device = connects(target(server.port(), true)).await;
    assert_eq!(device.legacy, [name.to_owned()], "{device:?}");
    assert_eq!(device.jump_host, None);
}

macro_rules! legacy_only_servers {
    ($($test:ident: $kind:ident $name:literal;)*) => {$(
        #[tokio::test]
        async fn $test() {
            reaches_only_with_the_switch(AlgorithmKind::$kind, $name).await;
        }
    )*};
}

legacy_only_servers! {
    kex_diffie_hellman_group1_sha1: KeyExchange "diffie-hellman-group1-sha1";
    kex_diffie_hellman_group14_sha1: KeyExchange "diffie-hellman-group14-sha1";
    kex_diffie_hellman_group_exchange_sha1: KeyExchange "diffie-hellman-group-exchange-sha1";
    host_key_ssh_rsa: HostKey "ssh-rsa";
    host_key_ssh_dss: HostKey "ssh-dss";
    cipher_aes128_cbc: Cipher "aes128-cbc";
    cipher_aes192_cbc: Cipher "aes192-cbc";
    cipher_aes256_cbc: Cipher "aes256-cbc";
    cipher_3des_cbc: Cipher "3des-cbc";
    mac_hmac_sha1: Mac "hmac-sha1";
    mac_hmac_sha1_etm: Mac "hmac-sha1-etm@openssh.com";
}

/// A server configured down to old algorithms of every kind at once, as a
/// switch from the 2000s.
#[tokio::test]
async fn a_server_with_only_old_algorithms_is_reached_with_the_switch_on() {
    let server = TestSshServer::start_with_algorithms(Preferred {
        kex: vec![kex::DH_G14_SHA1, kex::DH_G1_SHA1].into(),
        key: vec![Algorithm::Dsa].into(),
        cipher: vec![cipher::TRIPLE_DES_CBC, cipher::AES_128_CBC].into(),
        mac: vec![mac::HMAC_SHA1].into(),
        ..Preferred::default()
    })
    .await;

    let failure = negotiation_fails(target(server.port(), false)).await;
    assert_eq!(failure.kind, AlgorithmKind::KeyExchange);
    assert_eq!(
        failure.offered.key_exchange,
        ["diffie-hellman-group14-sha1", "diffie-hellman-group1-sha1"]
    );
    assert_eq!(failure.offered.host_key, ["ssh-dss"]);
    assert_eq!(failure.offered.cipher, ["3des-cbc", "aes128-cbc"]);
    assert_eq!(failure.offered.mac, ["hmac-sha1"]);
    assert!(failure.legacy_crypto_would_help());

    let device = connects(target(server.port(), true)).await;
    assert_eq!(device.key_exchange, "diffie-hellman-group14-sha1");
    assert_eq!(device.host_key, "ssh-dss");
    // The client's preference (CBC before 3DES) decides.
    assert_eq!(device.cipher, "aes128-cbc");
    assert_eq!(device.mac, "hmac-sha1");
    assert_eq!(
        device.legacy,
        [
            "diffie-hellman-group14-sha1",
            "ssh-dss",
            "aes128-cbc",
            "hmac-sha1"
        ]
    );
}

/// A device whose MACs are all MD5-based: `russh` has none of them, so
/// the switch cannot help, and the failure says so either way.
fn md5_only_device() -> Offer {
    Offer {
        kex: vec![
            "diffie-hellman-group14-sha1".into(),
            "diffie-hellman-group1-sha1".into(),
        ],
        host_key: vec!["ssh-rsa".into()],
        cipher: vec!["aes128-cbc".into(), "3des-cbc".into()],
        mac: vec!["hmac-md5".into(), "hmac-md5-96".into()],
    }
}

#[tokio::test]
async fn a_device_with_only_unsupported_algorithms_is_told_the_switch_would_not_help() {
    let device = FakeDevice::start(md5_only_device()).await;

    let failure = negotiation_fails(target(device.port(), false)).await;
    assert_eq!(failure.kind, AlgorithmKind::KeyExchange);
    assert!(!failure.legacy_crypto_would_help());
    assert_eq!(
        failure.unsupported_even_with_legacy_crypto(),
        Some(AlgorithmKind::Mac)
    );
    let message = OpenError::Negotiation(failure.clone()).to_string();
    assert!(
        message.contains("legacy crypto would not help"),
        "{message}"
    );
    assert!(message.contains("hmac-md5, hmac-md5-96"), "{message}");

    let failure = negotiation_fails(target(device.port(), true)).await;
    assert_eq!(failure.kind, AlgorithmKind::Mac);
    assert!(failure.legacy_crypto);
    assert!(!failure.legacy_crypto_would_help());
    let message = OpenError::Negotiation(failure).to_string();
    assert!(message.contains("even with legacy crypto on"), "{message}");
}

#[tokio::test]
async fn a_failed_negotiation_lists_everything_the_device_offered() {
    let mut offer = md5_only_device();
    // What OpenSSH adds to its key exchanges; not algorithms, so not shown.
    offer.kex.push("ext-info-s".into());
    offer.kex.push("kex-strict-s-v00@openssh.com".into());
    let device = FakeDevice::start(offer).await;

    let failure = negotiation_fails(target(device.port(), false)).await;

    assert_eq!(
        failure.offered.key_exchange,
        ["diffie-hellman-group14-sha1", "diffie-hellman-group1-sha1"]
    );
    assert_eq!(failure.offered.host_key, ["ssh-rsa"]);
    assert_eq!(failure.offered.cipher, ["aes128-cbc", "3des-cbc"]);
    assert_eq!(failure.offered.mac, ["hmac-md5", "hmac-md5-96"]);
    let message = OpenError::Negotiation(failure).to_string();
    assert!(
        message.contains(
            "key exchange: diffie-hellman-group14-sha1, diffie-hellman-group1-sha1; \
             host key: ssh-rsa; cipher: aes128-cbc, 3des-cbc; MAC: hmac-md5, hmac-md5-96"
        ),
        "{message}"
    );
}

/// A workspace made of `files` (relative path, text).
fn workspace(files: &[(&str, String)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("legacy");
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

fn host(port: u16, extra: &str) -> String {
    format!("address: 127.0.0.1\nport: {port}\ncredential_profile: lab\n{extra}")
}

fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    let profile = CredentialProfile {
        name: "lab".into(),
        username: USERNAME.into(),
        auth: ProfileAuth::Password {
            password: PASSWORD.into(),
        },
        enable_secret: None,
    };
    store.put(&workspace.credential_scope(), &profile).unwrap();
    store
}

/// The switch set on a folder applies to its hosts; a jump host elsewhere
/// keeps its own (off), and each connection is marked on its own.
#[tokio::test]
async fn the_switch_on_a_folder_applies_to_its_hosts_and_not_to_their_jump_hosts() {
    let bastion = TestSshServer::start().await;
    let old = TestSshServer::start_with_algorithms(old_device()).await;
    let (_temp, workspace) = workspace(&[
        ("bastion.yaml", host(bastion.port(), "")),
        (
            "Old/_folder.yaml",
            "legacy_crypto: true\njump_host: bastion.yaml\n".into(),
        ),
        ("Old/switch.yaml", host(old.port(), "")),
    ]);
    let store = store(&workspace);

    let target = Profiles::new(&store, &workspace)
        .profile_target(&HostId::new("Old/switch.yaml"))
        .unwrap();
    assert!(target.legacy_crypto);
    assert!(!target.jump_hosts[0].legacy_crypto);

    let (_manager, mut events) = open(target).await;
    let algorithms = next_algorithms(&mut events).await;
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    assert_eq!(algorithms.len(), 2);
    assert_eq!(algorithms[0].jump_host.as_deref(), Some("bastion.yaml"));
    assert!(algorithms[0].legacy.is_empty(), "{algorithms:?}");
    assert_eq!(algorithms[1].jump_host, None);
    assert_eq!(
        algorithms[1].legacy,
        [
            "diffie-hellman-group14-sha1",
            "ssh-rsa",
            "aes128-cbc",
            "hmac-sha1"
        ]
    );
    assert_eq!(bastion.forwarded(), 1);
}

/// An old jump host without the switch fails as a jump host, with what it
/// offered.
#[tokio::test]
async fn an_old_jump_host_without_the_switch_fails_naming_it() {
    let old_bastion = TestSshServer::start_with_algorithms(old_device()).await;
    let router = TestSshServer::start().await;
    let (_temp, workspace) = workspace(&[
        ("bastion.yaml", host(old_bastion.port(), "")),
        (
            "router.yaml",
            host(
                router.port(),
                "jump_host: bastion.yaml\nlegacy_crypto: true\n",
            ),
        ),
    ]);
    let store = store(&workspace);
    let target = Profiles::new(&store, &workspace)
        .profile_target(&HostId::new("router.yaml"))
        .unwrap();

    let reason = close_reason(target).await;

    let (name, failure) = reason.negotiation_failure().expect("a failed negotiation");
    assert_eq!(name, Some("bastion.yaml"));
    assert_eq!(failure.kind, AlgorithmKind::KeyExchange);
    assert_eq!(failure.offered.host_key, ["ssh-rsa"]);
    assert!(failure.legacy_crypto_would_help());
    assert!(
        reason.to_string().starts_with("jump host bastion.yaml: "),
        "{reason}"
    );
}

#[tokio::test]
async fn other_failures_are_not_negotiation_failures() {
    let server = TestSshServer::start().await;
    let reason = close_reason(SshTarget {
        auth: enodix_core::session::SshAuth::Password("wrong".into()),
        ..target(server.port(), false)
    })
    .await;
    assert_eq!(reason.negotiation_failure(), None);
}
