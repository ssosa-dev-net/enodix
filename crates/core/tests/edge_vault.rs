//! Edge tests of the vault (#117): deliberately hostile use of the file that
//! holds the user's logins. A file cut short at every length, bytes flipped
//! anywhere, the right header with another vault's body, a file from a newer
//! version; what a write stopped half way leaves (simulated: the state on
//! disk a power cut leaves, as no test can cut the power); two copies of the
//! app (two stores on the same files, each with its own threads, as two
//! processes would be) writing at once, turning the vault on or off and
//! changing the master password while the other holds it; the command line
//! reading the file while the app writes it. Since #125, the lock between
//! copies: two turning the vault on at once, a save or a start while
//! another turns it on, and real second processes (this binary run again
//! as a writer), one of them killed while it writes.
//!
//! What must hold: a damaged vault is named as damaged (never a wrong
//! password, never an empty vault), is never written over, and a readable
//! vault is never lost; no change is lost without the one who made it being
//! told. These speak for Windows and Linux alike.

mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use enodix_core::credentials::{
    vault_file, ActiveStore, CredentialProfile, CredentialStore, KdfParams, ProfileAuth,
    ScopedProfile, ScopedSecret, SecretValue, StoreContents, Vault, VaultError, VaultStatus,
    VAULT_LOCK_FILE,
};
use sha2::{Digest, Sha256};
use support::credentials::{FailingStore, Failures, Gate};
use support::processes;
use tempfile::TempDir;

const MASTER: &str = "correct horse battery staple";
const SCOPE: &str = "edge-workspace";

/// A cheap key derivation, so that the tests that unlock many times stay
/// fast; what is checked does not depend on the cost.
fn cheap() -> KdfParams {
    KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    }
}

fn profile(name: &str, password: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: "noc".into(),
        auth: ProfileAuth::Password {
            password: password.into(),
        },
        enable_secret: Some("en4ble".into()),
    }
}

fn contents() -> StoreContents {
    StoreContents {
        profiles: vec![
            ScopedProfile {
                scope: SCOPE.into(),
                profile: profile("core", "pw-core"),
            },
            ScopedProfile {
                scope: SCOPE.into(),
                profile: profile("edge", "pw-edge"),
            },
        ],
        secrets: vec![ScopedSecret {
            scope: SCOPE.into(),
            name: "netbox-token".into(),
            value: SecretValue::new("t0ken"),
        }],
    }
    .sorted()
}

/// A data folder of its own.
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

    /// A vault holding [`contents`], locked again; its bytes.
    fn with_vault(&self) -> Vec<u8> {
        drop(Vault::create_with(&self.vault(), MASTER, cheap(), contents()).unwrap());
        fs::read(self.vault()).unwrap()
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }
}

fn is_damaged(error: &VaultError) -> bool {
    matches!(error, VaultError::Damaged(_))
}

/// Everything the app could do with a vault file it cannot read: each is
/// refused, none writes, and the file is exactly as it was.
fn nothing_writes_over(data: &DataFolder) {
    let before = fs::read(data.vault()).unwrap();
    let keychain = FailingStore::new(Failures::default());
    keychain.put(SCOPE, &profile("keychain", "k")).unwrap();
    let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    assert_eq!(store.status(), VaultStatus::Locked, "a vault file is there");
    let error = store.unlock(MASTER).unwrap_err();
    assert!(is_damaged(&error), "{error:?}");
    assert_eq!(store.status(), VaultStatus::Locked);
    assert!(store.put(SCOPE, &profile("new", "n")).is_err());
    assert!(store.get(SCOPE, "core").is_err(), "not an empty vault");
    assert!(store.list(SCOPE).is_err(), "not an empty vault");
    assert_eq!(
        store.turn_on(MASTER, MASTER).unwrap_err(),
        VaultError::AlreadyExists
    );
    assert_eq!(
        store
            .change_password(MASTER, "new master", "new master")
            .unwrap_err(),
        VaultError::Locked
    );
    assert_eq!(store.turn_off(MASTER).unwrap_err(), VaultError::Locked);
    assert_eq!(
        Vault::create(&data.vault(), MASTER).unwrap_err(),
        VaultError::AlreadyExists
    );
    assert_eq!(
        fs::read(data.vault()).unwrap(),
        before,
        "the file is untouched"
    );
    assert_eq!(
        keychain.held().profiles.len(),
        1,
        "the keychain is untouched"
    );
}

#[test]
fn a_vault_cut_short_at_any_length_is_damaged_and_never_written_over() {
    let data = DataFolder::new();
    let good = data.with_vault();
    let text = String::from_utf8(good.clone()).unwrap();
    let in_header = text.find("\"salt\"").unwrap() + 3;
    let in_body = text.find("\"body\"").unwrap() + 20;

    for length in 0..good.len() {
        fs::write(data.vault(), &good[..length]).unwrap();
        let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
        assert!(is_damaged(&error), "cut at {length}: {error:?}");
        let error = Vault::unlock(&data.vault(), "a wrong one").unwrap_err();
        assert!(is_damaged(&error), "cut at {length}: {error:?}");
        if [0, 1, in_header, in_body, good.len() - 1].contains(&length) {
            nothing_writes_over(&data);
        }
    }

    fs::write(data.vault(), &good).unwrap();
    let vault = Vault::unlock(&data.vault(), MASTER).unwrap();
    assert_eq!(vault.contents().unwrap().sorted(), contents());
}

#[test]
fn bytes_flipped_anywhere_are_damage_never_a_wrong_password_nor_other_contents() {
    let data = DataFolder::new();
    let good = data.with_vault();
    // A fixed sequence (xorshift), so that a failure can be played again.
    let mut state: u64 = 0x117_0117_0117;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (mut damaged, mut harmless) = (0, 0);
    for _ in 0..600 {
        let mut bytes = good.clone();
        let at = (next() % bytes.len() as u64) as usize;
        let flip = (next() % 255 + 1) as u8;
        bytes[at] ^= flip;
        fs::write(data.vault(), &bytes).unwrap();
        match Vault::unlock(&data.vault(), MASTER) {
            Err(VaultError::Damaged(_)) => damaged += 1,
            // Whitespace between the JSON values changes nothing.
            Ok(vault) => {
                assert_eq!(vault.contents().unwrap().sorted(), contents());
                harmless += 1;
            }
            Err(other) => panic!("byte {at} ^ {flip:#04x}: {other:?}"),
        }
    }
    assert!(damaged > 500, "{damaged} damaged, {harmless} harmless");
    nothing_writes_over(&data);
}

/// The file's JSON, with `edit` applied and the checksum made right again
/// (as a tool that knows the format, or chance, could).
fn rewritten(bytes: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut file: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    edit(&mut file);
    let text = |key: &str| file[key].as_str().unwrap().to_string();
    let kdf = &file["kdf"];
    let header = format!(
        "{}/{}/{}/{}/{}/{}/{}",
        text("format"),
        file["version"],
        kdf["algorithm"].as_str().unwrap(),
        kdf["memory_kib"],
        kdf["iterations"],
        kdf["parallelism"],
        text("salt")
    );
    let mut hash = Sha256::new();
    hash.update(header.as_bytes());
    for part in ["check", "nonce", "body"] {
        hash.update(b"/");
        hash.update(text(part).as_bytes());
    }
    let checksum: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    file["checksum"] = checksum.into();
    serde_json::to_vec_pretty(&file).unwrap()
}

#[test]
fn the_right_header_with_another_vaults_body_is_damaged() {
    let data = DataFolder::new();
    let good = data.with_vault();
    // The same password, another vault: another salt, so another key.
    let other = DataFolder::new();
    drop(
        Vault::create_with(
            &other.vault(),
            MASTER,
            cheap(),
            StoreContents {
                profiles: vec![ScopedProfile {
                    scope: SCOPE.into(),
                    profile: profile("intruder", "x"),
                }],
                secrets: Vec::new(),
            },
        )
        .unwrap(),
    );
    let theirs: serde_json::Value =
        serde_json::from_slice(&fs::read(other.vault()).unwrap()).unwrap();

    // Their body as it is: the checksum tells.
    let mut spliced: serde_json::Value = serde_json::from_slice(&good).unwrap();
    spliced["nonce"] = theirs["nonce"].clone();
    spliced["body"] = theirs["body"].clone();
    fs::write(data.vault(), serde_json::to_vec_pretty(&spliced).unwrap()).unwrap();
    let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
    assert!(is_damaged(&error), "{error:?}");

    // Their body under a checksum made right: the body does not open.
    let forged = rewritten(&good, |file| {
        file["nonce"] = theirs["nonce"].clone();
        file["body"] = theirs["body"].clone();
    });
    fs::write(data.vault(), &forged).unwrap();
    let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
    assert!(is_damaged(&error), "{error:?}");
    nothing_writes_over(&data);

    // Our header, our check, a body cut short: damaged too.
    let cut = rewritten(&good, |file| {
        let body = file["body"].as_str().unwrap().to_string();
        file["body"] = body[..body.len() / 2].to_string().into();
    });
    fs::write(data.vault(), &cut).unwrap();
    let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
    assert!(is_damaged(&error), "{error:?}");
}

#[test]
fn a_vault_from_a_newer_version_says_so_and_is_never_written_over() {
    let data = DataFolder::new();
    let good = data.with_vault();
    let newer = rewritten(&good, |file| file["version"] = 2.into());
    fs::write(data.vault(), &newer).unwrap();
    let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
    assert!(is_damaged(&error), "{error:?}");
    assert!(error.to_string().contains("newer version"), "{error}");
    nothing_writes_over(&data);

    // A key derivation this version does not know: named, not guessed at.
    let unknown = rewritten(&good, |file| file["kdf"]["algorithm"] = "scrypt".into());
    fs::write(data.vault(), &unknown).unwrap();
    let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
    assert!(error.to_string().contains("scrypt"), "{error}");
    nothing_writes_over(&data);

    // A header asking for more memory than any machine should give.
    let greedy = rewritten(&good, |file| {
        file["kdf"]["memory_kib"] = (u64::from(u32::MAX)).into();
    });
    fs::write(data.vault(), &greedy).unwrap();
    let error = Vault::unlock(&data.vault(), MASTER).unwrap_err();
    assert!(is_damaged(&error), "{error:?}");
}

#[test]
fn a_vault_file_that_is_a_folder_or_cannot_be_read_is_not_taken_for_no_vault() {
    let data = DataFolder::new();
    fs::create_dir(data.vault()).unwrap();
    let store = ActiveStore::new(
        Box::new(FailingStore::new(Failures::default())),
        data.vault(),
    );
    assert_eq!(store.status(), VaultStatus::Locked);
    let error = store.unlock(MASTER).unwrap_err();
    assert!(
        matches!(error, VaultError::Io(_) | VaultError::Damaged(_)),
        "{error:?}"
    );
    assert_eq!(
        store.turn_on(MASTER, MASTER).unwrap_err(),
        VaultError::AlreadyExists
    );
    assert!(data.vault().is_dir());
}

// What a write stopped half way leaves. The vault is written whole to a
// temporary file beside it, flushed to the disk, then renamed over it: a
// power cut before the rename leaves the old vault and a temporary file, a
// cut after it the new vault. These play the states a cut can leave.

#[test]
fn a_write_cut_before_its_rename_leaves_the_old_vault_whole_and_its_leftover_is_ignored() {
    let data = DataFolder::new();
    let good = data.with_vault();
    // What an older version's write, and this one's, leave when cut short.
    let leftovers = [
        "credentials.vault.tmp".to_string(),
        "credentials.vault.4242-7.tmp".to_string(),
    ];
    for leftover in &leftovers {
        fs::write(data.path(leftover), &good[..good.len() / 3]).unwrap();
        // Stopped long ago, not a write going on now.
        let file = fs::File::options()
            .write(true)
            .open(data.path(leftover))
            .unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
    }

    let store = ActiveStore::new(
        Box::new(FailingStore::new(Failures::default())),
        data.vault(),
    );
    assert_eq!(store.status(), VaultStatus::Locked);
    store.unlock(MASTER).unwrap();
    assert_eq!(store.contents().unwrap().sorted(), contents());
    store.put(SCOPE, &profile("after", "a")).unwrap();
    assert_eq!(
        data.files(),
        ["credentials.vault", VAULT_LOCK_FILE],
        "the leftovers were cleared at the start"
    );
    drop(store);
    let reopened = Vault::unlock(&data.vault(), MASTER).unwrap();
    assert!(reopened.get(SCOPE, "after").unwrap().is_some());
}

#[test]
fn a_temporary_file_being_written_now_is_left_alone_at_the_start() {
    let data = DataFolder::new();
    data.with_vault();
    // Another copy of the app writing at this moment.
    fs::write(data.path("credentials.vault.4242-8.tmp"), b"{").unwrap();
    let store = ActiveStore::new(
        Box::new(FailingStore::new(Failures::default())),
        data.vault(),
    );
    assert_eq!(store.status(), VaultStatus::Locked);
    assert!(data.path("credentials.vault.4242-8.tmp").exists());
}

#[test]
fn turning_on_cut_before_its_rename_leaves_the_keychain_in_use_with_everything() {
    let data = DataFolder::new();
    let keychain = FailingStore::new(Failures::default());
    keychain.put(SCOPE, &profile("core", "pw")).unwrap();
    // The vault was being created when the power went: only its temporary
    // file is there, and nothing was taken out of the keychain yet.
    fs::write(
        data.path("credentials.vault.tmp"),
        b"{\"format\": \"enodix-va",
    )
    .unwrap();
    let store = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    assert_eq!(store.status(), VaultStatus::Off);
    assert!(store.get(SCOPE, "core").unwrap().is_some());
    store.turn_on(MASTER, MASTER).unwrap();
    assert!(store.get(SCOPE, "core").unwrap().is_some());
    assert!(keychain.held().is_empty());
}

#[test]
fn every_write_leaves_a_whole_vault_for_a_reader_at_every_moment() {
    // The command line reads the vault file while the app writes it: at no
    // moment may it see half a file, or no file.
    let data = DataFolder::new();
    data.with_vault();
    let writer = Vault::unlock(&data.vault(), MASTER).unwrap();
    let path = data.vault();
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let done = Arc::clone(&done);
        thread::spawn(move || {
            let (mut reads, mut failures) = (0, Vec::new());
            while !done.load(Ordering::SeqCst) {
                match Vault::unlock(&path, MASTER) {
                    Ok(_) => reads += 1,
                    Err(error) => failures.push(error),
                }
            }
            (reads, failures)
        })
    };
    let mut write_failures = Vec::new();
    for index in 0..150 {
        if let Err(error) = writer.put(SCOPE, &profile(&format!("p{index}"), "x")) {
            write_failures.push(error);
        }
    }
    done.store(true, Ordering::SeqCst);
    let (reads, read_failures) = reader.join().unwrap();
    assert!(reads > 0);
    // Said together: a reader that saw no file while every write was
    // reported done is a gap in the write; with writes failing too, the
    // folder itself went away under the test.
    assert!(
        read_failures.is_empty(),
        "{} of {} reads failed, first {:?}; {} of 150 writes failed, first {:?}; the vault file is {} now",
        read_failures.len(),
        reads + read_failures.len(),
        read_failures.first(),
        write_failures.len(),
        write_failures.first(),
        if data.vault().exists() {
            "there"
        } else {
            "not there"
        }
    );
    assert!(
        write_failures.is_empty(),
        "{} writes failed while a reader read, first {:?}",
        write_failures.len(),
        write_failures.first()
    );
}

// Two copies of the app: two stores over the same vault file (and the same
// keychain), as two processes have. Each store's own changes are serialised;
// between them only the file is shared.

/// Two handles of the vault in [`DataFolder::with_vault`], each changing it
/// from its own thread, at the same moment.
fn two_writers(data: &DataFolder, each: usize) -> Vec<(String, bool)> {
    let barrier = Arc::new(Barrier::new(2));
    let writers: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|copy| {
            let vault = Vault::unlock(&data.vault(), MASTER).unwrap();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                (0..each)
                    .map(|index| {
                        let name = format!("{copy}{index}");
                        let saved = vault.put(SCOPE, &profile(&name, "x")).is_ok();
                        (name, saved)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    writers
        .into_iter()
        .flat_map(|writer| writer.join().unwrap())
        .collect()
}

#[test]
fn two_copies_writing_at_once_never_leave_a_damaged_vault() {
    for _ in 0..3 {
        let data = DataFolder::new();
        data.with_vault();
        let written = two_writers(&data, 60);
        let failed = written.iter().filter(|(_, saved)| !saved).count();
        println!("{failed} of {} changes reported not saved", written.len());
        let reopened = Vault::unlock(&data.vault(), MASTER);
        assert!(reopened.is_ok(), "{:?}", reopened.err());
        assert_eq!(
            data.files(),
            ["credentials.vault", VAULT_LOCK_FILE],
            "no temporary file left"
        );
    }
}

#[test]
fn two_copies_writing_at_once_keep_every_change_they_were_told_was_saved() {
    let data = DataFolder::new();
    data.with_vault();
    let written = two_writers(&data, 60);
    let reopened = Vault::unlock(&data.vault(), MASTER).unwrap();
    let lost: Vec<&String> = written
        .iter()
        .filter(|(name, saved)| *saved && reopened.get(SCOPE, name).unwrap().is_none())
        .map(|(name, _)| name)
        .collect();
    assert!(
        lost.is_empty(),
        "{} saved changes lost: {lost:?}",
        lost.len()
    );
}

/// Two copies of the app over one keychain and one data folder.
fn two_copies(data: &DataFolder) -> (ActiveStore, ActiveStore, FailingStore) {
    let keychain = FailingStore::new(Failures::default());
    let first = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    let second = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    (first, second, keychain)
}

#[test]
fn a_vault_turned_on_by_another_copy_locks_this_one_rather_than_writing_to_the_keychain() {
    let data = DataFolder::new();
    let (first, second, keychain) = two_copies(&data);
    first.put(SCOPE, &profile("core", "pw")).unwrap();
    assert_eq!(second.status(), VaultStatus::Off);

    first.turn_on(MASTER, MASTER).unwrap();
    assert!(keychain.held().is_empty());

    // The second copy still thinks it uses the keychain: what it would
    // store there now nobody would find again.
    let refused = second.put(SCOPE, &profile("late", "l")).unwrap_err();
    assert!(refused.0.contains("locked"), "{refused}");
    assert!(keychain.held().is_empty(), "nothing went to the keychain");
    let refused = second.get(SCOPE, "core").unwrap_err();
    assert!(
        refused.0.contains("locked"),
        "not silently missing: {refused}"
    );
    assert_eq!(second.status(), VaultStatus::Locked);
    second.unlock(MASTER).unwrap();
    assert!(second.get(SCOPE, "core").unwrap().is_some());
}

#[test]
fn a_vault_turned_off_by_another_copy_is_reported_by_this_one_and_nothing_is_lost() {
    let data = DataFolder::new();
    let (first, second, keychain) = two_copies(&data);
    first.turn_on(MASTER, MASTER).unwrap();
    first.put(SCOPE, &profile("core", "pw")).unwrap();
    let second = {
        drop(second);
        let second = ActiveStore::new(Box::new(keychain.clone()), data.vault());
        second.unlock(MASTER).unwrap();
        second
    };
    first.turn_off(MASTER).unwrap();
    assert!(!data.vault().exists());

    let refused = second.put(SCOPE, &profile("late", "l")).unwrap_err();
    assert!(
        refused.0.contains("turned off") && refused.0.contains("not saved"),
        "{refused}"
    );
    assert!(!data.vault().exists(), "no vault written back");
    // Everything the vault held is in the keychain.
    assert!(keychain.get(SCOPE, "core").unwrap().is_some());
}

#[test]
fn a_master_password_changed_by_another_copy_is_reported_by_this_one_not_called_damage() {
    let data = DataFolder::new();
    let (first, second, _keychain) = two_copies(&data);
    first.turn_on(MASTER, MASTER).unwrap();
    first.put(SCOPE, &profile("core", "pw")).unwrap();
    let second = {
        drop(second);
        let second = ActiveStore::new(
            Box::new(FailingStore::new(Failures::default())),
            data.vault(),
        );
        second.unlock(MASTER).unwrap();
        second
    };
    first
        .change_password(MASTER, "new master", "new master")
        .unwrap();

    let refused = second.put(SCOPE, &profile("late", "l")).unwrap_err();
    assert!(!refused.0.contains("damaged"), "{refused}");
    assert!(
        refused.0.contains("master password") && refused.0.contains("not saved"),
        "{refused}"
    );
    let reopened = Vault::unlock(&data.vault(), "new master").unwrap();
    assert!(reopened.get(SCOPE, "core").unwrap().is_some());
    assert!(reopened.get(SCOPE, "late").unwrap().is_none());

    // The second copy can take the new password without a restart.
    second.unlock("new master").unwrap();
    second.put(SCOPE, &profile("late", "l")).unwrap();
    let reopened = Vault::unlock(&data.vault(), "new master").unwrap();
    assert!(reopened.get(SCOPE, "core").unwrap().is_some());
    assert!(reopened.get(SCOPE, "late").unwrap().is_some());
}

#[test]
fn changing_the_password_or_turning_off_takes_what_another_copy_wrote_meanwhile() {
    let data = DataFolder::new();
    let (first, _second, keychain) = two_copies(&data);
    first.turn_on(MASTER, MASTER).unwrap();
    // The other copy writes through its own handle.
    let other = Vault::unlock(&data.vault(), MASTER).unwrap();
    other.put(SCOPE, &profile("theirs", "t")).unwrap();

    first
        .change_password(MASTER, "second master", "second master")
        .unwrap();
    assert!(first.get(SCOPE, "theirs").unwrap().is_some());

    let other = Vault::unlock(&data.vault(), "second master").unwrap();
    other.put(SCOPE, &profile("theirs-too", "t")).unwrap();
    first.turn_off("second master").unwrap();
    assert!(keychain.get(SCOPE, "theirs").unwrap().is_some());
    assert!(keychain.get(SCOPE, "theirs-too").unwrap().is_some());
}

// The lock between copies (#125): every write of the vault, and every save
// to the keychain while the vault is off, holds `credentials.vault.lock`.

#[test]
fn two_copies_turning_the_vault_on_at_once_make_one_vault_and_lose_nothing() {
    let data = DataFolder::new();
    let (first, second, keychain) = two_copies(&data);
    first.put(SCOPE, &profile("core", "pw")).unwrap();
    let passwords = ["first master", "second master"];
    let barrier = Barrier::new(2);
    let results: Vec<Result<(), VaultError>> = thread::scope(|scope| {
        let turning_on: Vec<_> = [&first, &second]
            .into_iter()
            .zip(passwords)
            .map(|(store, password)| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    store.turn_on(password, password)
                })
            })
            .collect();
        turning_on
            .into_iter()
            .map(|copy| copy.join().unwrap())
            .collect()
    });

    let winners: Vec<usize> = (0..2).filter(|&copy| results[copy].is_ok()).collect();
    assert_eq!(winners.len(), 1, "one vault, one turn on: {results:?}");
    let winner = winners[0];
    assert_eq!(results[1 - winner], Err(VaultError::AlreadyExists));
    // The vault is the winner's, whole, with what the keychain had.
    let vault = Vault::unlock(&data.vault(), passwords[winner]).unwrap();
    assert!(vault.get(SCOPE, "core").unwrap().is_some());
    assert!(keychain.held().is_empty());
    let loser = [&first, &second][1 - winner];
    assert_eq!(loser.status(), VaultStatus::Locked);
    loser.unlock(passwords[winner]).unwrap();
}

#[test]
fn a_copy_saving_to_the_keychain_while_another_turns_the_vault_on_is_told_it_was_not_saved() {
    let data = DataFolder::new();
    let gate = Gate::default();
    let keychain = FailingStore::gated(Failures::default(), gate.clone());
    keychain.put(SCOPE, &profile("core", "pw")).unwrap();
    let first = ActiveStore::new(Box::new(keychain.clone()), data.vault());
    let second = ActiveStore::new(Box::new(keychain.clone()), data.vault());

    let saved = thread::scope(|scope| {
        let turning_on = scope.spawn(|| first.turn_on(MASTER, MASTER));
        // The first copy has listed the keychain and is moving it.
        assert!(gate.wait_reached(Duration::from_secs(30)));
        let (sent, received) = mpsc::channel();
        let second = &second;
        scope.spawn(move || sent.send(second.put(SCOPE, &profile("late", "l"))));
        // Saved to the keychain now, it would be left out of the move.
        let early = received.recv_timeout(Duration::from_millis(300));
        gate.open();
        turning_on.join().unwrap().unwrap();
        early.unwrap_or_else(|_| received.recv().unwrap())
    });

    let in_vault = Vault::unlock(&data.vault(), MASTER)
        .unwrap()
        .get(SCOPE, "late")
        .unwrap()
        .is_some();
    match saved {
        Ok(()) => assert!(in_vault, "told it was saved, and it is not in the vault"),
        Err(refused) => {
            assert!(refused.0.contains("locked"), "{refused}");
            assert!(!in_vault);
        }
    }
    assert!(keychain.held().is_empty(), "{:?}", keychain.held());
}

#[test]
fn a_copy_starting_while_another_turns_the_vault_on_finds_it_locked() {
    let data = DataFolder::new();
    let gate = Gate::default();
    let keychain = FailingStore::gated(Failures::default(), gate.clone());
    keychain.put(SCOPE, &profile("core", "pw")).unwrap();
    let first = ActiveStore::new(Box::new(keychain.clone()), data.vault());

    let status = thread::scope(|scope| {
        let turning_on = scope.spawn(|| first.turn_on(MASTER, MASTER));
        assert!(gate.wait_reached(Duration::from_secs(30)));
        // No vault file yet, but one is being written: a copy starting now
        // must not conclude there is none (and use the keychain).
        let (sent, received) = mpsc::channel();
        let vault = data.vault();
        let keychain = keychain.clone();
        scope.spawn(move || {
            let starting = ActiveStore::new(Box::new(keychain), vault);
            sent.send(starting.status())
        });
        let early = received.recv_timeout(Duration::from_millis(300));
        gate.open();
        turning_on.join().unwrap().unwrap();
        early.unwrap_or_else(|_| received.recv().unwrap())
    });
    assert_eq!(status, VaultStatus::Locked);
}

/// Windows: a program that opens the vault file without letting it be
/// replaced (an antivirus scanner can) makes a rename over it fail for as
/// long as it holds it. A save waits for it a moment rather than failing.
#[cfg(windows)]
#[test]
fn a_scanner_holding_the_vault_file_for_a_moment_only_delays_a_save() {
    use std::os::windows::fs::OpenOptionsExt;

    let data = DataFolder::new();
    data.with_vault();
    let writer = Vault::unlock(&data.vault(), MASTER).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ only
        .open(data.vault())
        .unwrap();
    let scanner = thread::spawn(move || {
        // The scanner's moment.
        thread::sleep(Duration::from_millis(300));
        drop(held);
    });
    writer.put(SCOPE, &profile("during-scan", "s")).unwrap();
    scanner.join().unwrap();
    let reopened = Vault::unlock(&data.vault(), MASTER).unwrap();
    assert!(reopened.get(SCOPE, "during-scan").unwrap().is_some());
}

// Real second processes: this test binary run again as a helper that
// writes the vault ([`WRITER`]).

/// A helper that puts profiles named `<copy><n>` into the vault in the
/// folder [`processes::PIDS_VARIABLE`] names, once the file `go` is
/// there, printing `saved <name>` after each save it was told was done:
/// [`WRITES_VARIABLE`] of them, or until it is killed (0).
const WRITER: &str = "vault writer";
const WRITES_VARIABLE: &str = "ENODIX_TEST_WRITES";
const COPY_VARIABLE: &str = "ENODIX_TEST_COPY";

#[test]
fn helper() {
    processes::play();
    if processes::role().as_deref() == Some(WRITER) {
        write_as_a_helper();
    }
}

fn write_as_a_helper() -> ! {
    let folder = PathBuf::from(std::env::var_os(processes::PIDS_VARIABLE).unwrap());
    let writes: usize = std::env::var(WRITES_VARIABLE).unwrap().parse().unwrap();
    let copy = std::env::var(COPY_VARIABLE).unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    while !folder.join("go").exists() {
        assert!(Instant::now() < until, "never told to go");
        thread::sleep(Duration::from_millis(1));
    }
    let vault = Vault::unlock(&vault_file(&folder), MASTER).unwrap();
    let mut out = std::io::stdout().lock();
    let mut index = 0;
    while (writes == 0 || index < writes) && Instant::now() < until {
        let name = format!("{copy}{index}");
        if vault.put(SCOPE, &profile(&name, "x")).is_ok() {
            writeln!(out, "saved {name}").unwrap();
            out.flush().unwrap();
        }
        index += 1;
    }
    std::process::exit(0)
}

fn writer_process(data: &DataFolder, copy: &str, writes: usize) -> std::process::Child {
    processes::helper(WRITER, data.temp.path())
        .env(WRITES_VARIABLE, writes.to_string())
        .env(COPY_VARIABLE, copy)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn saved_names(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| line.strip_prefix("saved "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn two_processes_writing_at_once_keep_every_change_they_were_told_was_saved() {
    let data = DataFolder::new();
    data.with_vault();
    let writers = [
        writer_process(&data, "a", 40),
        writer_process(&data, "b", 40),
    ];
    fs::write(data.path("go"), "").unwrap();
    let saved: Vec<String> = writers
        .into_iter()
        .flat_map(|writer| saved_names(&writer.wait_with_output().unwrap().stdout))
        .collect();

    assert_eq!(saved.len(), 80, "every save was done: {saved:?}");
    let reopened = Vault::unlock(&data.vault(), MASTER).unwrap();
    let lost: Vec<&String> = saved
        .iter()
        .filter(|name| reopened.get(SCOPE, name).unwrap().is_none())
        .collect();
    assert!(
        lost.is_empty(),
        "{} saved changes lost: {lost:?}",
        lost.len()
    );
}

#[test]
fn a_process_killed_while_writing_blocks_nobody_and_loses_nothing_it_saved() {
    let data = DataFolder::new();
    data.with_vault();
    let mut writer = writer_process(&data, "killed", 0);
    let output = writer.stdout.take().unwrap();
    let (lines, received) = mpsc::channel();
    let reading = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    fs::write(data.path("go"), "").unwrap();
    // Killed in the middle of its writes: almost all of its time is spent
    // holding the lock.
    let mut saved = Vec::new();
    while saved.len() < 20 {
        let line = received
            .recv_timeout(Duration::from_secs(30))
            .expect("the writer saves");
        saved.extend(saved_names(line.as_bytes()));
    }
    writer.kill().unwrap();
    writer.wait().unwrap();
    reading.join().unwrap();
    saved.extend(
        received
            .try_iter()
            .flat_map(|line| saved_names(line.as_bytes())),
    );

    // The lock it may have held went with it, and its lock file, left
    // behind, is no lock: this copy saves at once.
    assert!(data.path(VAULT_LOCK_FILE).exists());
    let (sent, done) = mpsc::channel();
    let path = data.vault();
    thread::spawn(move || {
        let vault = Vault::unlock(&path, MASTER).unwrap();
        sent.send(vault.put(SCOPE, &profile("after", "a"))).unwrap();
    });
    done.recv_timeout(Duration::from_secs(10))
        .expect("a save after the writer died is not blocked")
        .unwrap();

    let reopened = Vault::unlock(&data.vault(), MASTER).unwrap();
    assert!(reopened.get(SCOPE, "after").unwrap().is_some());
    let lost: Vec<&String> = saved
        .iter()
        .filter(|name| reopened.get(SCOPE, name).unwrap().is_none())
        .collect();
    assert!(
        lost.is_empty(),
        "{} saved changes lost: {lost:?}",
        lost.len()
    );
}
