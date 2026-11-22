//! The keychain credential store against the real Windows Credential
//! Manager. Every entry these tests write is named with a throwaway
//! `enodix-test-<random>` prefix and deleted when the test ends, pass or
//! fail; nothing else in the user's keychain is read or changed.
//!
//! Several processes using the keychain at once (#138) are played by this
//! test binary run again as helpers (`support/processes.rs`); what killed
//! test processes left is swept at the start (`support/keychain.rs`).
#![cfg(windows)]

mod support;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

use enodix_core::credentials::{
    vault_file, ActiveStore, AuthDraft, CredentialProfile, CredentialStore, KeychainStore,
    ProfileAuth, ProfileDraft, Profiles, RemovedEntries, SecretValue, Secrets, StoreContents,
    Vault, VaultStatus,
};
use enodix_core::workspace::Workspace;
use support::keychain::{prefix_of, sweep_leftovers, LEFTOVER_AGE, TEST_PREFIX};
use support::processes;

const SCOPE_A: &str = r"C:\enodix-test\customer-a";
const SCOPE_B: &str = r"C:\enodix-test\customer-b";

/// A keychain store under a fresh test prefix that deletes every entry
/// (profiles and secrets) of the test scopes when dropped.
struct TestKeychain {
    store: KeychainStore,
    prefix: String,
}

impl TestKeychain {
    fn new() -> Self {
        let suffix: u64 = rand::random();
        Self::with_prefix(format!("{TEST_PREFIX}{suffix:016x}"))
    }

    fn with_prefix(prefix: String) -> Self {
        assert!(prefix.starts_with(TEST_PREFIX));
        sweep_leftovers();
        Self {
            store: KeychainStore::new(prefix.clone()),
            prefix,
        }
    }
}

impl Drop for TestKeychain {
    fn drop(&mut self) {
        for scope in [SCOPE_A, SCOPE_B] {
            clear(&self.store, scope);
        }
        // Whatever else a test that failed part-way left under the prefix,
        // readable or not: leftovers fill Credential Manager until it
        // refuses every write ("not enough memory resources").
        let _ = self.store.remove_all();
    }
}

fn clear(store: &KeychainStore, scope: &str) {
    for profile in store.list(scope).unwrap_or_default() {
        let _ = store.delete(scope, &profile.name);
    }
    for name in store.secret_names(scope).unwrap_or_default() {
        let _ = store.delete_secret(scope, &name);
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
    assert!(store.list(r"C:\enodix-test\nobody").unwrap().is_empty());
}

#[test]
fn a_profile_too_large_for_the_keychain_is_refused() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;

    let result = store.put(SCOPE_A, &password_profile("huge", &"x".repeat(4000)));

    assert!(result.is_err());
    assert_eq!(store.get(SCOPE_A, "huge").unwrap(), None);
}

/// Deletes every profile of the scopes when dropped.
struct Cleanup<'a> {
    store: &'a KeychainStore,
    scopes: Vec<String>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for scope in &self.scopes {
            clear(self.store, scope);
        }
    }
}

#[test]
fn two_workspaces_with_a_profile_of_the_same_name_do_not_collide_in_the_keychain() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;
    let parent = tempfile::TempDir::new().unwrap();
    let acme = Workspace::create(parent.path().join("acme")).unwrap();
    let globex = Workspace::create(parent.path().join("globex")).unwrap();
    let _cleanup = Cleanup {
        store,
        scopes: vec![acme.credential_scope(), globex.credential_scope()],
    };
    let draft = |username: &str| ProfileDraft {
        name: "core".into(),
        username: username.into(),
        auth: AuthDraft::Password {
            password: Some("pw".into()),
        },
        enable_secret: None,
    };

    Profiles::new(store, &acme)
        .save(None, &draft("acme-admin"))
        .unwrap();
    Profiles::new(store, &globex)
        .save(None, &draft("globex-admin"))
        .unwrap();

    let username = |workspace: &Workspace| {
        Profiles::new(store, workspace)
            .profile("core")
            .unwrap()
            .map(|profile| profile.username)
    };
    assert_eq!(username(&acme).as_deref(), Some("acme-admin"));
    assert_eq!(username(&globex).as_deref(), Some("globex-admin"));
}

#[test]
fn secrets_are_written_read_listed_and_deleted_beside_profiles() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;
    let value = |scope: &str, name: &str| {
        store
            .get_secret(scope, name)
            .unwrap()
            .map(|value| value.expose().to_owned())
    };

    store
        .put_secret(SCOPE_A, "NetBox-Token", &SecretValue::new("t0kén"))
        .unwrap();
    store
        .put_secret(SCOPE_A, "backup.key", &SecretValue::new("k3y"))
        .unwrap();
    store
        .put_secret(SCOPE_B, "netbox-token", &SecretValue::new("other"))
        .unwrap();
    // A profile of the same name is another entry.
    store
        .put(SCOPE_A, &password_profile("netbox-token", "pw"))
        .unwrap();

    assert_eq!(value(SCOPE_A, "netbox-token").as_deref(), Some("t0kén"));
    assert_eq!(value(SCOPE_B, "NETBOX-TOKEN").as_deref(), Some("other"));
    let mut names = store.secret_names(SCOPE_A).unwrap();
    names.sort();
    assert_eq!(names, ["NetBox-Token", "backup.key"]);
    assert_eq!(store.list(SCOPE_A).unwrap().len(), 1);

    store
        .put_secret(SCOPE_A, "NetBox-Token", &SecretValue::new("r0tated"))
        .unwrap();
    assert_eq!(value(SCOPE_A, "netbox-token").as_deref(), Some("r0tated"));

    assert!(store.delete_secret(SCOPE_A, "netbox-token").unwrap());
    assert!(!store.delete_secret(SCOPE_A, "netbox-token").unwrap());
    assert_eq!(value(SCOPE_A, "netbox-token"), None);
    assert_eq!(store.secret_names(SCOPE_A).unwrap(), ["backup.key"]);
    assert_eq!(
        store.get(SCOPE_A, "netbox-token").unwrap(),
        Some(password_profile("netbox-token", "pw"))
    );
    assert!(store.delete(SCOPE_A, "netbox-token").unwrap());
    assert_eq!(value(SCOPE_B, "netbox-token").as_deref(), Some("other"));
}

#[test]
fn a_workspace_keeps_its_secrets_in_the_keychain_and_leaves_nothing_behind() {
    let keychain = TestKeychain::new();
    let store = &keychain.store;
    let parent = tempfile::TempDir::new().unwrap();
    let acme = Workspace::create(parent.path().join("acme")).unwrap();
    let scope = acme.credential_scope();
    {
        let _cleanup = Cleanup {
            store,
            scopes: vec![scope.clone()],
        };
        let secrets = Secrets::new(store, &acme);
        secrets.add("api-token", "s3cret").unwrap();
        secrets.replace("API-TOKEN", "r0tated").unwrap();

        assert_eq!(secrets.list().unwrap(), ["api-token"]);
        assert_eq!(
            secrets
                .get("api-token")
                .unwrap()
                .map(|v| v.expose().to_owned()),
            Some("r0tated".into())
        );
    }
    assert!(store.secret_names(&scope).unwrap().is_empty());
    assert!(store.list(&scope).unwrap().is_empty());
}

#[test]
fn everything_is_listed_across_scopes_and_only_under_the_prefix() {
    let keychain = TestKeychain::new();
    let other = TestKeychain::new();
    let store = &keychain.store;
    store
        .put(SCOPE_A, &password_profile("core", "pw-a"))
        .unwrap();
    store
        .put(SCOPE_B, &password_profile("core", "pw-b"))
        .unwrap();
    store
        .put_secret(SCOPE_B, "Token", &SecretValue::new("t0ken"))
        .unwrap();
    other
        .store
        .put(SCOPE_A, &password_profile("elsewhere", "x"))
        .unwrap();

    let everything = store.contents().unwrap().sorted();
    let profiles: Vec<_> = everything
        .profiles
        .iter()
        .map(|p| (p.scope.as_str(), p.profile.name.as_str()))
        .collect();
    assert_eq!(profiles, [(SCOPE_A, "core"), (SCOPE_B, "core")]);
    let secrets: Vec<_> = everything
        .secrets
        .iter()
        .map(|s| (s.scope.as_str(), s.name.as_str(), s.value.expose()))
        .collect();
    assert_eq!(secrets, [(SCOPE_B, "Token", "t0ken")]);
}

// The vault's moves over the real Credential Manager (#125): what was only
// tested on the memory stand-in.

const MASTER: &str = "correct horse battery staple";

/// Two profiles and a secret in two scopes; what the keychain then holds.
fn fill(store: &KeychainStore) -> StoreContents {
    store
        .put(SCOPE_A, &password_profile("core", "pw-a"))
        .unwrap();
    store
        .put(SCOPE_B, &password_profile("core", "pw-b"))
        .unwrap();
    store
        .put_secret(SCOPE_B, "Token", &SecretValue::new("t0ken"))
        .unwrap();
    store.contents().unwrap().sorted()
}

fn nothing_left(store: &KeychainStore) {
    assert_eq!(store.count_all().unwrap(), RemovedEntries::default());
}

#[test]
fn the_vault_moves_everything_out_of_the_real_keychain_and_back() {
    let keychain = TestKeychain::new();
    let everything = fill(&keychain.store);
    let data = tempfile::TempDir::new().unwrap();
    let store = ActiveStore::new(Box::new(keychain.store.clone()), vault_file(data.path()));

    store.turn_on(MASTER, MASTER).unwrap();
    nothing_left(&keychain.store);
    assert_eq!(store.contents().unwrap().sorted(), everything);
    let vault = Vault::unlock(&vault_file(data.path()), MASTER).unwrap();
    assert_eq!(vault.contents().unwrap().sorted(), everything);

    store.turn_off(MASTER).unwrap();
    assert!(!vault_file(data.path()).exists());
    assert_eq!(keychain.store.contents().unwrap().sorted(), everything);
}

#[test]
fn turning_off_with_a_profile_too_large_for_a_windows_credential_names_it_and_moves_nothing() {
    let keychain = TestKeychain::new();
    let data = tempfile::TempDir::new().unwrap();
    let store = ActiveStore::new(Box::new(keychain.store.clone()), vault_file(data.path()));
    store.turn_on(MASTER, MASTER).unwrap();
    store
        .put(SCOPE_A, &password_profile("core", "pw-a"))
        .unwrap();
    // A pasted private key, say: more than a credential's 2,560 bytes.
    store
        .put(SCOPE_B, &password_profile("Huge Key", &"k".repeat(3000)))
        .unwrap();

    let error = store.turn_off(MASTER).unwrap_err().to_string();
    assert!(error.contains("\"Huge Key\""), "{error}");
    assert!(error.contains("2560"), "{error}");
    assert!(error.contains("nothing was moved"), "{error}");
    nothing_left(&keychain.store);
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(store.contents().unwrap().profiles.len(), 2);
}

#[test]
fn copies_left_in_the_real_keychain_go_at_the_next_unlock_and_with_a_start_over() {
    let keychain = TestKeychain::new();
    let everything = fill(&keychain.store);
    let data = tempfile::TempDir::new().unwrap();
    let path = vault_file(data.path());
    // A turn on cut short after its vault was written.
    drop(Vault::create(&path, MASTER).unwrap());
    let vault = Vault::unlock(&path, MASTER).unwrap();
    for entry in &everything.profiles {
        vault.put(&entry.scope, &entry.profile).unwrap();
    }
    drop(vault);

    let store = ActiveStore::new(Box::new(keychain.store.clone()), &path);
    store.unlock(MASTER).unwrap();
    nothing_left(&keychain.store);
    // The secret only the keychain had is in the vault now.
    assert_eq!(store.contents().unwrap().sorted(), everything);
    drop(store);

    // Copies again, and the master password forgotten: starting over
    // leaves no profile to come back.
    fill(&keychain.store);
    let store = ActiveStore::new(Box::new(keychain.store.clone()), &path);
    store.start_over().unwrap();
    nothing_left(&keychain.store);
    assert!(store.contents().unwrap().is_empty());
}

#[test]
fn everything_under_the_prefix_is_removed_and_nothing_else() {
    let suffix: u64 = rand::random();
    let prefix = format!("enodix-test-{suffix:016x}");
    let keychain = TestKeychain::with_prefix(prefix.clone());
    // Names that start like the prefix's own but are another prefix's.
    let longer = TestKeychain::with_prefix(format!("{prefix}x"));
    let other = TestKeychain::new();
    let store = &keychain.store;
    store
        .put(SCOPE_A, &password_profile("core", "pw-a"))
        .unwrap();
    store
        .put(SCOPE_B, &password_profile("core", "pw-b"))
        .unwrap();
    store
        .put(SCOPE_B, &password_profile("edge", "pw-e"))
        .unwrap();
    store
        .put_secret(SCOPE_A, "Token", &SecretValue::new("t0ken"))
        .unwrap();
    for kept in [&longer.store, &other.store] {
        kept.put(SCOPE_A, &password_profile("core", "x")).unwrap();
        kept.put_secret(SCOPE_A, "Token", &SecretValue::new("y"))
            .unwrap();
    }

    // Counted first, removing nothing.
    let counted = store.count_all().unwrap();
    assert_eq!((counted.profiles, counted.secrets), (3, 1));
    assert_eq!(store.contents().unwrap().profiles.len(), 3);

    let removed = store.remove_all().unwrap();

    assert_eq!(removed, counted);
    assert!(store.contents().unwrap().is_empty());
    for kept in [&longer.store, &other.store] {
        assert_eq!(kept.list(SCOPE_A).unwrap().len(), 1);
        assert_eq!(kept.secret_names(SCOPE_A).unwrap(), ["Token"]);
    }
    // Nothing left: nothing more to remove.
    let again = store.remove_all().unwrap();
    assert_eq!((again.profiles, again.secrets), (0, 0));
    assert_eq!(store.count_all().unwrap(), again);
}

// What killed test processes leave (#138).

#[test]
fn a_test_entry_is_named_after_its_prefix_and_nothing_else_is_one() {
    assert_eq!(
        prefix_of(r"enodix-test-0a1b:C:\ws:core"),
        Some("enodix-test-0a1b")
    );
    assert_eq!(
        prefix_of("enodix-test-0a1b.secret:ws:token"),
        Some("enodix-test-0a1b")
    );
    assert_eq!(
        prefix_of("enodix-test-0a1bx:ws:core"),
        Some("enodix-test-0a1bx")
    );
    for other in [
        "dev.enodix.app:ws:core",
        "dev.enodix.app.secret:ws:token",
        "ENODIX-TEST-0a1b:ws:core",
        "enodix-test-:ws:core",
        "enodix-test-no-colon",
        "x-enodix-test-0a1b:ws:core",
    ] {
        assert_eq!(prefix_of(other), None, "{other}");
    }
}

#[test]
fn the_sweep_keeps_what_a_running_test_wrote() {
    let keychain = TestKeychain::new();
    keychain
        .store
        .put(SCOPE_A, &password_profile("core", "pw"))
        .unwrap();
    keychain
        .store
        .put_secret(SCOPE_A, "Token", &SecretValue::new("t0ken"))
        .unwrap();

    support::keychain::sweep(LEFTOVER_AGE).unwrap();

    assert_eq!(
        keychain.store.count_all().unwrap(),
        RemovedEntries {
            profiles: 1,
            secrets: 1
        }
    );
}

// Several processes using the keychain at once (#138).

/// The role of a helper that uses the keychain under its own prefix.
const KEYCHAIN_USER: &str = "keychain-user";
/// The helper's prefix and how many rounds it plays.
const PREFIX_VARIABLE: &str = "ENODIX_TEST_KEYCHAIN_PREFIX";
const ROUNDS_VARIABLE: &str = "ENODIX_TEST_KEYCHAIN_ROUNDS";
/// What a helper prints for each answer that is not what it just did, and
/// at the end.
const INCONSISTENT: &str = "inconsistent";
const REFUSED: &str = "refused";
const FINISHED: &str = "finished";

#[test]
fn helper() {
    processes::play();
    if processes::role().as_deref() == Some(KEYCHAIN_USER) {
        let prefix = std::env::var(PREFIX_VARIABLE).unwrap();
        let rounds = std::env::var(ROUNDS_VARIABLE).unwrap().parse().unwrap();
        let keychain = TestKeychain::with_prefix(prefix);
        use_the_keychain(&keychain.store, rounds);
        drop(keychain);
        std::process::exit(0);
    }
}

/// Rounds of what the app does with a profile and a secret: saved, read
/// back, changed, read back, listed, deleted, looked for, listed. Every
/// answer that is not what this process itself just did is printed
/// (`inconsistent <what>: expected …, got …`), and every call that failed
/// (`refused <what>: <error>`).
fn use_the_keychain(store: &KeychainStore, rounds: usize) {
    let mut inconsistent = 0;
    let mut refused = 0;
    let mut check = |what: &str, expected: String, got: Result<String, String>| match got {
        Ok(got) if got == expected => {}
        Ok(got) => {
            inconsistent += 1;
            println!("{INCONSISTENT} {what}: expected {expected}, got {got}");
        }
        Err(error) => {
            refused += 1;
            println!("{REFUSED} {what}: {error}");
        }
    };
    let password = |profile: Option<CredentialProfile>| match profile.map(|p| p.auth) {
        Some(ProfileAuth::Password { password }) => password,
        Some(_) => "another kind of login".into(),
        None => "nothing".into(),
    };
    let done = |result: Result<(), _>| result.map(|()| "done".to_owned());
    for round in 0..rounds {
        let first = format!("first-{round}");
        let second = format!("second-{round}");
        let token = format!("token-{round}");
        check(
            "save",
            "done".into(),
            done(
                store
                    .put(SCOPE_A, &password_profile("core", &first))
                    .map_err(|e| e.to_string()),
            ),
        );
        check(
            "read after saving",
            first,
            store
                .get(SCOPE_A, "core")
                .map(password)
                .map_err(|e| e.to_string()),
        );
        check(
            "change",
            "done".into(),
            done(
                store
                    .put(SCOPE_A, &password_profile("core", &second))
                    .map_err(|e| e.to_string()),
            ),
        );
        check(
            "read after changing",
            second.clone(),
            store
                .get(SCOPE_A, "core")
                .map(password)
                .map_err(|e| e.to_string()),
        );
        check(
            "secret saved",
            "done".into(),
            done(
                store
                    .put_secret(SCOPE_B, "Token", &SecretValue::new(token.clone()))
                    .map_err(|e| e.to_string()),
            ),
        );
        check(
            "everything listed",
            format!("[{second}] [{token}]"),
            store
                .contents()
                .map(|everything| {
                    let passwords: Vec<String> = everything
                        .profiles
                        .into_iter()
                        .map(|entry| password(Some(entry.profile)))
                        .collect();
                    let values: Vec<String> = everything
                        .secrets
                        .iter()
                        .map(|entry| entry.value.expose().to_owned())
                        .collect();
                    format!("[{}] [{}]", passwords.join(", "), values.join(", "))
                })
                .map_err(|e| e.to_string()),
        );
        check(
            "delete",
            "true true".into(),
            store
                .delete(SCOPE_A, "core")
                .and_then(|profile| Ok((profile, store.delete_secret(SCOPE_B, "Token")?)))
                .map(|(profile, secret)| format!("{profile} {secret}"))
                .map_err(|e| e.to_string()),
        );
        check(
            "read after deleting",
            "nothing".into(),
            store
                .get(SCOPE_A, "core")
                .map(password)
                .map_err(|e| e.to_string()),
        );
        check(
            "listed after deleting",
            "0 0".into(),
            store
                .count_all()
                .map(|left| format!("{} {}", left.profiles, left.secrets))
                .map_err(|e| e.to_string()),
        );
    }
    println!("{FINISHED} {rounds} rounds, {inconsistent} inconsistent, {refused} refused");
}

/// `processes` helpers at once, each [`use_the_keychain`] for `rounds`
/// under a prefix of its own; every line they printed about an answer
/// that was not what they had just done.
fn several_processes_use_the_keychain(processes: usize, rounds: usize) -> Vec<String> {
    let pids = tempfile::TempDir::new().unwrap();
    let keychains: Vec<TestKeychain> = (0..processes).map(|_| TestKeychain::new()).collect();
    let started = Instant::now();
    let children: Vec<_> = keychains
        .iter()
        .map(|keychain| {
            processes::helper(KEYCHAIN_USER, pids.path())
                .env(PREFIX_VARIABLE, &keychain.prefix)
                .env(ROUNDS_VARIABLE, rounds.to_string())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut problems = Vec::new();
    let mut finished = 0;
    for child in children {
        let output = child.wait_with_output().unwrap();
        let printed = String::from_utf8_lossy(&output.stdout);
        for line in printed.lines() {
            if line.starts_with(INCONSISTENT) || line.starts_with(REFUSED) {
                problems.push(line.to_owned());
            } else if line.starts_with(FINISHED) {
                finished += 1;
                eprintln!("{line}");
            }
        }
        assert!(output.status.success(), "a helper failed: {printed}");
    }
    assert_eq!(finished, processes, "every helper played every round");
    eprintln!(
        "{processes} processes x {rounds} rounds in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    problems
}

#[test]
fn several_processes_using_the_keychain_at_once_each_read_back_what_they_wrote() {
    let problems = several_processes_use_the_keychain(4, 10);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The reproduction of #138, minutes of load on Credential Manager: run
/// by hand (`cargo test -p enodix-core --test keychain under_load --
/// --ignored --nocapture`; `ENODIX_TEST_KEYCHAIN_PROCESSES` and
/// `ENODIX_TEST_KEYCHAIN_STRESS_ROUNDS` change its 16 processes and 100
/// rounds). Before every call took turns, 16 processes saw old values,
/// missing entries and lost changes in about one round in a hundred.
#[test]
#[ignore = "minutes of load on Credential Manager: run by hand"]
fn several_processes_using_the_keychain_at_once_each_read_back_what_they_wrote_under_load() {
    let number = |name: &str, default: usize| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let problems = several_processes_use_the_keychain(
        number("ENODIX_TEST_KEYCHAIN_PROCESSES", 16),
        number("ENODIX_TEST_KEYCHAIN_STRESS_ROUNDS", 100),
    );
    assert!(
        problems.is_empty(),
        "{} problems: {problems:#?}",
        problems.len()
    );
}
