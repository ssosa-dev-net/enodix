//! The vault: the app's own encrypted credential store behind a master
//! password, in a temporary folder. The store behaviour checks the memory
//! store passes run against it; then unlocking (a wrong password, a damaged
//! file), what the file holds, and turning the vault on, which moves every
//! profile and secret out of the keychain (the memory store, or a failing
//! one, stands in for it) all or nothing.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use netterm_core::credentials::{
    prepare_workspace, vault_file, ActiveStore, AuthDraft, CredentialProfile, CredentialStore,
    KdfParams, MemoryStore, ProfileAuth, ProfileDraft, ProfileError, Profiles, SecretValue,
    Secrets, StoreContents, Vault, VaultError, VaultStatus, VAULT_FILE,
};
use netterm_core::session::{SessionState, SessionTarget, TerminalSize};
use netterm_core::workspace::{HostId, Workspace};
use support::credentials::{FailingStore, Failures};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

const MASTER: &str = "correct horse battery staple";
const SCOPE_A: &str = "workspace-a";
const SCOPE_B: &str = "workspace-b";

fn password_profile(name: &str, password: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: "noc".into(),
        auth: ProfileAuth::Password {
            password: password.into(),
        },
        enable_secret: Some("en4ble-s3cret".into()),
    }
}

fn key_profile(name: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: "automation".into(),
        auth: ProfileAuth::PrivateKey {
            key_file: PathBuf::from(r"C:\keys\id_ed25519"),
            passphrase: Some("k3y-passphr4se".into()),
        },
        enable_secret: None,
    }
}

fn secret(value: &str) -> SecretValue {
    SecretValue::new(value)
}

fn value(store: &dyn CredentialStore, scope: &str, name: &str) -> Option<String> {
    store
        .get_secret(scope, name)
        .unwrap()
        .map(|value| value.expose().to_owned())
}

fn sorted_names(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

/// The checks every credential store passes: profiles and secrets kept by
/// name ignoring case, scopes apart, a profile and a secret sharing a name,
/// everything listed across scopes.
fn behaves_as_a_store(store: &dyn CredentialStore) {
    assert!(store.list(SCOPE_A).unwrap().is_empty());
    assert_eq!(store.get(SCOPE_A, "core").unwrap(), None);

    store
        .put(SCOPE_A, &password_profile("Core-RW", "pw-a"))
        .unwrap();
    store.put(SCOPE_A, &key_profile("automation")).unwrap();
    store
        .put(SCOPE_B, &password_profile("core-rw", "pw-b"))
        .unwrap();

    assert_eq!(
        store.get(SCOPE_A, "CORE-rw").unwrap(),
        Some(password_profile("Core-RW", "pw-a"))
    );
    assert_eq!(
        store.get(SCOPE_B, "Core-RW").unwrap(),
        Some(password_profile("core-rw", "pw-b"))
    );
    let mut names: Vec<_> = store
        .list(SCOPE_A)
        .unwrap()
        .into_iter()
        .map(|profile| profile.name)
        .collect();
    names.sort();
    assert_eq!(names, ["Core-RW", "automation"]);

    // Replacing by a name in other case keeps one profile, the new one.
    store
        .put(SCOPE_A, &password_profile("core-rw", "pw-new"))
        .unwrap();
    assert_eq!(store.list(SCOPE_A).unwrap().len(), 2);
    assert_eq!(
        store.get(SCOPE_A, "Core-RW").unwrap(),
        Some(password_profile("core-rw", "pw-new"))
    );

    store
        .put_secret(SCOPE_A, "NetBox-Token", &secret("t0kén"))
        .unwrap();
    store
        .put_secret(SCOPE_A, "core-rw", &secret("same name"))
        .unwrap();
    store
        .put_secret(SCOPE_B, "netbox-token", &secret("other"))
        .unwrap();
    assert_eq!(
        value(store, SCOPE_A, "netbox-TOKEN").as_deref(),
        Some("t0kén")
    );
    assert_eq!(
        value(store, SCOPE_B, "NetBox-Token").as_deref(),
        Some("other")
    );
    assert_eq!(
        sorted_names(store.secret_names(SCOPE_A).unwrap()),
        ["NetBox-Token", "core-rw"]
    );
    store
        .put_secret(SCOPE_A, "netbox-token", &secret("r0tated"))
        .unwrap();
    assert_eq!(
        value(store, SCOPE_A, "NetBox-Token").as_deref(),
        Some("r0tated")
    );
    assert_eq!(store.secret_names(SCOPE_A).unwrap().len(), 2);

    let everything = store.contents().unwrap().sorted();
    assert_eq!(everything.profiles.len(), 3);
    assert_eq!(everything.secrets.len(), 3);
    assert_eq!(
        everything
            .secrets
            .iter()
            .map(|s| (s.scope.as_str(), s.name.as_str(), s.value.expose()))
            .collect::<Vec<_>>(),
        [
            (SCOPE_A, "core-rw", "same name"),
            (SCOPE_A, "netbox-token", "r0tated"),
            (SCOPE_B, "netbox-token", "other"),
        ]
    );

    // Deleting a secret leaves the profile of the same name, and the other
    // way round.
    assert!(store.delete_secret(SCOPE_A, "CORE-RW").unwrap());
    assert!(!store.delete_secret(SCOPE_A, "core-rw").unwrap());
    assert!(store.get(SCOPE_A, "core-rw").unwrap().is_some());
    assert!(store.delete(SCOPE_A, "CORE-RW").unwrap());
    assert!(!store.delete(SCOPE_A, "core-rw").unwrap());
    assert_eq!(store.get(SCOPE_A, "core-rw").unwrap(), None);
    assert!(store.get(SCOPE_B, "core-rw").unwrap().is_some());
    assert_eq!(
        value(store, SCOPE_A, "netbox-token").as_deref(),
        Some("r0tated")
    );
}

/// A temporary data folder.
struct DataFolder {
    temp: TempDir,
}

impl DataFolder {
    fn new() -> Self {
        Self {
            temp: TempDir::new().unwrap(),
        }
    }

    fn vault(&self) -> PathBuf {
        vault_file(self.temp.path())
    }

    /// Every file in the folder, by name.
    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

fn filled_keychain(store: &dyn CredentialStore) -> StoreContents {
    store
        .put(SCOPE_A, &password_profile("core", "pw-a"))
        .unwrap();
    store.put(SCOPE_A, &key_profile("automation")).unwrap();
    store
        .put(SCOPE_B, &password_profile("core", "pw-b"))
        .unwrap();
    store
        .put_secret(SCOPE_A, "netbox-token", &secret("t0ken-a"))
        .unwrap();
    store
        .put_secret(SCOPE_B, "Backup.Key", &secret("t0ken-b"))
        .unwrap();
    store.contents().unwrap().sorted()
}

fn unlocked(path: &Path) -> Vault {
    Vault::unlock(path, MASTER).unwrap()
}

#[test]
fn the_memory_store_passes_the_store_checks() {
    behaves_as_a_store(&MemoryStore::new());
}

#[test]
fn a_vault_passes_the_store_checks_and_keeps_everything_when_reopened() {
    let data = DataFolder::new();
    let vault = Vault::create(&data.vault(), MASTER).unwrap();
    behaves_as_a_store(&vault);
    let before = vault.contents().unwrap().sorted();
    drop(vault);

    let reopened = unlocked(&data.vault());
    assert_eq!(reopened.contents().unwrap().sorted(), before);
    assert_eq!(reopened.kdf(), KdfParams::default());
    assert_eq!(
        data.files(),
        ["credentials.vault"],
        "no temporary file left"
    );
}

#[test]
fn the_active_store_passes_the_store_checks_with_and_without_a_vault() {
    let data = DataFolder::new();
    let keychain = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(keychain.status(), VaultStatus::Off);
    behaves_as_a_store(&keychain);
    assert!(data.files().is_empty(), "no vault without turning it on");

    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    assert_eq!(store.status(), VaultStatus::Unlocked);
    behaves_as_a_store(&store);
}

#[test]
fn a_wrong_password_is_reported_and_the_right_one_still_unlocks() {
    let data = DataFolder::new();
    let vault = Vault::create(&data.vault(), MASTER).unwrap();
    vault.put(SCOPE_A, &password_profile("core", "pw")).unwrap();
    drop(vault);

    assert_eq!(
        Vault::unlock(&data.vault(), "correct horse battery stapler").unwrap_err(),
        VaultError::WrongPassword
    );
    assert_eq!(
        Vault::unlock(&data.vault(), "").unwrap_err(),
        VaultError::WrongPassword
    );

    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(store.status(), VaultStatus::Locked);
    assert_eq!(
        store.unlock("guess").unwrap_err(),
        VaultError::WrongPassword
    );
    assert_eq!(store.status(), VaultStatus::Locked);
    store.unlock(MASTER).unwrap();
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert!(store.get(SCOPE_A, "core").unwrap().is_some());
    // Unlocking again still checks the password.
    assert_eq!(
        store.unlock("anything").unwrap_err(),
        VaultError::WrongPassword
    );
    store.unlock(MASTER).unwrap();
    assert_eq!(store.status(), VaultStatus::Unlocked);
}

#[test]
fn a_locked_vault_refuses_reads_and_writes() {
    let data = DataFolder::new();
    drop(Vault::create(&data.vault(), MASTER).unwrap());
    let keychain = MemoryStore::new();
    keychain
        .put(SCOPE_A, &password_profile("core", "pw"))
        .unwrap();
    let store = ActiveStore::new(Box::new(keychain), data.vault());

    let error = store.list(SCOPE_A).unwrap_err();
    assert!(error.0.contains("locked"), "{error}");
    assert!(store.get(SCOPE_A, "core").is_err());
    assert!(store.put(SCOPE_A, &password_profile("x", "y")).is_err());
    assert!(store.secret_names(SCOPE_A).is_err());
    assert!(store.put_secret(SCOPE_A, "token", &secret("v")).is_err());
}

#[test]
fn a_truncated_or_altered_file_is_damaged_not_a_wrong_password() {
    let data = DataFolder::new();
    let vault = Vault::create(&data.vault(), MASTER).unwrap();
    vault.put(SCOPE_A, &password_profile("core", "pw")).unwrap();
    drop(vault);
    let good = fs::read(data.vault()).unwrap();
    let damaged = |bytes: &[u8]| {
        fs::write(data.vault(), bytes).unwrap();
        let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
        assert!(matches!(error, VaultError::Damaged(_)), "{error:?}");
        // Not a wrong password whatever is typed.
        let error = Vault::unlock(&data.vault(), "wrong").unwrap_err();
        assert!(matches!(error, VaultError::Damaged(_)), "{error:?}");
        error
    };

    damaged(&good[..good.len() / 2]);
    damaged(&[]);
    damaged(b"not a vault at all");

    // One hex digit of the sealed body changed.
    let text = String::from_utf8(good.clone()).unwrap();
    let at = text.find("\"body\": \"").unwrap() + "\"body\": \"".len() + 10;
    let mut altered = text.clone().into_bytes();
    altered[at] = if altered[at] == b'0' { b'1' } else { b'0' };
    damaged(&altered);

    // The salt changed: the checksum tells, before any key is derived.
    let at = text.find("\"salt\": \"").unwrap() + "\"salt\": \"".len();
    let mut altered = text.into_bytes();
    altered[at] = if altered[at] == b'0' { b'1' } else { b'0' };
    let error = damaged(&altered);
    assert!(error.to_string().contains("damaged"), "{error}");

    fs::write(data.vault(), &good).unwrap();
    assert!(unlocked(&data.vault())
        .get(SCOPE_A, "core")
        .unwrap()
        .is_some());
}

#[test]
fn a_missing_vault_cannot_be_unlocked() {
    let data = DataFolder::new();
    assert_eq!(
        Vault::unlock(&data.vault(), MASTER).unwrap_err(),
        VaultError::NoVault
    );
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(store.unlock(MASTER).unwrap_err(), VaultError::NoVault);
}

#[test]
fn neither_the_master_password_nor_any_secret_is_in_the_file() {
    let data = DataFolder::new();
    let vault = Vault::create(&data.vault(), MASTER).unwrap();
    vault
        .put(SCOPE_A, &password_profile("core", "pl4in-passw0rd"))
        .unwrap();
    vault.put(SCOPE_A, &key_profile("automation")).unwrap();
    vault
        .put_secret(SCOPE_A, "netbox", &secret("t0ken-v4lue"))
        .unwrap();
    let bytes = fs::read(data.vault()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for plain in [
        MASTER,
        "pl4in-passw0rd",
        "en4ble-s3cret",
        "k3y-passphr4se",
        "t0ken-v4lue",
        "automation",
        "netbox",
        SCOPE_A,
    ] {
        assert!(!text.contains(plain), "{plain} is in the vault file");
    }
}

#[test]
fn the_key_derivation_cost_comes_from_the_file() {
    let data = DataFolder::new();
    let cheap = KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };
    let vault = Vault::create_with(&data.vault(), MASTER, cheap, StoreContents::default()).unwrap();
    vault.put_secret(SCOPE_A, "token", &secret("v")).unwrap();
    drop(vault);
    let reopened = unlocked(&data.vault());
    assert_eq!(reopened.kdf(), cheap);
    assert_eq!(value(&reopened, SCOPE_A, "token").as_deref(), Some("v"));
}

#[test]
fn creating_over_an_existing_vault_or_with_an_empty_password_is_refused() {
    let data = DataFolder::new();
    drop(Vault::create(&data.vault(), MASTER).unwrap());
    let before = fs::read(data.vault()).unwrap();
    assert_eq!(
        Vault::create(&data.vault(), "another").unwrap_err(),
        VaultError::AlreadyExists
    );
    assert_eq!(fs::read(data.vault()).unwrap(), before);

    let data = DataFolder::new();
    assert!(matches!(
        Vault::create(&data.vault(), "").unwrap_err(),
        VaultError::Invalid(_)
    ));
    assert!(data.files().is_empty());
}

#[test]
fn a_change_through_one_handle_keeps_a_change_made_through_another() {
    let data = DataFolder::new();
    drop(Vault::create(&data.vault(), MASTER).unwrap());
    let first = unlocked(&data.vault());
    let second = unlocked(&data.vault());
    first.put(SCOPE_A, &password_profile("one", "1")).unwrap();
    second.put_secret(SCOPE_A, "two", &secret("2")).unwrap();

    let reopened = unlocked(&data.vault());
    assert!(reopened.get(SCOPE_A, "one").unwrap().is_some());
    assert_eq!(value(&reopened, SCOPE_A, "two").as_deref(), Some("2"));
}

#[test]
fn debug_output_shows_no_secret() {
    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    store
        .put(SCOPE_A, &password_profile("core", "pl4in"))
        .unwrap();
    store
        .put_secret(SCOPE_A, "netbox", &secret("t0ken"))
        .unwrap();
    let vault = unlocked(&data.vault());
    let shown = format!("{store:?} {vault:?} {:?}", vault.contents().unwrap());
    for plain in [MASTER, "pl4in", "t0ken", "en4ble-s3cret"] {
        assert!(!shown.contains(plain), "{plain} in {shown}");
    }
}

#[test]
fn turning_the_vault_on_moves_every_scope_out_of_the_keychain() {
    let data = DataFolder::new();
    let keychain = MemoryStore::new();
    let everything = filled_keychain(&keychain);
    let store = ActiveStore::new(Box::new(keychain), data.vault());

    store.turn_on(MASTER, MASTER).unwrap();

    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(store.contents().unwrap().sorted(), everything);
    assert_eq!(
        unlocked(&data.vault()).contents().unwrap().sorted(),
        everything
    );
    // The keychain is empty: a store over it alone finds nothing.
    let after = ActiveStore::new(Box::new(MemoryStore::new()), data.temp.path().join("none"));
    assert!(after.contents().unwrap().is_empty());
    drop(store);

    // At the next launch the vault is found, locked, and holds everything.
    let next_launch = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(next_launch.status(), VaultStatus::Locked);
    next_launch.unlock(MASTER).unwrap();
    assert_eq!(next_launch.contents().unwrap().sorted(), everything);
}

#[test]
fn the_keychain_is_empty_after_turning_on() {
    let data = DataFolder::new();
    let keychain = FailingStore::new(Failures::default());
    filled_keychain(&keychain);
    let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    assert!(keychain.held().is_empty(), "{:?}", keychain.held());
}

#[test]
fn turning_on_needs_the_same_password_twice_and_not_an_empty_one() {
    let data = DataFolder::new();
    let keychain = MemoryStore::new();
    let everything = filled_keychain(&keychain);
    let store = ActiveStore::new(Box::new(keychain), data.vault());

    assert!(matches!(
        store
            .turn_on(MASTER, "correct horse battery stapel")
            .unwrap_err(),
        VaultError::Invalid(_)
    ));
    assert!(matches!(
        store.turn_on("", "").unwrap_err(),
        VaultError::Invalid(_)
    ));
    assert_eq!(store.status(), VaultStatus::Off);
    assert!(data.files().is_empty());
    assert_eq!(store.contents().unwrap().sorted(), everything);

    store.turn_on(MASTER, MASTER).unwrap();
    assert_eq!(
        store.turn_on(MASTER, MASTER).unwrap_err(),
        VaultError::AlreadyExists
    );
}

/// After a failed move, the keychain has everything and there is no vault.
fn nothing_moved(
    data: &DataFolder,
    store: &ActiveStore,
    keychain: &FailingStore,
    everything: &StoreContents,
) {
    assert_eq!(store.status(), VaultStatus::Off);
    assert!(data.files().is_empty(), "{:?}", data.files());
    assert_eq!(&keychain.held(), everything);
    assert_eq!(&store.contents().unwrap().sorted(), everything);
}

#[test]
fn a_keychain_failing_halfway_through_the_move_keeps_everything_and_leaves_no_vault() {
    for after in [0, 1, 3, 4] {
        let data = DataFolder::new();
        let keychain = FailingStore::new(Failures {
            deletes_after: Some(after),
            ..Failures::default()
        });
        let everything = filled_keychain(&keychain);
        let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());

        let error = store.turn_on(MASTER, MASTER).unwrap_err();
        assert!(error.to_string().contains("keychain"), "{error}");
        nothing_moved(&data, &store, &keychain, &everything);
    }
}

#[test]
fn a_keychain_that_can_neither_remove_nor_take_back_keeps_the_vault_in_use() {
    let data = DataFolder::new();
    // Filling takes the five puts allowed; the third removal fails, and
    // the two removed cannot be put back.
    let keychain = FailingStore::new(Failures {
        deletes_after: Some(2),
        puts_after: Some(5),
        ..Failures::default()
    });
    let everything = filled_keychain(&keychain);
    let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());

    let error = store.turn_on(MASTER, MASTER).unwrap_err();
    assert!(error.to_string().contains("kept"), "{error}");
    // Nothing is lost: the vault has everything and is the store in use.
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(store.contents().unwrap().sorted(), everything);
    assert_eq!(
        unlocked(&data.vault()).contents().unwrap().sorted(),
        everything
    );
    assert_eq!(keychain.held().profiles.len(), 1);
}

#[test]
fn a_keychain_that_cannot_be_listed_moves_nothing() {
    let data = DataFolder::new();
    let keychain = FailingStore::new(Failures {
        contents: true,
        ..Failures::default()
    });
    let everything = filled_keychain_held(&keychain);
    let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    assert!(store.turn_on(MASTER, MASTER).is_err());
    assert_eq!(store.status(), VaultStatus::Off);
    assert!(data.files().is_empty());
    assert_eq!(keychain.held(), everything);
}

#[test]
fn a_vault_that_cannot_be_written_moves_nothing() {
    let data = DataFolder::new();
    // The vault's folder is a file, so the vault cannot be written.
    let blocked = data.temp.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    let keychain = FailingStore::new(Failures::default());
    let everything = filled_keychain(&keychain);
    let store = ActiveStore::new(Box::new(keychain.clone()), vault_file(&blocked));

    let error = store.turn_on(MASTER, MASTER).unwrap_err();
    assert!(matches!(error, VaultError::Io(_)), "{error:?}");
    assert_eq!(store.status(), VaultStatus::Off);
    assert_eq!(data.files(), ["blocked"]);
    assert_eq!(keychain.held(), everything);
}

fn filled_keychain_held(keychain: &FailingStore) -> StoreContents {
    keychain
        .put(SCOPE_A, &password_profile("core", "pw-a"))
        .unwrap();
    keychain.put_secret(SCOPE_B, "token", &secret("t")).unwrap();
    keychain.held()
}

fn workspace_with_host(port: u16) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("acme");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("r1.yaml"),
        format!("address: 127.0.0.1\nport: {port}\ncredential_profile: lab-admin\n"),
    )
    .unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    workspace.ensure_id().unwrap();
    (temp, workspace)
}

fn draft(name: &str, password: Option<&str>) -> ProfileDraft {
    ProfileDraft {
        name: name.into(),
        username: USERNAME.into(),
        auth: AuthDraft::Password {
            password: password.map(Into::into),
        },
        enable_secret: None,
    }
}

#[tokio::test]
async fn profiles_in_an_unlocked_vault_list_edit_rename_delete_and_connect() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = workspace_with_host(server.port());
    let data = DataFolder::new();
    let keychain = MemoryStore::new();
    Profiles::new(&keychain, &workspace)
        .save(None, &draft("lab-admin", Some(PASSWORD)))
        .unwrap();
    Secrets::new(&keychain, &workspace)
        .add("netbox", "t0ken")
        .unwrap();
    let store = ActiveStore::new(Box::new(keychain), data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    drop(store);

    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    let profiles = Profiles::new(&store, &workspace);
    assert!(matches!(profiles.list(), Err(ProfileError::Store(_))));
    store.unlock(MASTER).unwrap();

    assert_eq!(profiles.list().unwrap().len(), 1);
    assert!(profiles.missing().unwrap().is_empty());
    let target = profiles.profile_target(&HostId::new("r1.yaml")).unwrap();
    let manager = support::manager();
    let (_id, mut events) = manager.open(
        SessionTarget::Ssh(target),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;

    // Edited (the password kept), renamed, then deleted: the host then
    // names a profile that is not set up.
    profiles
        .save(Some("lab-admin"), &draft("Lab-Admin-2", None))
        .unwrap();
    let listed: Vec<_> = profiles
        .list()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(listed, ["Lab-Admin-2"]);
    assert_eq!(
        store
            .get(&workspace.credential_scope(), "lab-admin-2")
            .unwrap()
            .unwrap()
            .auth,
        ProfileAuth::Password {
            password: PASSWORD.into()
        }
    );
    assert_eq!(profiles.missing().unwrap(), ["lab-admin"]);
    profiles.delete("lab-admin-2").unwrap();
    assert!(profiles.list().unwrap().is_empty());

    let secrets = Secrets::new(&store, &workspace);
    assert_eq!(secrets.list().unwrap(), ["netbox"]);
    // What was done is in the file, not only in memory.
    let reopened = unlocked(&data.vault());
    assert!(reopened
        .list(&workspace.credential_scope())
        .unwrap()
        .is_empty());
    assert_eq!(
        value(&reopened, &workspace.credential_scope(), "netbox").as_deref(),
        Some("t0ken")
    );
}

#[test]
fn profiles_under_the_folder_path_move_to_the_workspace_id_inside_a_vault() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("acme");
    fs::create_dir_all(&root).unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    let by_path = workspace.path_credential_scope();
    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    store
        .put(&by_path, &password_profile("core", "pw"))
        .unwrap();
    store.put_secret(&by_path, "token", &secret("t")).unwrap();

    prepare_workspace(&store, &mut workspace).unwrap();

    let scope = workspace.credential_scope();
    assert_ne!(scope, by_path);
    let reopened = unlocked(&data.vault());
    assert!(reopened.get(&scope, "core").unwrap().is_some());
    assert_eq!(value(&reopened, &scope, "token").as_deref(), Some("t"));
    assert!(reopened.list(&by_path).unwrap().is_empty());
    assert!(reopened.secret_names(&by_path).unwrap().is_empty());
}

const NEW_MASTER: &str = "a whole new master passw0rd";

/// A store with the vault on, holding what `filled_keychain` puts (five
/// puts), over a keychain the test keeps a handle to.
fn vault_on(data: &DataFolder, failures: Failures) -> (ActiveStore, FailingStore, StoreContents) {
    let keychain = FailingStore::new(failures);
    let everything = filled_keychain(&keychain);
    let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    assert!(keychain.held().is_empty());
    (store, keychain, everything)
}

#[test]
fn changing_the_password_needs_the_current_one_and_then_only_the_new_one_unlocks() {
    let data = DataFolder::new();
    let (store, _keychain, everything) = vault_on(&data, Failures::default());

    assert_eq!(
        store
            .change_password("not the master", NEW_MASTER, NEW_MASTER)
            .unwrap_err(),
        VaultError::WrongPassword
    );
    assert!(matches!(
        store
            .change_password(MASTER, NEW_MASTER, "a whole new master passw0rD")
            .unwrap_err(),
        VaultError::Invalid(_)
    ));
    assert!(matches!(
        store.change_password(MASTER, "", "").unwrap_err(),
        VaultError::Invalid(_)
    ));
    // Nothing changed so far.
    assert!(Vault::unlock(&data.vault(), MASTER).is_ok());

    store
        .change_password(MASTER, NEW_MASTER, NEW_MASTER)
        .unwrap();

    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(store.contents().unwrap().sorted(), everything);
    // The store goes on working, and writes under the new password.
    store.put_secret(SCOPE_A, "after", &secret("v")).unwrap();
    assert_eq!(
        Vault::unlock(&data.vault(), MASTER).unwrap_err(),
        VaultError::WrongPassword
    );
    let reopened = Vault::unlock(&data.vault(), NEW_MASTER).unwrap();
    assert_eq!(value(&reopened, SCOPE_A, "after").as_deref(), Some("v"));
    assert_eq!(
        data.files(),
        ["credentials.vault"],
        "no temporary file left"
    );
    drop(store);

    let next_launch = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(
        next_launch.unlock(MASTER).unwrap_err(),
        VaultError::WrongPassword
    );
    next_launch.unlock(NEW_MASTER).unwrap();
    assert_eq!(
        next_launch.get(SCOPE_A, "core").unwrap(),
        Some(password_profile("core", "pw-a"))
    );
}

#[test]
fn changing_the_password_keeps_the_key_derivation_cost() {
    let data = DataFolder::new();
    let cheap = KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };
    drop(Vault::create_with(&data.vault(), MASTER, cheap, StoreContents::default()).unwrap());
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    store.unlock(MASTER).unwrap();
    store
        .change_password(MASTER, NEW_MASTER, NEW_MASTER)
        .unwrap();
    assert_eq!(unlocked_with(&data.vault(), NEW_MASTER).kdf(), cheap);
}

fn unlocked_with(path: &Path, password: &str) -> Vault {
    Vault::unlock(path, password).unwrap()
}

#[test]
fn the_password_of_a_locked_or_missing_vault_cannot_be_changed() {
    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(
        store
            .change_password(MASTER, NEW_MASTER, NEW_MASTER)
            .unwrap_err(),
        VaultError::NoVault
    );
    drop(Vault::create(&data.vault(), MASTER).unwrap());
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(
        store
            .change_password(MASTER, NEW_MASTER, NEW_MASTER)
            .unwrap_err(),
        VaultError::Locked
    );
    assert!(Vault::unlock(&data.vault(), MASTER).is_ok());
}

#[test]
fn turning_the_vault_off_moves_everything_back_and_removes_the_file() {
    let data = DataFolder::new();
    let (store, keychain, _) = vault_on(&data, Failures::default());
    // Kept while the vault was on: moves back too.
    store.put(SCOPE_B, &key_profile("Later")).unwrap();
    store
        .put_secret(SCOPE_A, "later-token", &secret("l4ter"))
        .unwrap();
    let everything = store.contents().unwrap().sorted();
    assert_eq!(everything.profiles.len(), 4);
    assert_eq!(everything.secrets.len(), 3);

    assert_eq!(
        store.turn_off("not the master").unwrap_err(),
        VaultError::WrongPassword
    );
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert!(keychain.held().is_empty());

    store.turn_off(MASTER).unwrap();

    assert_eq!(store.status(), VaultStatus::Off);
    assert!(data.files().is_empty(), "{:?}", data.files());
    assert_eq!(keychain.held(), everything);
    assert_eq!(store.contents().unwrap().sorted(), everything);
    // Changes now go to the keychain.
    store.put_secret(SCOPE_B, "new", &secret("n")).unwrap();
    assert_eq!(value(&keychain, SCOPE_B, "new").as_deref(), Some("n"));
    // At the next launch there is no vault to unlock.
    let next_launch = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    assert_eq!(next_launch.status(), VaultStatus::Off);
}

#[test]
fn a_keychain_failing_while_turning_off_keeps_the_vault_and_the_keychain_as_they_were() {
    // Puts 0-4 fill the keychain and 5 adds the keychain's own profile;
    // the move back starts at 6 (three profiles, then two secrets).
    for failing in [6, 7, 8, 10] {
        let data = DataFolder::new();
        let (store, keychain, everything) = vault_on(
            &data,
            Failures {
                failing_put: Some(failing),
                ..Failures::default()
            },
        );
        // A profile the keychain has of its own, under a name the vault
        // also has: it is put back as it was.
        keychain
            .put(SCOPE_A, &password_profile("Core", "the keychain's own"))
            .unwrap();
        let before = keychain.held();

        let error = store.turn_off(MASTER).unwrap_err();
        assert!(error.to_string().contains("keychain"), "{error}");

        assert_eq!(store.status(), VaultStatus::Unlocked, "put {failing}");
        assert_eq!(store.contents().unwrap().sorted(), everything);
        assert_eq!(
            unlocked(&data.vault()).contents().unwrap().sorted(),
            everything
        );
        assert_eq!(keychain.held(), before, "put {failing}");
    }
}

#[test]
fn a_keychain_that_cannot_take_back_its_own_entries_still_keeps_the_vault() {
    let data = DataFolder::new();
    // Every put after the sixth fails: the move back stops at its second,
    // and the keychain's own profile cannot be put back.
    let (store, keychain, everything) = vault_on(
        &data,
        Failures {
            puts_after: Some(7),
            ..Failures::default()
        },
    );
    keychain.put(SCOPE_A, &key_profile("automation")).unwrap();

    let error = store.turn_off(MASTER).unwrap_err();
    assert!(
        error.to_string().contains("could not be put back"),
        "{error}"
    );
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(
        unlocked(&data.vault()).contents().unwrap().sorted(),
        everything
    );
}

/// Windows refuses to delete a file another handle holds without sharing
/// deletion: the vault file then cannot be removed.
#[cfg(windows)]
#[test]
fn a_vault_file_that_cannot_be_removed_keeps_the_vault_and_empties_the_keychain_again() {
    use std::os::windows::fs::OpenOptionsExt;

    let data = DataFolder::new();
    let (store, keychain, everything) = vault_on(&data, Failures::default());
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ only
        .open(data.vault())
        .unwrap();

    let error = store.turn_off(MASTER).unwrap_err();
    assert!(error.to_string().contains("vault file"), "{error}");
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert!(keychain.held().is_empty(), "{:?}", keychain.held());
    drop(held);
    assert_eq!(
        unlocked(&data.vault()).contents().unwrap().sorted(),
        everything
    );
    // Once the file can go, turning off works.
    store.turn_off(MASTER).unwrap();
    assert_eq!(keychain.held(), everything);
}

#[test]
fn a_locked_or_missing_vault_cannot_be_turned_off() {
    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(store.turn_off(MASTER).unwrap_err(), VaultError::NoVault);
    drop(Vault::create(&data.vault(), MASTER).unwrap());
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(store.turn_off(MASTER).unwrap_err(), VaultError::Locked);
    assert_eq!(data.files(), ["credentials.vault"]);
}

#[test]
fn starting_over_deletes_a_locked_vault_and_returns_to_an_empty_keychain() {
    let data = DataFolder::new();
    let (store, _keychain, _) = vault_on(&data, Failures::default());
    drop(store);

    // The next launch, with the password forgotten.
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(store.status(), VaultStatus::Locked);
    store.start_over().unwrap();

    assert_eq!(store.status(), VaultStatus::Off);
    assert!(data.files().is_empty(), "{:?}", data.files());
    assert!(store.contents().unwrap().is_empty());
    store.put(SCOPE_A, &password_profile("fresh", "f")).unwrap();
    assert!(store.get(SCOPE_A, "fresh").unwrap().is_some());
    let next_launch = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(next_launch.status(), VaultStatus::Off);
}

#[test]
fn a_damaged_vault_can_be_started_over() {
    let data = DataFolder::new();
    fs::write(data.vault(), "cut sho").unwrap();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert!(matches!(
        store.unlock(MASTER).unwrap_err(),
        VaultError::Damaged(_)
    ));
    store.start_over().unwrap();
    assert_eq!(store.status(), VaultStatus::Off);
    assert!(data.files().is_empty());
}

#[test]
fn starting_over_needs_a_locked_vault() {
    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert_eq!(store.start_over().unwrap_err(), VaultError::NoVault);

    let (store, _keychain, everything) = vault_on(&data, Failures::default());
    assert!(matches!(
        store.start_over().unwrap_err(),
        VaultError::Invalid(_)
    ));
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(
        unlocked(&data.vault()).contents().unwrap().sorted(),
        everything
    );
}

// A system with no keychain the app can use (Linux): the vault is the only
// store.

#[test]
fn without_a_keychain_nothing_is_stored_until_the_vault_is_turned_on() {
    let data = DataFolder::new();
    let store = ActiveStore::without_keychain(data.vault());
    assert!(!store.has_keychain());
    assert_eq!(store.status(), VaultStatus::Off);
    assert!(store.needs_vault());

    // Reading finds nothing (no errors: the app lists no profiles).
    assert!(store.list(SCOPE_A).unwrap().is_empty());
    assert!(store.secret_names(SCOPE_A).unwrap().is_empty());
    assert_eq!(store.get(SCOPE_A, "core").unwrap(), None);
    assert!(store.get_secret(SCOPE_A, "token").unwrap().is_none());
    assert!(store.contents().unwrap().is_empty());
    assert!(!store.delete(SCOPE_A, "core").unwrap());

    // Storing says the vault must be turned on.
    let refused = store
        .put(SCOPE_A, &password_profile("core", "pw"))
        .unwrap_err();
    assert!(refused.0.contains("turn on the vault"), "{refused}");
    let refused = store
        .put_secret(SCOPE_A, "token", &secret("t"))
        .unwrap_err();
    assert!(refused.0.contains("turn on the vault"), "{refused}");
    let (_temp, workspace) = workspace_with_host(22);
    match Profiles::new(&store, &workspace).save(None, &draft("lab-admin", Some(PASSWORD))) {
        Err(ProfileError::Store(error)) => assert!(error.0.contains("turn on the vault")),
        other => panic!("{other:?}"),
    }
    assert!(data.files().is_empty(), "{:?}", data.files());

    // Turning it on moves nothing and then everything is kept there.
    store.turn_on(MASTER, MASTER).unwrap();
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert!(!store.needs_vault());
    behaves_as_a_store(&store);
    Profiles::new(&store, &workspace)
        .save(None, &draft("lab-admin", Some(PASSWORD)))
        .unwrap();
    let next_launch = ActiveStore::without_keychain(data.vault());
    assert_eq!(next_launch.status(), VaultStatus::Locked);
    assert!(!next_launch.needs_vault());
}

#[test]
fn without_a_keychain_the_vault_cannot_be_turned_off() {
    let data = DataFolder::new();
    let store = ActiveStore::without_keychain(data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    store.put(SCOPE_A, &password_profile("core", "pw")).unwrap();

    let refused = store.turn_off(MASTER).unwrap_err();
    assert_eq!(refused, VaultError::NoKeychain);
    assert!(refused.to_string().contains("no other store"), "{refused}");
    // Even before the password is checked: there is nothing to try.
    assert_eq!(store.turn_off("wrong").unwrap_err(), VaultError::NoKeychain);
    assert_eq!(store.status(), VaultStatus::Unlocked);
    assert_eq!(data.files(), [VAULT_FILE]);
    assert!(store.get(SCOPE_A, "core").unwrap().is_some());
}

#[test]
fn without_a_keychain_starting_over_needs_the_vault_again() {
    let data = DataFolder::new();
    let store = ActiveStore::without_keychain(data.vault());
    store.turn_on(MASTER, MASTER).unwrap();
    drop(store);

    let next_launch = ActiveStore::without_keychain(data.vault());
    next_launch.start_over().unwrap();
    assert_eq!(next_launch.status(), VaultStatus::Off);
    assert!(next_launch.needs_vault());
    assert!(data.files().is_empty());
}

#[test]
fn with_a_keychain_the_vault_is_never_needed() {
    let data = DataFolder::new();
    let store = ActiveStore::new(Box::new(MemoryStore::new()), data.vault());
    assert!(store.has_keychain());
    assert!(!store.needs_vault());
}
