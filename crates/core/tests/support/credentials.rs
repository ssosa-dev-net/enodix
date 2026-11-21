//! A credential store that stands in for a keychain that fails: an
//! in-memory store whose listing, or whose deletes after a number of them,
//! fail as a real keychain can, or whose puts fail after a number of them
//! (or just one of them), or whose every read is refused.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, SecretValue, StoreContents, StoreError,
};

/// How a [`FailingStore`] fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct Failures {
    /// Listing everything ([`CredentialStore::contents`]) fails.
    pub contents: bool,
    /// Deletes (profiles and secrets alike) succeed this many times, then
    /// every one fails.
    pub deletes_after: Option<usize>,
    /// Puts succeed this many times, then every one fails.
    pub puts_after: Option<usize>,
    /// Only this put (counting from 0, profiles and secrets alike) fails.
    pub failing_put: Option<usize>,
    /// Every read (list, get, secret names, a secret, everything) fails
    /// with [`READ_REFUSED`], as a keychain that denies access does.
    pub reads: bool,
}

/// What a [`FailingStore`] with [`Failures::reads`] says to every read.
pub const READ_REFUSED: &str = "access to the keychain was denied";

/// Clones share one store, so a test keeps a handle to look inside after
/// giving one away.
#[derive(Clone)]
pub struct FailingStore {
    inner: Arc<MemoryStore>,
    failures: Failures,
    deletes: Arc<AtomicUsize>,
    puts: Arc<AtomicUsize>,
}

impl FailingStore {
    pub fn new(failures: Failures) -> Self {
        Self {
            inner: Arc::default(),
            failures,
            deletes: Arc::default(),
            puts: Arc::default(),
        }
    }

    /// What it holds, read past the failures.
    pub fn held(&self) -> StoreContents {
        self.inner.contents().unwrap().sorted()
    }

    fn deleting(&self) -> Result<(), StoreError> {
        counted(&self.deletes, self.failures.deletes_after).map(drop)
    }

    fn reading(&self) -> Result<(), StoreError> {
        if self.failures.reads {
            return Err(StoreError(READ_REFUSED.into()));
        }
        Ok(())
    }

    fn putting(&self) -> Result<(), StoreError> {
        let done = counted(&self.puts, self.failures.puts_after)?;
        if self.failures.failing_put == Some(done) {
            return Err(StoreError("the keychain refused this one".into()));
        }
        Ok(())
    }
}

/// Counts one more call (resolving to how many were made before it); fails
/// once `limit` calls were made.
fn counted(calls: &AtomicUsize, limit: Option<usize>) -> Result<usize, StoreError> {
    let done = calls.fetch_add(1, Ordering::SeqCst);
    match limit {
        Some(limit) if done >= limit => Err(StoreError("the keychain refused".into())),
        _ => Ok(done),
    }
}

impl CredentialStore for FailingStore {
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        self.reading()?;
        self.inner.list(scope)
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        self.reading()?;
        self.inner.get(scope, name)
    }

    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
        self.putting()?;
        self.inner.put(scope, profile)
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.deleting()?;
        self.inner.delete(scope, name)
    }

    fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
        self.reading()?;
        self.inner.secret_names(scope)
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        self.reading()?;
        self.inner.get_secret(scope, name)
    }

    fn put_secret(&self, scope: &str, name: &str, value: &SecretValue) -> Result<(), StoreError> {
        self.putting()?;
        self.inner.put_secret(scope, name, value)
    }

    fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        self.deleting()?;
        self.inner.delete_secret(scope, name)
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        self.reading()?;
        if self.failures.contents {
            return Err(StoreError("the keychain cannot be listed".into()));
        }
        self.inner.contents()
    }
}
