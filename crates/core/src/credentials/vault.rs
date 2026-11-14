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

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};

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

/// Where the vault lives in the per-user data folder `data_folder`.
pub fn vault_file(data_folder: &Path) -> PathBuf {
    data_folder.join(VAULT_FILE)
}

const FORMAT: &str = "netterm-vault";
const VERSION: u32 = 1;
const ALGORITHM: &str = "argon2id";
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const MIN_SALT_LEN: usize = 16;
/// The most memory a header may ask the key derivation for (4 GiB): a
/// damaged or hostile header must not exhaust the machine.
const MAX_MEMORY_KIB: u32 = 4 * 1024 * 1024;

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
/// Every change reads the file again first (so a change written by another
/// handle on the same file is kept), applies itself and writes the whole
/// file; changes are serialised within the vault. Two app instances
/// writing at once can still lose one change.
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
    /// Whether there is a vault file at `path`.
    pub fn exists(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
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
        usable_password(password)?;
        if Self::exists(path) {
            return Err(VaultError::AlreadyExists);
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|error| VaultError::Io(error.to_string()))?;
        }
        Self::seal_new(path, password, kdf, contents)
    }

    /// Writes `contents` to `path` under `password` with a new salt,
    /// replacing whatever file is there (atomically: on a failure the old
    /// file stays as it was).
    fn seal_new(
        path: &Path,
        password: &str,
        kdf: KdfParams,
        contents: StoreContents,
    ) -> Result<Self, VaultError> {
        // Held as a body first, so its secrets are wiped on any failure.
        let body = Body::from_contents(contents);
        usable_password(password)?;
        let salt = Key::generate().to_vec();
        let key = derive_key(password, &kdf, &salt)?;
        let vault = Self {
            path: path.to_path_buf(),
            key,
            kdf,
            salt,
            body: Mutex::new(body),
        };
        vault.write(&vault.body())?;
        Ok(vault)
    }

    /// Opens the vault at `path` with `password`.
    pub fn unlock(path: &Path, password: &str) -> Result<Self, VaultError> {
        let (sealed, salt) = read_sealed(path)?;
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

    /// The file's contents as they are now, opened with this vault's key.
    fn reread(&self) -> Result<Body, VaultError> {
        let (sealed, salt) = read_sealed(&self.path)?;
        if salt != self.salt || sealed.kdf.params != self.kdf {
            return Err(VaultError::Damaged(
                "it was replaced since it was unlocked".into(),
            ));
        }
        open(&sealed, &self.key).map_err(|error| match error {
            VaultError::WrongPassword => {
                VaultError::Damaged("it was replaced since it was unlocked".into())
            }
            other => other,
        })
    }

    /// Applies `edit` to the file's current contents and writes them.
    fn change<T>(&self, edit: impl FnOnce(&mut Body) -> T) -> Result<T, StoreError> {
        let mut held = self.body();
        let mut fresh = self.reread().map_err(store_error)?;
        let result = edit(&mut fresh);
        self.write(&fresh).map_err(store_error)?;
        *held = fresh;
        Ok(result)
    }

    /// Seals `body` and replaces the file with it.
    fn write(&self, body: &Body) -> Result<(), VaultError> {
        let plain = Zeroizing::new(
            serde_json::to_vec(&body.contents())
                .map_err(|error| VaultError::Io(format!("cannot encode the contents: {error}")))?,
        );
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

/// Reads and checks the file at `path`: everything but what needs the key.
fn read_sealed(path: &Path) -> Result<(Sealed, Vec<u8>), VaultError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(VaultError::NoVault)
        }
        Err(error) => return Err(VaultError::Io(error.to_string())),
    };
    let sealed: Sealed = serde_json::from_slice(&bytes).map_err(|_| {
        VaultError::Damaged("it cannot be read (cut short, or not a vault file)".into())
    })?;
    if sealed.format != FORMAT {
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

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
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

/// The credential store the app uses: the keychain it is given while there
/// is no vault file, the vault once one exists. Owns the unlocked vault, so
/// everything that serves credentials goes through it.
pub struct ActiveStore {
    keychain: Box<dyn CredentialStore>,
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
    pub fn new(keychain: Box<dyn CredentialStore>, vault_file: impl Into<PathBuf>) -> Self {
        let vault_file = vault_file.into();
        let state = if Vault::exists(&vault_file) {
            State::Locked
        } else {
            State::Off
        };
        Self {
            keychain,
            vault_file,
            state: RwLock::new(state),
        }
    }

    pub fn status(&self) -> VaultStatus {
        match &*self.state() {
            State::Off => VaultStatus::Off,
            State::Locked => VaultStatus::Locked,
            State::Unlocked(_) => VaultStatus::Unlocked,
        }
    }

    fn state(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Unlocks the vault with `password`; it stays unlocked as long as this
    /// store lives. On an unlocked vault the password is still checked.
    pub fn unlock(&self, password: &str) -> Result<(), VaultError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        match *state {
            State::Off => Err(VaultError::NoVault),
            State::Unlocked(_) => Vault::unlock(&self.vault_file, password).map(drop),
            State::Locked => {
                *state = State::Unlocked(Vault::unlock(&self.vault_file, password)?);
                Ok(())
            }
        }
    }

    /// Turns the vault on: creates it with `password` (typed twice, the
    /// second time as `repeated`), copies every profile and secret of every
    /// scope from the keychain into it, reads them back, and only then
    /// removes them from the keychain. On a failure the keychain keeps
    /// everything it had and there is no vault file; only if the keychain
    /// can neither remove nor take back its entries is the vault kept (and
    /// used), as the one place that has them all. The vault is unlocked
    /// afterwards.
    pub fn turn_on(&self, password: &str, repeated: &str) -> Result<(), VaultError> {
        check_new_password(password, repeated)?;
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if !matches!(*state, State::Off) {
            return Err(VaultError::AlreadyExists);
        }
        let contents = self.keychain.contents()?.sorted();
        let vault = Vault::create_with(
            &self.vault_file,
            password,
            KdfParams::default(),
            contents.clone(),
        )?;
        let undo = |error: VaultError| {
            let _ = fs::remove_file(&self.vault_file);
            error
        };
        let read_back = vault.reread().map_err(undo)?.contents().sorted();
        if read_back != contents {
            return Err(undo(VaultError::Io(
                "what was read back from the vault differs from what was written".into(),
            )));
        }
        match self.empty_keychain(&contents) {
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

    /// Removes `contents` from the keychain; on a failure puts back what
    /// was already removed, so that the keychain is complete again.
    fn empty_keychain(&self, contents: &StoreContents) -> Emptied {
        let keychain = self.keychain.as_ref();
        let mut removed = StoreContents::default();
        let mut remove = || -> Result<(), StoreError> {
            for entry in &contents.profiles {
                keychain.delete(&entry.scope, &entry.profile.name)?;
                removed.profiles.push(entry.clone());
            }
            for entry in &contents.secrets {
                keychain.delete_secret(&entry.scope, &entry.name)?;
                removed.secrets.push(entry.clone());
            }
            Ok(())
        };
        let Err(error) = remove() else {
            return Emptied::All;
        };
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
        let contents = Vault::unlock(&self.vault_file, current)?.body().contents();
        let renewed = Vault::seal_new(&self.vault_file, password, kdf, contents)?;
        *state = State::Unlocked(renewed);
        Ok(())
    }

    /// Turns the vault off: `password` (the master password, asked again)
    /// must open it; every profile and secret of every scope is copied to
    /// the keychain, read back, and only then is the vault file removed.
    /// On a failure the vault stays, unlocked and in use, and the keychain
    /// is put back as it was (what it had of its own under the same names
    /// included); the error says if that could not be done in full.
    pub fn turn_off(&self, password: &str) -> Result<(), VaultError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        unlocked_vault(&state)?;
        let contents = Wiping(Vault::unlock(&self.vault_file, password)?.body().contents());
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
        let now = keychain.contents()?;
        let all_there = contents
            .profiles
            .iter()
            .all(|entry| now.profiles.contains(entry))
            && contents
                .secrets
                .iter()
                .all(|entry| now.secrets.contains(entry));
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
    /// what it holds).
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
        match &*self.state() {
            State::Off => use_store(self.keychain.as_ref()),
            State::Locked => Err(StoreError(VaultError::Locked.to_string())),
            State::Unlocked(vault) => use_store(vault),
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
        self.with(|store| store.put(scope, profile))
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.with(|store| store.delete(scope, name))
    }

    fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
        self.with(|store| store.secret_names(scope))
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        self.with(|store| store.get_secret(scope, name))
    }

    fn put_secret(&self, scope: &str, name: &str, value: &SecretValue) -> Result<(), StoreError> {
        self.with(|store| store.put_secret(scope, name, value))
    }

    fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.with(|store| store.delete_secret(scope, name))
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        self.with(|store| store.contents())
    }

    fn vault_status(&self) -> VaultStatus {
        self.status()
    }
}
