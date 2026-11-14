//! The keychain credential store against the real Windows Credential
//! Manager. Every entry these tests write is named with a throwaway
//! `netterm-test-<random>` prefix and deleted when the test ends, pass or
//! fail; nothing else in the user's keychain is read or changed.
#![cfg(windows)]

use std::path::PathBuf;

use netterm_core::credentials::{CredentialProfile, CredentialStore, KeychainStore, ProfileAuth};

const SCOPE_A: &str = r"C:\netterm-test\customer-a";
const SCOPE_B: &str = r"C:\netterm-test\customer-b";

/// A keychain store under a fresh test prefix that deletes every entry of
/// the test scopes when dropped.
struct TestKeychain {
    store: KeychainStore,
}

impl TestKeychain {
    fn new() -> Self {
        let suffix: u64 = rand::random();
        Self {
            store: KeychainStore::new(format!("netterm-test-{suffix:016x}")),
        }
    }
}

impl Drop for TestKeychain {
    fn drop(&mut self) {
        for scope in [SCOPE_A, SCOPE_B] {
            for profile in self.store.list(scope).unwrap_or_default() {
                let _ = self.store.delete(scope, &profile.name);
            }
        }
    }
}

fn password_profile(name: &str, password: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: "noc".into(),
        auth: ProfileAuth::Password {
            password: password.into(),
        },
        enable_secret: Some("en4ble".into()),
    }
}

#[test]
fn profiles_are_written_read_listed_and_deleted() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;
    let key_profile = CredentialProfile {
        name: "Backbone Key".into(),
        username: "ops".into(),
        auth: ProfileAuth::PrivateKey {
            key_file: PathBuf::from(r"C:\keys\id_ed25519"),
            passphrase: Some("pässphrase".into()),
        },
        enable_secret: None,
    };

    store
        .put(SCOPE_A, &password_profile("lab-admin", "pw1"))
        .unwrap();
    store.put(SCOPE_A, &key_profile).unwrap();

    assert_eq!(
        store.get(SCOPE_A, "lab-admin").unwrap(),
        Some(password_profile("lab-admin", "pw1"))
    );
    // Names match ignoring case.
    assert_eq!(
        store.get(SCOPE_A, "backbone key").unwrap(),
        Some(key_profile.clone())
    );
    let mut names: Vec<String> = store
        .list(SCOPE_A)
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    names.sort();
    assert_eq!(names, ["Backbone Key", "lab-admin"]);

    // Writing the same name again replaces it.
    store
        .put(SCOPE_A, &password_profile("LAB-ADMIN", "pw2"))
        .unwrap();
    assert_eq!(
        store.get(SCOPE_A, "lab-admin").unwrap(),
        Some(password_profile("LAB-ADMIN", "pw2"))
    );
    assert_eq!(store.list(SCOPE_A).unwrap().len(), 2);

    assert!(store.delete(SCOPE_A, "lab-admin").unwrap());
    assert!(!store.delete(SCOPE_A, "lab-admin").unwrap());
    assert_eq!(store.get(SCOPE_A, "lab-admin").unwrap(), None);
    assert_eq!(store.list(SCOPE_A).unwrap(), [key_profile]);
}

#[test]
fn each_scope_keeps_its_own_profiles() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;

    store
        .put(SCOPE_A, &password_profile("shared-name", "a"))
        .unwrap();
    store
        .put(SCOPE_B, &password_profile("shared-name", "b"))
        .unwrap();

    assert_eq!(
        store.get(SCOPE_A, "shared-name").unwrap(),
        Some(password_profile("shared-name", "a"))
    );
    assert_eq!(
        store.get(SCOPE_B, "shared-name").unwrap(),
        Some(password_profile("shared-name", "b"))
    );
    assert_eq!(store.list(SCOPE_B).unwrap().len(), 1);
    assert!(store.list(r"C:\netterm-test\nobody").unwrap().is_empty());
}

#[test]
fn a_profile_too_large_for_the_keychain_is_refused() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;

    let result = store.put(SCOPE_A, &password_profile("huge", &"x".repeat(4000)));

    assert!(result.is_err());
    assert_eq!(store.get(SCOPE_A, "huge").unwrap(), None);
}
