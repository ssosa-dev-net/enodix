//! The operating system's keychain as a [`CredentialStore`].
//!
//! On Windows each profile is one generic credential in Credential Manager,
//! named `<prefix>:<scope>:<profile name in lower case>`, persisted for
//! this user on this machine (never roaming). Its secret is the whole
//! profile as JSON, with the scope again so that a listing keeps only the
//! scope's own entries. Other systems have no keychain support yet: every
//! call fails with a [`StoreError`].

use super::{CredentialProfile, CredentialStore, StoreError};

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

    fn target(&self, scope: &str, name: &str) -> String {
        format!("{}:{scope}:{}", self.prefix, name.to_lowercase())
    }

    /// The pattern of every entry of `scope`.
    fn filter(&self, scope: &str) -> String {
        format!("{}:{scope}:*", self.prefix)
    }
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

fn encode(scope: &str, profile: &CredentialProfile) -> Result<Vec<u8>, StoreError> {
    let entry = Entry {
        scope: scope.to_owned(),
        profile: profile.clone(),
    };
    serde_json::to_vec(&entry).map_err(|error| StoreError(format!("cannot encode: {error}")))
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
        os::write(&self.target(scope, &profile.name), &profile.username, &blob)
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        os::delete(&self.target(scope, name))
    }
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

    use super::StoreError;

    const COMMENT: &str = "Credential profile (written by the app; edit it there)";

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
    unsafe fn blob(credential: *const CREDENTIALW) -> Vec<u8> {
        let credential = &*credential;
        if credential.CredentialBlob.is_null() {
            return Vec::new();
        }
        std::slice::from_raw_parts(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        )
        .to_vec()
    }

    pub fn list(filter: &str) -> Result<Vec<Vec<u8>>, StoreError> {
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

    pub fn read(target: &str) -> Result<Option<Vec<u8>>, StoreError> {
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

    pub fn write(target: &str, username: &str, secret: &[u8]) -> Result<(), StoreError> {
        if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(StoreError(format!(
                "the profile is too long for the Windows credential store (at most {CRED_MAX_CREDENTIAL_BLOB_SIZE} bytes)"
            )));
        }
        let mut target = wide(target);
        let mut username = wide(username);
        let mut comment = wide(COMMENT);
        // The struct takes mutable pointers; Windows only reads them.
        let mut secret = secret.to_vec();
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
    use super::StoreError;

    fn unsupported() -> StoreError {
        StoreError("this system's keychain is not supported yet".into())
    }

    pub fn list(_filter: &str) -> Result<Vec<Vec<u8>>, StoreError> {
        Err(unsupported())
    }

    pub fn read(_target: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Err(unsupported())
    }

    pub fn write(_target: &str, _username: &str, _secret: &[u8]) -> Result<(), StoreError> {
        Err(unsupported())
    }

    pub fn delete(_target: &str) -> Result<bool, StoreError> {
        Err(unsupported())
    }
}
