//! A credential store that stands in for a keychain that fails: an
//! in-memory store whose listing, or whose deletes after a number of them,
//! fail as a real keychain can, or whose puts fail after a number of them
//! (or just one of them), or whose every read is refused; whose deletes
//! say done and keep the entry; which refuses to hold a password longer
//! than a limit (as a Windows credential refuses a large entry); and whose
//! listing can stop at a [`Gate`] until the test opens it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, SecretValue, StoreContents,
    StoreError,
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
    /// Deletes say they removed the entry and keep it.
    pub deletes_kept: bool,
    /// [`CredentialStore::check_fits`] refuses a profile whose password is
    /// longer than this, with [`TOO_LARGE`].
    pub largest_password: Option<usize>,
}

/// What a [`FailingStore`] with [`Failures::reads`] says to every read.
pub const READ_REFUSED: &str = "access to the keychain was denied";
/// What a [`FailingStore`] with [`Failures::largest_password`] says.
pub const TOO_LARGE: &str = "the stand-in keychain cannot hold that profile";

/// A point where a [`FailingStore`]'s listing ([`CredentialStore::contents`])
/// stops until the test opens it: the test knows a move is under way and
/// holds it there. Once open, listings go through.
#[derive(Clone, Default)]
pub struct Gate {
    state: Arc<(Mutex<GateState>, Condvar)>,
}

#[derive(Default)]
struct GateState {
    reached: bool,
    open: bool,
}

impl Gate {
    /// Waits until a listing reached the gate; whether one did `within`.
    pub fn wait_reached(&self, within: Duration) -> bool {
        let (state, changed) = &*self.state;
        let until = Instant::now() + within;
        let mut held = state.lock().unwrap();
        while !held.reached {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            held = changed.wait_timeout(held, left).unwrap().0;
        }
        true
    }

    /// Lets the listing waiting at the gate, and every later one, go on.
    pub fn open(&self) {
        let (state, changed) = &*self.state;
        state.lock().unwrap().open = true;
        changed.notify_all();
    }

    fn pass(&self) {
        let (state, changed) = &*self.state;
        let mut held = state.lock().unwrap();
        held.reached = true;
        changed.notify_all();
        while !held.open {
            held = changed.wait(held).unwrap();
        }
    }
}

/// Clones share one store, so a test keeps a handle to look inside after
/// giving one away.
#[derive(Clone)]
pub struct FailingStore {
    inner: Arc<MemoryStore>,
    failures: Failures,
    deletes: Arc<AtomicUsize>,
    puts: Arc<AtomicUsize>,
    gate: Option<Gate>,
}

impl FailingStore {
    pub fn new(failures: Failures) -> Self {
        Self {
            inner: Arc::default(),
            failures,
            deletes: Arc::default(),
            puts: Arc::default(),
            gate: None,
        }
    }

    /// A store whose listings stop at `gate` until it is opened.
    pub fn gated(failures: Failures, gate: Gate) -> Self {
        Self {
            gate: Some(gate),
            ..Self::new(failures)
        }
    }

    /// What it holds, read past the failures.
    pub fn held(&self) -> StoreContents {
        self.inner.contents().unwrap().sorted()
    }

    /// How many puts (profiles and secrets) were asked of it so far.
    pub fn puts_made(&self) -> usize {
        self.puts.load(Ordering::SeqCst)
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
        if self.failures.deletes_kept {
            return Ok(self.inner.get(scope, name)?.is_some());
        }
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
        if self.failures.deletes_kept {
            return Ok(self.inner.get_secret(scope, name)?.is_some());
        }
        self.inner.delete_secret(scope, name)
    }

    /// Stops at the gate, if any, once the listing is taken: what is put
    /// while it waits is not in it.
    fn contents(&self) -> Result<StoreContents, StoreError> {
        let listed = (|| {
            self.reading()?;
            if self.failures.contents {
                return Err(StoreError("the keychain cannot be listed".into()));
            }
            self.inner.contents()
        })();
        if let Some(gate) = &self.gate {
            gate.pass();
        }
        listed
    }

    fn check_fits(&self, contents: &StoreContents) -> Result<(), StoreError> {
        let Some(largest) = self.failures.largest_password else {
            return Ok(());
        };
        let too_large = contents.profiles.iter().any(|entry| {
            matches!(&entry.profile.auth, ProfileAuth::Password { password } if password.len() > largest)
        });
        if too_large {
            return Err(StoreError(TOO_LARGE.into()));
        }
        Ok(())
    }
}
