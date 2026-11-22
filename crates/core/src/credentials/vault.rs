//! The vault: the app's own encrypted credential store, one file in the
//! per-user data folder behind a master password.
//!
//! The file is JSON: a plain header (format, version, the key-derivation
//! parameters and salt), a password check, the sealed body and a checksum.
//! The key is derived from the master password with Argon2id (parameters
//! from the header, so they can be raised later); the check and the body
//! are sealed with XChaCha20-Poly1305 under that key, bound to the header.
//! The body holds every scope's profiles and secrets; it is sealed again
//! with a new nonce on every change and written atomically (a temporary
//! file, then a rename). Opening tells a damaged file (it does not parse,
//! or does not match its checksum, or its body does not open) from a wrong
//! password (the check does not open).
//!
//! The master password is never stored. The key and the opened contents
//! live in memory only while the [`Vault`] does and are wiped when it is
//! dropped. `Debug` shows neither.
//!
//! [`ActiveStore`] is the store the app uses: the keychain while there is no
//! vault file, the vault (locked until [`ActiveStore::unlock`]) while there
//! is one. Which one follows from the file existing, never from a setting.
//!
//! Copies of the app (and the command line) share the file. Every write of
//! it (a change, turning on, changing the master password, turning off,
//! starting over), and every save to the keychain while the vault is off,
//! holds an exclusive lock on [`VAULT_LOCK_FILE`] beside it, from reading
//! the file (or the keychain) to the end of the write: two copies saving at
//! once take turns, and each change is made to what the other wrote. The
//! lock is the system's (`File::lock`: `LockFileEx` on Windows, `flock` on
//! Linux), held by an open file: a process that dies lets go of it with its
//! handles, and the lock file it leaves is no lock. The file stays (removing
//! it while another copy waits on it would let a third take a lock of its
//! own). Holders keep it for milliseconds: the slow key derivation is done
//! before it is taken. Readers take no lock while the file is there (a
//! rename replaces it whole, so they see the old file or the new one); only
//! a reader that finds no file, or is refused it, waits for a writer
//! holding the lock and looks again before it concludes anything, since
//! Windows can leave the name missing, or the old file refusing to open,
//! for a moment (tens of milliseconds, measured) while it is replaced.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, SystemTime};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, Generate, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    key, CredentialProfile, CredentialStore, ScopedProfile, ScopedSecret, SecretValue,
    StoreContents, StoreError,
};

/// The vault's file name in the data folder.
pub const VAULT_FILE: &str = "credentials.vault";
/// The name of the vault's lock file, beside it.
pub const VAULT_LOCK_FILE: &str = "credentials.vault.lock";

/// Where the vault lives in the per-user data folder `data_folder`.
pub fn vault_file(data_folder: &Path) -> PathBuf {
    data_folder.join(VAULT_FILE)
}

const FORMAT: &str = "enodix-vault";
/// The format name vaults were written with before the app was renamed:
/// still read, and the file is written with [`FORMAT`] at its next save.
const FORMAT_BEFORE_RENAME: &str = "netterm-vault";
const VERSION: u32 = 1;
const ALGORITHM: &str = "argon2id";
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const MIN_SALT_LEN: usize = 16;
/// The most memory a header may ask the key derivation for (4 GiB): a
/// damaged or hostile header must not exhaust the machine.
const MAX_MEMORY_KIB: u32 = 4 * 1024 * 1024;
/// A temporary file of the vault's older than this was left by a write that
/// stopped (a power cut, a crash): no write takes that long.
const LEFTOVER_AGE: Duration = Duration::from_secs(60);

/// Why a change to an unlocked vault whose file was sealed again under
/// another key was refused.
const REPLACED: &str = "the vault's master password was changed (perhaps by another copy of the app) since it was unlocked here: this change was not saved; unlock it again with the master password it has now";
/// Why a change to an unlocked vault whose file is gone was refused.
const TURNED_OFF: &str = "the vault was turned off (perhaps by another copy of the app) since it was unlocked here, and its profiles and secrets moved to the keychain: this change was not saved; restart the app to use the keychain";
/// Why an app using the keychain refuses once a vault file appeared.
const TURNED_ON: &str = "the vault is locked: it was turned on (perhaps by another copy of the app) since this one started, and the keychain's profiles and secrets moved into it; unlock it to use them";

/// The Argon2id cost of deriving the key from the master password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// RFC 9106's second recommendation: 64 MiB, 3 passes, 4 lanes.
    fn default() -> Self {
        Self {
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    #[error("there is no vault")]
    NoVault,
    #[error("a vault already exists")]
    AlreadyExists,
    #[error("the master password is not right")]
    WrongPassword,
    /// The file is there but cannot be what this app wrote: cut short,
    /// altered, or not a vault at all.
    #[error("the vault file is damaged: {0}")]
    Damaged(String),
    #[error("the vault is locked")]
    Locked,
    #[error("{0}")]
    Invalid(String),
    #[error("cannot use the vault file: {0}")]
    Io(String),
    #[error("{0}")]
    Store(#[from] StoreError),
    /// Turning off on a system without a keychain the app can use.
    #[error("this system has no keychain the app can use, so there is no other store to move the profiles and secrets to: the vault stays on")]
    NoKeychain,
}

/// The file as written.
#[derive(Serialize, Deserialize)]
struct Sealed {
    format: String,
    version: u32,
    kdf: Kdf,
    /// Hex, as are the next three.
    salt: String,
    /// Nonce and tag of an empty message: opens only with the right key.
    check: String,
    nonce: String,
    body: String,
    /// SHA-256 (hex) of everything above.
    checksum: String,
}

#[derive(Serialize, Deserialize)]
struct Kdf {
    algorithm: String,
    #[serde(flatten)]
    params: KdfParams,
}

impl Sealed {
    /// What the check and the body are bound to.
    fn header(&self) -> Vec<u8> {
        let KdfParams {
            memory_kib,
            iterations,
            parallelism,
        } = self.kdf.params;
        format!(
            "{}/{}/{}/{memory_kib}/{iterations}/{parallelism}/{}",
            self.format, self.version, self.kdf.algorithm, self.salt
        )
        .into_bytes()
    }

    fn digest(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.header());
        for part in [&self.check, &self.nonce, &self.body] {
            hash.update(b"/");
            hash.update(part.as_bytes());
        }
        hex(&hash.finalize())
    }
}

/// The opened contents: as in the memory store, by scope and lower-case
/// name. Wiped when dropped.
#[derive(Default)]
struct Body {
    profiles: BTreeMap<(String, String), CredentialProfile>,
    secrets: BTreeMap<(String, String), (String, SecretValue)>,
}

impl Body {
    fn from_contents(contents: StoreContents) -> Self {
        let mut body = Self::default();
        for ScopedProfile { scope, profile } in contents.profiles {
            body.profiles.insert(key(&scope, &profile.name), profile);
        }
        for ScopedSecret { scope, name, value } in contents.secrets {
            body.secrets.insert(key(&scope, &name), (name, value));
        }
        body
    }

    fn contents(&self) -> StoreContents {
        StoreContents {
            profiles: self
                .profiles
                .iter()
                .map(|((scope, _), profile)| ScopedProfile {
                    scope: scope.clone(),
                    profile: profile.clone(),
                })
                .collect(),
            secrets: self
                .secrets
                .iter()
                .map(|((scope, _), (name, value))| ScopedSecret {
                    scope: scope.clone(),
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
        }
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        for profile in self.profiles.values_mut() {
            profile.wipe();
        }
        for (_, value) in self.secrets.values_mut() {
            value.wipe();
        }
    }
}

/// An unlocked vault: a [`CredentialStore`] kept in its file.
///
/// Every change takes the lock between copies, reads the file again (so a
/// change written by another handle on the same file, in this process or
/// another, is kept), applies itself and writes the whole file.
pub struct Vault {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    kdf: KdfParams,
    salt: Vec<u8>,
    body: Mutex<Body>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("kdf", &self.kdf)
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// Whether there is a vault file at `path`. A name missing (or
    /// refused) while another copy writes the vault is looked at again once
    /// it is done, never taken for no vault at once.
    pub fn exists(path: &Path) -> bool {
        use std::io::ErrorKind::{NotFound, PermissionDenied};
        match fs::symlink_metadata(path) {
            Ok(_) => true,
            Err(error) if matches!(error.kind(), NotFound | PermissionDenied) => {
                wait_for_writer(path);
                present(path)
            }
            Err(_) => false,
        }
    }

    /// Checks the file at `path` as far as it can be without the master
    /// password: [`VaultError::Damaged`] for a file that cannot be the
    /// vault (cut short, altered, from a newer version), before anyone is
    /// asked for a password that would not help.
    pub fn check(path: &Path) -> Result<(), VaultError> {
        read_sealed(path, None).map(drop)
    }

    /// Creates an empty vault at `path` protected by `password`, with the
    /// default key-derivation cost. Refused if `path` exists.
    pub fn create(path: &Path, password: &str) -> Result<Self, VaultError> {
        Self::create_with(
            path,
            password,
            KdfParams::default(),
            StoreContents::default(),
        )
    }

    /// Creates a vault at `path` holding `contents`, its key derived with
    /// `kdf`. Refused if `path` exists; nothing is left behind on failure.
    pub fn create_with(
        path: &Path,
        password: &str,
        kdf: KdfParams,
        contents: StoreContents,
    ) -> Result<Self, VaultError> {
        // Held as a body first, so its secrets are wiped on any failure.
        let body = Body::from_contents(contents);
        usable_password(password)?;
        if Self::exists(path) {
            return Err(VaultError::AlreadyExists);
        }
        make_folder(path)?;
        let vault = Self::keyed(path, password, kdf)?;
        *vault.body() = body;
        let lock = WriteLock::take(path)?;
        vault.write_new(&lock)?;
        Ok(vault)
    }

    /// A vault for `path` with a new salt and the key `password` derives
    /// for it (the slow part, done before any lock is taken), holding
    /// nothing yet and written nowhere.
    fn keyed(path: &Path, password: &str, kdf: KdfParams) -> Result<Self, VaultError> {
        usable_password(password)?;
        let salt = Key::generate().to_vec();
        let key = derive_key(password, &kdf, &salt)?;
        Ok(Self {
            path: path.to_path_buf(),
            key,
            kdf,
            salt,
            body: Mutex::new(Body::default()),
        })
    }

    /// Writes this vault's file where there is none: refused if one is
    /// there (another copy turned the vault on first). Under `lock`, so no
    /// other copy can create one between the look and the write.
    fn write_new(&self, lock: &WriteLock) -> Result<(), VaultError> {
        if present(&self.path) {
            return Err(VaultError::AlreadyExists);
        }
        self.write(lock, &self.body())
    }

    /// Opens the vault at `path` with `password`.
    pub fn unlock(path: &Path, password: &str) -> Result<Self, VaultError> {
        let (sealed, salt) = read_sealed(path, None)?;
        let kdf = sealed.kdf.params;
        let key = derive_key(password, &kdf, &salt)?;
        let body = open(&sealed, &key)?;
        Ok(Self {
            path: path.to_path_buf(),
            key,
            kdf,
            salt,
            body: Mutex::new(body),
        })
    }

    /// The key-derivation cost this vault's file was written with.
    pub fn kdf(&self) -> KdfParams {
        self.kdf
    }

    fn body(&self) -> MutexGuard<'_, Body> {
        self.body.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The file's contents as they are now, opened with this vault's key. A
    /// file sealed under another key (its master password changed, perhaps
    /// by another copy of the app) is [`VaultError::Invalid`] with
    /// [`REPLACED`], not damage: it opens with its own password.
    /// Read under `lock`, so that no write of another copy comes between
    /// this and the write that follows it.
    fn reread(&self, lock: &WriteLock) -> Result<Body, VaultError> {
        let (sealed, salt) = read_sealed(&self.path, Some(lock))?;
        if salt != self.salt || sealed.kdf.params != self.kdf {
            return Err(VaultError::Invalid(REPLACED.into()));
        }
        open(&sealed, &self.key).map_err(|error| match error {
            VaultError::WrongPassword => VaultError::Invalid(REPLACED.into()),
            other => other,
        })
    }

    /// Applies `edit` to the file's current contents and writes them,
    /// holding the lock between copies from the read to the write.
    fn change<T>(&self, edit: impl FnOnce(&mut Body) -> T) -> Result<T, StoreError> {
        let mut held = self.body();
        let lock = WriteLock::take(&self.path).map_err(store_error)?;
        let fresh = self.change_under(&lock, edit)?;
        *held = fresh.1;
        Ok(fresh.0)
    }

    /// [`change`](Self::change) under a lock already taken; the new body.
    fn change_under<T>(
        &self,
        lock: &WriteLock,
        edit: impl FnOnce(&mut Body) -> T,
    ) -> Result<(T, Body), StoreError> {
        let mut fresh = self.reread(lock).map_err(|error| match error {
            VaultError::NoVault => StoreError(TURNED_OFF.into()),
            other => store_error(other),
        })?;
        let result = edit(&mut fresh);
        self.write(lock, &fresh).map_err(store_error)?;
        Ok((result, fresh))
    }

    /// Seals `body` and replaces the file with it. Only under the lock.
    fn write(&self, _lock: &WriteLock, body: &Body) -> Result<(), VaultError> {
        let plain = super::encoded_exactly(&Wiping(body.contents()).0)
            .map_err(|error| VaultError::Io(format!("cannot encode the contents: {error}")))?;
        let mut sealed = Sealed {
            format: FORMAT.into(),
            version: VERSION,
            kdf: Kdf {
                algorithm: ALGORITHM.into(),
                params: self.kdf,
            },
            salt: hex(&self.salt),
            check: String::new(),
            nonce: String::new(),
            body: String::new(),
            checksum: String::new(),
        };
        let header = sealed.header();
        let cipher = cipher(&self.key);
        let seal = |nonce: &XNonce, message: &[u8], purpose: &[u8]| {
            let aad = [&header[..], purpose].concat();
            cipher
                .encrypt(
                    nonce,
                    Payload {
                        msg: message,
                        aad: &aad,
                    },
                )
                .map_err(|_| VaultError::Io("cannot seal the contents".into()))
        };
        let check_nonce = XNonce::generate();
        let check = seal(&check_nonce, b"", b"/check")?;
        let nonce = XNonce::generate();
        let ciphertext = seal(&nonce, &plain, b"/body")?;
        sealed.check = hex(&[&check_nonce[..], &check].concat());
        sealed.nonce = hex(&nonce);
        sealed.body = hex(&ciphertext);
        sealed.checksum = sealed.digest();
        let file = serde_json::to_vec_pretty(&sealed)
            .map_err(|error| VaultError::Io(format!("cannot encode the file: {error}")))?;
        write_atomically(&self.path, &file).map_err(|error| VaultError::Io(error.to_string()))
    }
}

/// A copy of store contents taken while moving them: its secrets are wiped
/// when it is dropped, whichever way the move ends.
#[derive(Default)]
struct Wiping(StoreContents);

impl Drop for Wiping {
    fn drop(&mut self) {
        for entry in &mut self.0.profiles {
            entry.profile.wipe();
        }
        for entry in &mut self.0.secrets {
            entry.value.wipe();
        }
    }
}

/// The vault in `state`, if it is unlocked.
fn unlocked_vault(state: &State) -> Result<&Vault, VaultError> {
    match state {
        State::Off => Err(VaultError::NoVault),
        State::Locked => Err(VaultError::Locked),
        State::Unlocked(vault) => Ok(vault),
    }
}

fn usable_password(password: &str) -> Result<(), VaultError> {
    if password.is_empty() {
        return Err(VaultError::Invalid(
            "the master password cannot be empty".into(),
        ));
    }
    Ok(())
}

/// A new master password, typed twice.
fn check_new_password(password: &str, repeated: &str) -> Result<(), VaultError> {
    if password != repeated {
        return Err(VaultError::Invalid(
            "the two master passwords are not the same".into(),
        ));
    }
    usable_password(password)
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key).expect("the key has the cipher's length")
}

fn store_error(error: VaultError) -> StoreError {
    StoreError(error.to_string())
}

fn derive_key(
    password: &str,
    kdf: &KdfParams,
    salt: &[u8],
) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    if kdf.memory_kib > MAX_MEMORY_KIB {
        return Err(VaultError::Damaged(
            "its key parameters ask for too much memory".into(),
        ));
    }
    let params = Params::new(kdf.memory_kib, kdf.iterations, kdf.parallelism, Some(32))
        .map_err(|error| VaultError::Damaged(format!("its key parameters are invalid: {error}")))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key[..])
        .map_err(|error| VaultError::Damaged(format!("cannot derive its key: {error}")))?;
    Ok(key)
}

/// The lock taken around every write of the vault at a path (see the
/// module's documentation): an exclusive lock on [`lock_file`], released
/// when dropped, and by the system if the process dies.
struct WriteLock(fs::File);

impl WriteLock {
    /// Waits for the lock of the vault at `vault` (holders keep it for
    /// milliseconds), making its lock file if there is none.
    fn take(vault: &Path) -> Result<Self, VaultError> {
        let path = lock_file(vault);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                VaultError::Io(format!("cannot open the vault's lock file: {error}"))
            })?;
        file.lock()
            .map_err(|error| VaultError::Io(format!("cannot lock the vault: {error}")))?;
        Ok(Self(file))
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        // Closing the file lets go too; Windows may take a moment for that.
        let _ = self.0.unlock();
    }
}

/// The lock file of the vault at `vault`: [`VAULT_LOCK_FILE`] for the
/// app's.
fn lock_file(vault: &Path) -> PathBuf {
    let mut path = vault.as_os_str().to_owned();
    path.push(".lock");
    PathBuf::from(path)
}

/// Makes the folder the vault at `path` goes in.
fn make_folder(path: &Path) -> Result<(), VaultError> {
    match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => {
            fs::create_dir_all(parent).map_err(|error| VaultError::Io(error.to_string()))
        }
        None => Ok(()),
    }
}

/// Whether the name `path` is there now (under the lock, nobody renames
/// over it).
fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Waits until no copy of the app holds the lock of the vault at `vault`
/// (it is writing it), taking it shared for a moment; returns at once if
/// nobody does, or if there is no lock file (nothing of this version ever
/// wrote there). Never called while holding the lock: it would wait for
/// itself.
fn wait_for_writer(vault: &Path) {
    if let Ok(file) = fs::File::open(lock_file(vault)) {
        if file.lock_shared().is_ok() {
            let _ = file.unlock();
        }
    }
}

/// Reads and checks the file at `path`: everything but what needs the key.
/// Under `held` (the lock), what is there is what is read; else a file
/// found missing (or refused) is read again once a writer is done.
fn read_sealed(path: &Path, held: Option<&WriteLock>) -> Result<(Sealed, Vec<u8>), VaultError> {
    use std::io::ErrorKind::{NotFound, PermissionDenied};
    let bytes = match (fs::read(path), held) {
        // Windows, while another copy renames over the file: the name
        // missing, or the old file going away ("access denied") for a
        // moment.
        (Err(error), None) if matches!(error.kind(), NotFound | PermissionDenied) => {
            wait_for_writer(path);
            fs::read(path)
        }
        (read, _) => read,
    };
    let bytes = bytes.map_err(|error| match error.kind() {
        NotFound => VaultError::NoVault,
        _ => VaultError::Io(error.to_string()),
    })?;
    let sealed: Sealed = serde_json::from_slice(&bytes).map_err(|_| {
        VaultError::Damaged("it cannot be read (cut short, or not a vault file)".into())
    })?;
    if sealed.format != FORMAT && sealed.format != FORMAT_BEFORE_RENAME {
        return Err(VaultError::Damaged("it is not a vault file".into()));
    }
    if sealed.version > VERSION {
        return Err(VaultError::Damaged(format!(
            "it was written by a newer version of the app (format {})",
            sealed.version
        )));
    }
    if sealed.kdf.algorithm != ALGORITHM {
        return Err(VaultError::Damaged(format!(
            "it names an unknown key derivation ({})",
            sealed.kdf.algorithm
        )));
    }
    if sealed.digest() != sealed.checksum {
        return Err(VaultError::Damaged(
            "its contents do not match its checksum".into(),
        ));
    }
    let salt = unhex(&sealed.salt).filter(|salt| salt.len() >= MIN_SALT_LEN);
    let salt = salt.ok_or_else(|| VaultError::Damaged("its salt is invalid".into()))?;
    Ok((sealed, salt))
}

/// Opens the check (a wrong key fails here) and then the body.
fn open(sealed: &Sealed, key: &[u8; 32]) -> Result<Body, VaultError> {
    let damaged = |what: &str| VaultError::Damaged(what.into());
    let check = unhex(&sealed.check)
        .filter(|check| check.len() == NONCE_LEN + TAG_LEN)
        .ok_or_else(|| damaged("its password check is invalid"))?;
    let nonce = unhex(&sealed.nonce)
        .filter(|nonce| nonce.len() == NONCE_LEN)
        .ok_or_else(|| damaged("its nonce is invalid"))?;
    let body = unhex(&sealed.body).ok_or_else(|| damaged("its body is invalid"))?;
    let header = sealed.header();
    let cipher = cipher(key);
    let unseal = |nonce: &[u8], message: &[u8], purpose: &[u8]| {
        let aad = [&header[..], purpose].concat();
        let nonce = XNonce::try_from(nonce).map_err(|_| chacha20poly1305::Error)?;
        cipher.decrypt(
            &nonce,
            Payload {
                msg: message,
                aad: &aad,
            },
        )
    };
    unseal(&check[..NONCE_LEN], &check[NONCE_LEN..], b"/check")
        .map_err(|_| VaultError::WrongPassword)?;
    let plain = Zeroizing::new(
        unseal(&nonce, &body, b"/body").map_err(|_| damaged("its contents do not open"))?,
    );
    let contents: StoreContents =
        serde_json::from_slice(&plain).map_err(|_| damaged("its contents cannot be read"))?;
    Ok(Body::from_contents(contents))
}

/// Writes `bytes` to a temporary file beside `path`, flushes it to the disk
/// and renames it over `path`: a reader sees the old file or the new one,
/// never part of one. Each write has a temporary file of its own
/// (`<file>.<process>-<count>.tmp`), so that two copies of the app writing
/// at once never write into the same one.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = PathBuf::from(temporary);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_over(&temporary, path)?;
        sync_folder(path);
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// How long a rename over the vault waits for a program that holds the
/// file without letting it be replaced (Windows: an antivirus scanner
/// looking at it) before the write fails.
#[cfg(windows)]
const HELD_BY_ANOTHER: Duration = Duration::from_secs(3);

/// Renames `from` over `to`. On Windows a rename over a file another
/// program holds open without sharing its deletion is refused (access
/// denied, or a sharing violation) for as long as it holds it: tried again
/// until it lets go, for [`HELD_BY_ANOTHER`].
fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        const ERROR_SHARING_VIOLATION: i32 = 32;
        let until = std::time::Instant::now() + HELD_BY_ANOTHER;
        loop {
            match fs::rename(from, to) {
                Err(error)
                    if (error.kind() == std::io::ErrorKind::PermissionDenied
                        || error.raw_os_error() == Some(ERROR_SHARING_VIOLATION))
                        && std::time::Instant::now() < until =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                other => return other,
            }
        }
    }
    #[cfg(not(windows))]
    fs::rename(from, to)
}

/// Flushes the rename of `path` to the disk where the system needs it
/// asked for (Linux: the folder's entry), so that a power cut after a write
/// reported done does not bring back the file before it. Best effort: the
/// write itself is already whole.
fn sync_folder(path: &Path) {
    #[cfg(unix)]
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(folder) = fs::File::open(folder) {
            let _ = folder.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Removes the temporary files beside the vault at `path` that writes which
/// stopped half way (a power cut, a crash) left: those not changed for
/// [`LEFTOVER_AGE`], so that another copy of the app writing now keeps its
/// own. Older versions named theirs `<file>.tmp`. They hold nothing the
/// vault does not (a write is only done once renamed) and are never read.
fn remove_leftovers(path: &Path) {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let folder = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        if !(entry_name.starts_with(&prefix) && entry_name.ends_with(".tmp")) {
            continue;
        }
        let stopped = entry
            .metadata()
            .ok()
            .filter(fs::Metadata::is_file)
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= LEFTOVER_AGE);
        if stopped {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

impl CredentialStore for Vault {
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        Ok(self
            .body()
            .profiles
            .iter()
            .filter(|((s, _), _)| s == scope)
            .map(|(_, profile)| profile.clone())
            .collect())
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        Ok(self.body().profiles.get(&key(scope, name)).cloned())
    }

    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
        self.change(|body| {
            if let Some(mut old) = body
                .profiles
                .insert(key(scope, &profile.name), profile.clone())
            {
                old.wipe();
            }
        })
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.change(|body| match body.profiles.remove(&key(scope, name)) {
            Some(mut old) => {
                old.wipe();
                true
            }
            None => false,
        })
    }

    fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .body()
            .secrets
            .iter()
            .filter(|((s, _), _)| s == scope)
            .map(|(_, (name, _))| name.clone())
            .collect())
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        Ok(self
            .body()
            .secrets
            .get(&key(scope, name))
            .map(|(_, value)| value.clone()))
    }

    fn put_secret(&self, scope: &str, name: &str, value: &SecretValue) -> Result<(), StoreError> {
        self.change(|body| {
            if let Some((_, mut old)) = body
                .secrets
                .insert(key(scope, name), (name.to_owned(), value.clone()))
            {
                old.wipe();
            }
        })
    }

    fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.change(|body| match body.secrets.remove(&key(scope, name)) {
            Some((_, mut old)) => {
                old.wipe();
                true
            }
            None => false,
        })
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        Ok(self.body().contents())
    }

    fn vault_status(&self) -> VaultStatus {
        VaultStatus::Unlocked
    }
}

/// Whether the app's credentials are in a vault, and whether it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultStatus {
    /// No vault file: the keychain is used.
    Off,
    /// A vault file, not unlocked yet: nothing can be read or stored.
    Locked,
    Unlocked,
}

/// How emptying the keychain went.
enum Emptied {
    All,
    /// A removal failed; what was removed is back: the keychain is as it
    /// was.
    Restored(StoreError),
    /// A removal failed and some removed entries could not be put back.
    NotRestored(StoreError),
}

enum State {
    Off,
    Locked,
    Unlocked(Vault),
}

/// What a system without a keychain the app can use has instead: nothing
/// stored, and storing refused until the vault is turned on.
struct NoKeychain;

/// Why [`NoKeychain`] stores nothing.
const NEEDS_VAULT: &str =
    "this system has no keychain the app can use: turn on the vault to store profiles and secrets";

impl CredentialStore for NoKeychain {
    fn list(&self, _scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        Ok(Vec::new())
    }

    fn get(&self, _scope: &str, _name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        Ok(None)
    }

    fn put(&self, _scope: &str, _profile: &CredentialProfile) -> Result<(), StoreError> {
        Err(StoreError(NEEDS_VAULT.into()))
    }

    fn delete(&self, _scope: &str, _name: &str) -> Result<bool, StoreError> {
        Ok(false)
    }

    fn secret_names(&self, _scope: &str) -> Result<Vec<String>, StoreError> {
        Ok(Vec::new())
    }

    fn get_secret(&self, _scope: &str, _name: &str) -> Result<Option<SecretValue>, StoreError> {
        Ok(None)
    }

    fn put_secret(
        &self,
        _scope: &str,
        _name: &str,
        _value: &SecretValue,
    ) -> Result<(), StoreError> {
        Err(StoreError(NEEDS_VAULT.into()))
    }

    fn delete_secret(&self, _scope: &str, _name: &str) -> Result<bool, StoreError> {
        Ok(false)
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        Ok(StoreContents::default())
    }
}

/// The credential store the app uses: the keychain it is given while there
/// is no vault file, the vault once one exists. Owns the unlocked vault, so
/// everything that serves credentials goes through it. On a system without
/// a keychain ([`ActiveStore::without_keychain`]) the vault is the only
/// store: nothing is kept until it is turned on, and it cannot be turned
/// off.
pub struct ActiveStore {
    keychain: Box<dyn CredentialStore>,
    has_keychain: bool,
    vault_file: PathBuf,
    state: RwLock<State>,
}

impl fmt::Debug for ActiveStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActiveStore")
            .field("vault_file", &self.vault_file)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl ActiveStore {
    /// The store over `keychain`, with the vault kept at `vault_file`
    /// ([`vault_file`] of the data folder): locked if that file exists.
    /// Temporary files left beside it by a write that stopped half way are
    /// removed.
    pub fn new(keychain: Box<dyn CredentialStore>, vault_file: impl Into<PathBuf>) -> Self {
        let vault_file = vault_file.into();
        remove_leftovers(&vault_file);
        let state = if Vault::exists(&vault_file) {
            State::Locked
        } else {
            State::Off
        };
        Self {
            keychain,
            has_keychain: true,
            vault_file,
            state: RwLock::new(state),
        }
    }

    /// The store of a system with no keychain the app can use (see
    /// [`KeychainStore::is_supported`](super::KeychainStore::is_supported)):
    /// the vault at `vault_file` only.
    pub fn without_keychain(vault_file: impl Into<PathBuf>) -> Self {
        Self {
            has_keychain: false,
            ..Self::new(Box::new(NoKeychain), vault_file)
        }
    }

    /// Whether there is a keychain to keep credentials while the vault is
    /// off (and to move them back to when it is turned off).
    pub fn has_keychain(&self) -> bool {
        self.has_keychain
    }

    /// Whether nothing can be stored until the vault is turned on: no
    /// keychain and no vault.
    pub fn needs_vault(&self) -> bool {
        !self.has_keychain && self.status() == VaultStatus::Off
    }

    /// Off, locked or unlocked. A vault file that appeared while this store
    /// used the keychain (another copy of the app turned the vault on)
    /// makes it locked.
    pub fn status(&self) -> VaultStatus {
        self.notice_vault_turned_on();
        match &*self.state() {
            State::Off => VaultStatus::Off,
            State::Locked => VaultStatus::Locked,
            State::Unlocked(_) => VaultStatus::Unlocked,
        }
    }

    fn state(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// While off: if a vault file is there now, another copy of the app
    /// turned the vault on and moved the keychain's entries into it, so this
    /// store is locked from now on rather than writing to (and reading from)
    /// a keychain nobody else uses. Whether it changed to locked now.
    ///
    /// A plain look, as it is made at every call while off (a planned job
    /// asks for each host's login): a name missing for the moment of
    /// another copy's write is only noticed at the next call, and a change
    /// looks again under the lock ([`with_change`](Self::with_change)).
    fn notice_vault_turned_on(&self) -> bool {
        if !matches!(*self.state(), State::Off) || !present(&self.vault_file) {
            return false;
        }
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if !matches!(*state, State::Off) || !present(&self.vault_file) {
            // This store's own turn on (or the file gone again) came first.
            return false;
        }
        *state = State::Locked;
        true
    }

    /// Unlocks the vault with `password`; it stays unlocked as long as this
    /// store lives. On an unlocked vault the password is still checked, and
    /// the file opened again with it (after another copy of the app changed
    /// the master password, the new one unlocks it here too).
    ///
    /// Then puts right what a turn on cut short left in the keychain (see
    /// [`ActiveStore::turn_on`]): with the vault on, the keychain holds
    /// nothing. An entry the vault has too is the vault's (the copy is the
    /// older); one it lacks is moved into it. Only what the keychain could
    /// not give up stays there (said on the standard error; the unlock
    /// stands), to be tried again at the next unlock.
    pub fn unlock(&self, password: &str) -> Result<(), VaultError> {
        self.notice_vault_turned_on();
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        match *state {
            State::Off => Err(VaultError::NoVault),
            State::Unlocked(_) | State::Locked => {
                let vault = Vault::unlock(&self.vault_file, password)?;
                if let Err(error) = self.take_back_keychain_copies(&vault) {
                    eprintln!("the vault is unlocked, but the keychain still holds copies of profiles or secrets: {error}");
                }
                *state = State::Unlocked(vault);
                Ok(())
            }
        }
    }

    /// Moves into `vault` whatever the keychain holds that it lacks, and
    /// empties the keychain: all under the lock, so that no other copy
    /// turns the vault off (filling the keychain) in between.
    fn take_back_keychain_copies(&self, vault: &Vault) -> Result<(), StoreError> {
        if !self.has_keychain {
            return Ok(());
        }
        let lock = WriteLock::take(&self.vault_file).map_err(store_error)?;
        let copies = Wiping(self.keychain.contents()?);
        if copies.0.is_empty() {
            return Ok(());
        }
        let ((), fresh) = vault.change_under(&lock, |body| {
            for entry in &copies.0.profiles {
                body.profiles
                    .entry(key(&entry.scope, &entry.profile.name))
                    .or_insert_with(|| entry.profile.clone());
            }
            for entry in &copies.0.secrets {
                body.secrets
                    .entry(key(&entry.scope, &entry.name))
                    .or_insert_with(|| (entry.name.clone(), entry.value.clone()));
            }
        })?;
        *vault.body() = fresh;
        for entry in &copies.0.profiles {
            self.keychain.delete(&entry.scope, &entry.profile.name)?;
        }
        for entry in &copies.0.secrets {
            self.keychain.delete_secret(&entry.scope, &entry.name)?;
        }
        super::left_over(&Wiping(self.keychain.contents()?).0)
    }

    /// Turns the vault on: creates it with `password` (typed twice, the
    /// second time as `repeated`), copies every profile and secret of every
    /// scope from the keychain into it, reads them back, and only then
    /// removes them from the keychain. On a failure the keychain keeps
    /// everything it had and there is no vault file; only if the keychain
    /// can neither remove nor take back its entries is the vault kept (and
    /// used), as the one place that has them all. The vault is unlocked
    /// afterwards.
    ///
    /// The order, and what a crash at each point leaves: the vault file is
    /// written whole beside its place, flushed to the disk and renamed into
    /// it, read back, and only then is anything removed from the keychain;
    /// afterwards the keychain is listed again and must be empty. A crash
    /// before the rename leaves the keychain in use with everything (and a
    /// temporary file, removed at a later start); after it, the vault with
    /// everything and copies still in the keychain, which the next unlock
    /// takes back ([`ActiveStore::unlock`]). The keychain is never emptied
    /// before the vault holding its entries is on the disk.
    ///
    /// Two copies turning it on at once make one vault: the second is
    /// refused with [`VaultError::AlreadyExists`] (and is locked).
    pub fn turn_on(&self, password: &str, repeated: &str) -> Result<(), VaultError> {
        check_new_password(password, repeated)?;
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if !matches!(*state, State::Off) {
            return Err(VaultError::AlreadyExists);
        }
        make_folder(&self.vault_file)?;
        let vault = Vault::keyed(&self.vault_file, password, KdfParams::default())?;
        let lock = WriteLock::take(&self.vault_file)?;
        if present(&self.vault_file) {
            return Err(VaultError::AlreadyExists);
        }
        let contents = Wiping(self.keychain.contents()?.sorted());
        *vault.body() = Body::from_contents(contents.0.clone());
        vault.write_new(&lock)?;
        let undo = |error: VaultError| {
            let _ = fs::remove_file(&self.vault_file);
            error
        };
        let read_back = Wiping(vault.reread(&lock).map_err(undo)?.contents().sorted());
        if read_back.0 != contents.0 {
            return Err(undo(VaultError::Io(
                "what was read back from the vault differs from what was written".into(),
            )));
        }
        match self.empty_keychain(&contents.0) {
            Emptied::All => {}
            Emptied::Restored(error) => {
                return Err(undo(VaultError::Store(StoreError(format!(
                    "cannot remove the profiles and secrets from the keychain, so they stay there: {error}"
                )))))
            }
            Emptied::NotRestored(error) => {
                *state = State::Unlocked(vault);
                return Err(VaultError::Store(StoreError(format!(
                    "cannot remove every profile and secret from the keychain ({error}), nor put back those removed; the vault, which holds them all, is kept and in use, and some are still in the keychain too"
                ))));
            }
        }
        *state = State::Unlocked(vault);
        Ok(())
    }

    /// Removes `contents` from the keychain and lists it again: it must be
    /// empty (a delete that says done is not taken at its word). On a
    /// failure puts back what was already removed, so that the keychain is
    /// complete again.
    fn empty_keychain(&self, contents: &StoreContents) -> Emptied {
        let keychain = self.keychain.as_ref();
        let mut removed = Wiping::default();
        let mut remove = || -> Result<(), StoreError> {
            for entry in &contents.profiles {
                keychain.delete(&entry.scope, &entry.profile.name)?;
                removed.0.profiles.push(entry.clone());
            }
            for entry in &contents.secrets {
                keychain.delete_secret(&entry.scope, &entry.name)?;
                removed.0.secrets.push(entry.clone());
            }
            super::left_over(&Wiping(keychain.contents()?).0)
        };
        let Err(error) = remove() else {
            return Emptied::All;
        };
        let removed = &removed.0;
        let mut restored = true;
        for entry in &removed.profiles {
            restored &= keychain.put(&entry.scope, &entry.profile).is_ok();
        }
        for entry in &removed.secrets {
            restored &= keychain
                .put_secret(&entry.scope, &entry.name, &entry.value)
                .is_ok();
        }
        if restored {
            Emptied::Restored(error)
        } else {
            Emptied::NotRestored(error)
        }
    }

    /// Changes the master password of the unlocked vault: `current` must
    /// open it, and the new one is typed twice (`password`, `repeated`).
    /// The file is sealed again with a new salt and key (the same
    /// key-derivation cost) and replaced atomically, so on a failure the
    /// old password still opens it. Afterwards only the new one does.
    pub fn change_password(
        &self,
        current: &str,
        password: &str,
        repeated: &str,
    ) -> Result<(), VaultError> {
        check_new_password(password, repeated)?;
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let kdf = unlocked_vault(&state)?.kdf();
        // The keys first (the slow part), then the lock: what is sealed
        // again is the file as it is then, other copies' changes included.
        let opened = Vault::unlock(&self.vault_file, current)?;
        let renewed = Vault::keyed(&self.vault_file, password, kdf)?;
        let lock = WriteLock::take(&self.vault_file)?;
        let fresh = opened.reread(&lock)?;
        renewed.write(&lock, &fresh)?;
        drop(lock);
        *renewed.body() = fresh;
        *state = State::Unlocked(renewed);
        Ok(())
    }

    /// Turns the vault off: refused with [`VaultError::NoKeychain`] without
    /// a keychain; `password` (the master password, asked again) must open
    /// it; every profile and secret of every scope is copied to
    /// the keychain, read back, and only then is the vault file removed.
    /// On a failure the vault stays, unlocked and in use, and the keychain
    /// is put back as it was (what it had of its own under the same names
    /// included); the error says if that could not be done in full. An
    /// entry the keychain cannot hold (a Windows credential holds 2,560
    /// bytes) is named before anything is moved.
    ///
    /// The order, and what a crash at each point leaves: the keychain is
    /// filled and read back, then the vault file removed. A crash before
    /// the removal leaves the vault on, with everything, and copies in the
    /// keychain, which the next unlock takes back; after it, the keychain
    /// with everything.
    pub fn turn_off(&self, password: &str) -> Result<(), VaultError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        unlocked_vault(&state)?;
        if !self.has_keychain {
            return Err(VaultError::NoKeychain);
        }
        let opened = Vault::unlock(&self.vault_file, password)?;
        let lock = WriteLock::take(&self.vault_file)?;
        let contents = Wiping(opened.reread(&lock)?.contents());
        self.keychain.check_fits(&contents.0).map_err(|error| {
            VaultError::Store(StoreError(format!(
                "{error}; nothing was moved, and the vault stays on"
            )))
        })?;
        let before = Wiping(self.keychain.contents()?);
        let mut undo = Wiping::default();
        let mut added = Wiping::default();
        let moved = self.fill_keychain(&contents.0, &before.0, &mut undo.0, &mut added.0);
        let result = match moved {
            Ok(()) => match fs::remove_file(&self.vault_file) {
                Ok(()) => Ok(()),
                Err(error) => Err(format!("cannot remove the vault file: {error}")),
            },
            Err(error) => Err(format!(
                "cannot move the profiles and secrets back to the keychain: {error}"
            )),
        };
        let result = match result {
            Ok(()) => {
                *state = State::Off;
                Ok(())
            }
            Err(error) if self.restore_keychain(&undo.0, &added.0) => Err(VaultError::Store(
                StoreError(format!("{error}; the vault stays on and the keychain is as it was")),
            )),
            Err(error) => Err(VaultError::Store(StoreError(format!(
                "{error}; the vault stays on, holding them all, but the keychain could not be put back as it was"
            )))),
        };
        result
    }

    /// Puts `contents` into the keychain and reads them back. Notes, for
    /// each entry before putting it, the keychain's own of that name
    /// (from `before`) in `undo`, or the entry in `added` if it had none.
    fn fill_keychain(
        &self,
        contents: &StoreContents,
        before: &StoreContents,
        undo: &mut StoreContents,
        added: &mut StoreContents,
    ) -> Result<(), StoreError> {
        let keychain = self.keychain.as_ref();
        let same_profile = |a: &ScopedProfile, b: &ScopedProfile| {
            key(&a.scope, &a.profile.name) == key(&b.scope, &b.profile.name)
        };
        let same_secret =
            |a: &ScopedSecret, b: &ScopedSecret| key(&a.scope, &a.name) == key(&b.scope, &b.name);
        for entry in &contents.profiles {
            match before.profiles.iter().find(|old| same_profile(old, entry)) {
                Some(old) => undo.profiles.push(old.clone()),
                None => added.profiles.push(entry.clone()),
            }
            keychain.put(&entry.scope, &entry.profile)?;
        }
        for entry in &contents.secrets {
            match before.secrets.iter().find(|old| same_secret(old, entry)) {
                Some(old) => undo.secrets.push(old.clone()),
                None => added.secrets.push(entry.clone()),
            }
            keychain.put_secret(&entry.scope, &entry.name, &entry.value)?;
        }
        let now = Wiping(keychain.contents()?);
        let all_there = contents
            .profiles
            .iter()
            .all(|entry| now.0.profiles.contains(entry))
            && contents
                .secrets
                .iter()
                .all(|entry| now.0.secrets.contains(entry));
        if !all_there {
            return Err(StoreError(
                "what was read back from the keychain differs from what was written".into(),
            ));
        }
        Ok(())
    }

    /// Takes `added` out of the keychain and puts `undo` back; whether
    /// all of it worked.
    fn restore_keychain(&self, undo: &StoreContents, added: &StoreContents) -> bool {
        let keychain = self.keychain.as_ref();
        let mut restored = true;
        for entry in &added.profiles {
            restored &= keychain.delete(&entry.scope, &entry.profile.name).is_ok();
        }
        for entry in &added.secrets {
            restored &= keychain.delete_secret(&entry.scope, &entry.name).is_ok();
        }
        for entry in &undo.profiles {
            restored &= keychain.put(&entry.scope, &entry.profile).is_ok();
        }
        for entry in &undo.secrets {
            restored &= keychain
                .put_secret(&entry.scope, &entry.name, &entry.value)
                .is_ok();
        }
        restored
    }

    /// Starts over after a forgotten master password: deletes the locked
    /// vault, with every profile and secret in it, and goes back to the
    /// keychain. Refused while the vault is unlocked (turning it off keeps
    /// what it holds). Whatever the keychain still holds (copies a turn on
    /// cut short left) goes first, so that none comes back after a start
    /// over whose warning calls them all gone; if it cannot all go, the
    /// vault is not deleted either.
    pub fn start_over(&self) -> Result<(), VaultError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        match &*state {
            State::Off => return Err(VaultError::NoVault),
            State::Unlocked(_) => return Err(VaultError::Invalid(
                "the vault is unlocked: turn it off instead, which keeps its profiles and secrets"
                    .into(),
            )),
            State::Locked => {}
        }
        let _lock = WriteLock::take(&self.vault_file)?;
        self.keychain.remove_everything().map_err(|error| {
            VaultError::Store(StoreError(format!(
                "cannot remove the copies of profiles and secrets the keychain still holds ({error}), so the vault was not deleted"
            )))
        })?;
        match fs::remove_file(&self.vault_file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(VaultError::Io(error.to_string())),
        }
        *state = State::Off;
        Ok(())
    }

    fn with<T>(
        &self,
        use_store: impl FnOnce(&dyn CredentialStore) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.notice_vault_turned_on() {
            return Err(StoreError(TURNED_ON.into()));
        }
        match &*self.state() {
            State::Off => use_store(self.keychain.as_ref()),
            State::Locked => Err(StoreError(VaultError::Locked.to_string())),
            State::Unlocked(vault) => use_store(vault),
        }
    }

    /// [`with`](Self::with) for a change. While off, the keychain is
    /// changed under the vault's lock, the vault file looked for again
    /// first: another copy turning the vault on holds the lock from listing
    /// the keychain to emptying it, so a save in between would be left out
    /// of its move. (Where no lock file can be made, no vault can be
    /// written either, and the save goes ahead.)
    fn with_change<T>(
        &self,
        change: impl FnOnce(&dyn CredentialStore) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.notice_vault_turned_on() {
            return Err(StoreError(TURNED_ON.into()));
        }
        match &*self.state() {
            State::Off if self.has_keychain => {
                let _lock = WriteLock::take(&self.vault_file).ok();
                if present(&self.vault_file) {
                    return Err(StoreError(TURNED_ON.into()));
                }
                change(self.keychain.as_ref())
            }
            State::Off => change(self.keychain.as_ref()),
            State::Locked => Err(StoreError(VaultError::Locked.to_string())),
            State::Unlocked(vault) => change(vault),
        }
    }
}

impl CredentialStore for ActiveStore {
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        self.with(|store| store.list(scope))
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        self.with(|store| store.get(scope, name))
    }

    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
        self.with_change(|store| store.put(scope, profile))
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.with_change(|store| store.delete(scope, name))
    }

    fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
        self.with(|store| store.secret_names(scope))
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        self.with(|store| store.get_secret(scope, name))
    }

    fn put_secret(&self, scope: &str, name: &str, value: &SecretValue) -> Result<(), StoreError> {
        self.with_change(|store| store.put_secret(scope, name, value))
    }

    fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.with_change(|store| store.delete_secret(scope, name))
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        self.with(|store| store.contents())
    }

    fn vault_status(&self) -> VaultStatus {
        self.status()
    }
}
