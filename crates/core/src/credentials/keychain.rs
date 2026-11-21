//! The operating system's keychain as a [`CredentialStore`].
//!
//! On Windows each profile is one generic credential in Credential Manager,
//! named `<prefix>:<scope>:<profile name in lower case>`, persisted for
//! this user on this machine (never roaming). Its secret is the whole
//! profile as JSON, with the scope again so that a listing keeps only the
//! scope's own entries. A named secret is one generic credential named
//! `<prefix>.secret:<scope>:<secret name in lower case>`: a profile's name
//! always has `:` right after the prefix, so the two never meet, whatever
//! the names. Its secret is the scope, the name as written and the value,
//! as JSON. Other systems have no keychain support yet: every call fails
//! with a [`StoreError`].

use super::{
    CredentialProfile, CredentialStore, ScopedProfile, ScopedSecret, SecretValue, StoreContents,
    StoreError,
};

/// Profiles in the OS keychain, under names starting with a prefix.
#[derive(Debug, Clone)]
pub struct KeychainStore {
    prefix: String,
}

impl KeychainStore {
    /// A store whose entries are named `<prefix>:...`. The app uses its
    /// own identifier; tests use a throwaway prefix of their own.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// Whether this system has a keychain this store can use (Windows'
    /// Credential Manager); elsewhere every call fails, and the app keeps
    /// credentials only in the vault
    /// ([`ActiveStore::without_keychain`](super::ActiveStore::without_keychain)).
    pub fn is_supported() -> bool {
        cfg!(windows)
    }

    fn target(&self, scope: &str, name: &str) -> String {
        format!("{}:{scope}:{}", self.prefix, name.to_lowercase())
    }

    /// The pattern of every entry of `scope`.
    fn filter(&self, scope: &str) -> String {
        format!("{}:{scope}:*", self.prefix)
    }

    fn secret_target(&self, scope: &str, name: &str) -> String {
        format!("{}.secret:{scope}:{}", self.prefix, name.to_lowercase())
    }

    fn secret_filter(&self, scope: &str) -> String {
        format!("{}.secret:{scope}:*", self.prefix)
    }

    /// Removes every entry under the prefix: the profiles and named secrets
    /// of every scope (every workspace), readable or not. Entries under any
    /// other name are not touched. What an uninstall that deletes the app's
    /// data runs (`netterm-cli clear-keychain`).
    pub fn remove_all(&self) -> Result<RemovedEntries, StoreError> {
        let mut removed = RemovedEntries::default();
        for target in os::targets(&format!("{}:*", self.prefix))? {
            if os::delete(&target)? {
                removed.profiles += 1;
            }
        }
        for target in os::targets(&format!("{}.secret:*", self.prefix))? {
            if os::delete(&target)? {
                removed.secrets += 1;
            }
        }
        Ok(removed)
    }

    /// How many entries [`remove_all`](Self::remove_all) would remove now,
    /// removing nothing (`netterm-cli clear-keychain` without `--yes`).
    pub fn count_all(&self) -> Result<RemovedEntries, StoreError> {
        Ok(RemovedEntries {
            profiles: os::targets(&format!("{}:*", self.prefix))?.len(),
            secrets: os::targets(&format!("{}.secret:*", self.prefix))?.len(),
        })
    }
}

/// How many entries [`KeychainStore::remove_all`] removed (or, from
/// [`KeychainStore::count_all`], would remove).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemovedEntries {
    /// Credential profiles.
    pub profiles: usize,
    /// Named secrets.
    pub secrets: usize,
}

/// What kind of entry is written: its comment in Credential Manager and
/// what an error calls it.
#[cfg_attr(not(windows), allow(dead_code))]
struct EntryKind {
    comment: &'static str,
    noun: &'static str,
}

const PROFILE: EntryKind = EntryKind {
    comment: "Credential profile (written by the app; edit it there)",
    noun: "the profile",
};
const SECRET: EntryKind = EntryKind {
    comment: "Named secret (written by the app; edit it there)",
    noun: "the secret",
};

/// What one keychain entry of a named secret holds.
#[derive(serde::Serialize, serde::Deserialize)]
struct SecretEntry {
    scope: String,
    name: String,
    value: SecretValue,
}

fn decode_secret(blob: &[u8], scope: &str) -> Option<SecretEntry> {
    let entry: SecretEntry = serde_json::from_slice(blob).ok()?;
    (entry.scope == scope).then_some(entry)
}

/// What one keychain entry holds.
#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    scope: String,
    profile: CredentialProfile,
}

fn decode(blob: &[u8], scope: &str) -> Option<CredentialProfile> {
    let entry: Entry = serde_json::from_slice(blob).ok()?;
    (entry.scope == scope).then_some(entry.profile)
}

/// A credential's secret as read or written here: JSON with passwords in
/// it, wiped when dropped.
type Blob = zeroize::Zeroizing<Vec<u8>>;

/// What [`Entry`] is written from, borrowed: no copy of the profile.
#[derive(serde::Serialize)]
struct EntryRef<'a> {
    scope: &'a str,
    profile: &'a CredentialProfile,
}

/// What [`SecretEntry`] is written from, borrowed.
#[derive(serde::Serialize)]
struct SecretEntryRef<'a> {
    scope: &'a str,
    name: &'a str,
    value: &'a SecretValue,
}

/// The most a Windows credential's secret holds
/// (`CRED_MAX_CREDENTIAL_BLOB_SIZE`, 5 × 512 bytes).
const MAX_SECRET_BYTES: usize = 2560;
/// The longest user name a Windows credential holds
/// (`CRED_MAX_USERNAME_LENGTH`, in UTF-16 units).
const MAX_USERNAME: usize = 513;

/// The entry's JSON, wiped when dropped.
fn encoded(value: &impl serde::Serialize) -> Result<zeroize::Zeroizing<Vec<u8>>, StoreError> {
    super::encoded_exactly(value).map_err(|error| StoreError(format!("cannot encode: {error}")))
}

fn encode(
    scope: &str,
    profile: &CredentialProfile,
) -> Result<zeroize::Zeroizing<Vec<u8>>, StoreError> {
    encoded(&EntryRef { scope, profile })
}

/// Refuses an entry too large for a Windows credential, naming it.
fn fits(what: &str, secret: usize, username: &str) -> Result<(), StoreError> {
    if secret > MAX_SECRET_BYTES {
        return Err(StoreError(format!(
            "{what} is too large for a Windows credential ({secret} bytes; at most {MAX_SECRET_BYTES}): shorten what it keeps, or delete it"
        )));
    }
    if username.encode_utf16().count() > MAX_USERNAME {
        return Err(StoreError(format!(
            "{what} has a user name too long for a Windows credential (at most {MAX_USERNAME} characters)"
        )));
    }
    Ok(())
}

impl CredentialStore for KeychainStore {
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        Ok(os::list(&self.filter(scope))?
            .iter()
            .filter_map(|blob| decode(blob, scope))
            .collect())
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        Ok(os::read(&self.target(scope, name))?.and_then(|blob| decode(&blob, scope)))
    }

    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
        let blob = encode(scope, profile)?;
        os::write(
            &self.target(scope, &profile.name),
            &profile.username,
            &PROFILE,
            &blob,
        )
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        os::delete(&self.target(scope, name))
    }

    fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
        Ok(os::list(&self.secret_filter(scope))?
            .iter()
            .filter_map(|blob| decode_secret(blob, scope))
            .map(|entry| entry.name)
            .collect())
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        Ok(os::read(&self.secret_target(scope, name))?
            .and_then(|blob| decode_secret(&blob, scope))
            .map(|entry| entry.value))
    }

    fn put_secret(&self, scope: &str, name: &str, value: &SecretValue) -> Result<(), StoreError> {
        let blob = encoded(&SecretEntryRef { scope, name, value })?;
        os::write(&self.secret_target(scope, name), name, &SECRET, &blob)
    }

    fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        os::delete(&self.secret_target(scope, name))
    }

    /// Fails on an entry under the prefix that cannot be read, so that a
    /// move never leaves one behind unnoticed.
    fn contents(&self) -> Result<StoreContents, StoreError> {
        let profiles = os::list(&format!("{}:*", self.prefix))?
            .iter()
            .map(|blob| {
                serde_json::from_slice::<Entry>(blob).map(|entry| ScopedProfile {
                    scope: entry.scope,
                    profile: entry.profile,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(|_| unreadable("profile"))?;
        let secrets = os::list(&format!("{}.secret:*", self.prefix))?
            .iter()
            .map(|blob| {
                serde_json::from_slice::<SecretEntry>(blob).map(|entry| ScopedSecret {
                    scope: entry.scope,
                    name: entry.name,
                    value: entry.value,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(|_| unreadable("secret"))?;
        Ok(StoreContents { profiles, secrets })
    }

    /// Each entry as it would be written, against what a Windows
    /// credential holds.
    fn check_fits(&self, contents: &StoreContents) -> Result<(), StoreError> {
        for entry in &contents.profiles {
            let blob = encode(&entry.scope, &entry.profile)?;
            let what = format!(
                "the profile \"{}\" (workspace {})",
                entry.profile.name, entry.scope
            );
            fits(&what, blob.len(), &entry.profile.username)?;
        }
        for entry in &contents.secrets {
            let blob = encoded(&SecretEntryRef {
                scope: &entry.scope,
                name: &entry.name,
                value: &entry.value,
            })?;
            let what = format!("the secret \"{}\" (workspace {})", entry.name, entry.scope);
            fits(&what, blob.len(), &entry.name)?;
        }
        Ok(())
    }

    /// [`remove_all`](Self::remove_all), readable entries or not, then
    /// counted again: nothing may be left.
    fn remove_everything(&self) -> Result<(), StoreError> {
        self.remove_all()?;
        let left = self.count_all()?;
        if left.profiles + left.secrets > 0 {
            return Err(StoreError(format!(
                "{} are still there after being removed",
                super::counted(left.profiles, left.secrets)
            )));
        }
        Ok(())
    }
}

fn unreadable(what: &str) -> StoreError {
    StoreError(format!(
        "a {what} in the keychain cannot be read (written by something else?)"
    ))
}

#[cfg(windows)]
mod os {
    //! Windows Credential Manager through the `Cred*W` functions.

    use std::ptr;

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
        CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    use super::{Blob, EntryKind, StoreError};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// The last Windows error, unless it is "not found".
    fn failure(action: &str) -> Option<StoreError> {
        // SAFETY: no preconditions.
        let code = unsafe { GetLastError() };
        (code != ERROR_NOT_FOUND).then(|| {
            let error = std::io::Error::from_raw_os_error(code as i32);
            StoreError(format!("cannot {action} the Windows credential: {error}"))
        })
    }

    /// The secret of a credential, copied out.
    ///
    /// # Safety
    /// `credential` must point to a credential Windows returned.
    unsafe fn blob(credential: *const CREDENTIALW) -> Blob {
        let credential = &*credential;
        if credential.CredentialBlob.is_null() {
            return Blob::default();
        }
        Blob::new(
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
            .to_vec(),
        )
    }

    pub fn list(filter: &str) -> Result<Vec<Blob>, StoreError> {
        let filter = wide(filter);
        let mut count = 0;
        let mut credentials: *mut *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: the filter is NUL-terminated; Windows allocates the list,
        // freed below.
        let found = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut credentials) };
        if found == 0 {
            return match failure("list") {
                Some(error) => Err(error),
                None => Ok(Vec::new()),
            };
        }
        // SAFETY: Windows returned `count` credential pointers.
        let blobs = unsafe {
            std::slice::from_raw_parts(credentials, count as usize)
                .iter()
                .map(|&credential| blob(credential))
                .collect()
        };
        // SAFETY: allocated by CredEnumerateW.
        unsafe { CredFree(credentials.cast()) };
        Ok(blobs)
    }

    /// The names of the generic credentials matching `filter`.
    pub fn targets(filter: &str) -> Result<Vec<String>, StoreError> {
        let filter = wide(filter);
        let mut count = 0;
        let mut credentials: *mut *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: as in `list`.
        let found = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut credentials) };
        if found == 0 {
            return match failure("list") {
                Some(error) => Err(error),
                None => Ok(Vec::new()),
            };
        }
        // SAFETY: Windows returned `count` credential pointers, each with a
        // NUL-terminated name.
        let names = unsafe {
            std::slice::from_raw_parts(credentials, count as usize)
                .iter()
                .map(|&credential| &*credential)
                .filter(|credential| {
                    credential.Type == CRED_TYPE_GENERIC && !credential.TargetName.is_null()
                })
                .map(|credential| {
                    let name = credential.TargetName;
                    let length = (0..).take_while(|&i| *name.add(i) != 0).count();
                    String::from_utf16_lossy(std::slice::from_raw_parts(name, length))
                })
                .collect()
        };
        // SAFETY: allocated by CredEnumerateW.
        unsafe { CredFree(credentials.cast()) };
        Ok(names)
    }

    pub fn read(target: &str) -> Result<Option<Blob>, StoreError> {
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: the target is NUL-terminated; Windows allocates the
        // credential, freed below.
        let found = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if found == 0 {
            return match failure("read") {
                Some(error) => Err(error),
                None => Ok(None),
            };
        }
        // SAFETY: Windows returned a credential.
        let secret = unsafe { blob(credential) };
        // SAFETY: allocated by CredReadW.
        unsafe { CredFree(credential.cast()) };
        Ok(Some(secret))
    }

    pub fn write(
        target: &str,
        username: &str,
        kind: &EntryKind,
        secret: &[u8],
    ) -> Result<(), StoreError> {
        if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(StoreError(format!(
                "{} is too long for the Windows credential store (at most {CRED_MAX_CREDENTIAL_BLOB_SIZE} bytes)",
                kind.noun
            )));
        }
        let mut target = wide(target);
        let mut username = wide(username);
        let mut comment = wide(kind.comment);
        // The struct takes mutable pointers; Windows only reads them.
        let mut secret = Blob::new(secret.to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            CredentialBlobSize: secret.len() as u32,
            CredentialBlob: secret.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: username.as_mut_ptr(),
            ..CREDENTIALW::default()
        };
        // SAFETY: every pointer is valid for the call.
        let written = unsafe { CredWriteW(&credential, 0) };
        if written == 0 {
            // SAFETY: no preconditions.
            let code = unsafe { GetLastError() };
            let error = std::io::Error::from_raw_os_error(code as i32);
            return Err(StoreError(format!(
                "cannot write the Windows credential: {error}"
            )));
        }
        Ok(())
    }

    pub fn delete(target: &str) -> Result<bool, StoreError> {
        let target = wide(target);
        // SAFETY: the target is NUL-terminated.
        let deleted = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if deleted == 0 {
            return match failure("delete") {
                Some(error) => Err(error),
                None => Ok(false),
            };
        }
        Ok(true)
    }
}

#[cfg(not(windows))]
mod os {
    use super::{Blob, EntryKind, StoreError};

    fn unsupported() -> StoreError {
        StoreError("this system's keychain is not supported yet".into())
    }

    pub fn list(_filter: &str) -> Result<Vec<Blob>, StoreError> {
        Err(unsupported())
    }

    pub fn targets(_filter: &str) -> Result<Vec<String>, StoreError> {
        Err(unsupported())
    }

    pub fn read(_target: &str) -> Result<Option<Blob>, StoreError> {
        Err(unsupported())
    }

    pub fn write(
        _target: &str,
        _username: &str,
        _kind: &EntryKind,
        _secret: &[u8],
    ) -> Result<(), StoreError> {
        Err(unsupported())
    }

    pub fn delete(_target: &str) -> Result<bool, StoreError> {
        Err(unsupported())
    }
}
