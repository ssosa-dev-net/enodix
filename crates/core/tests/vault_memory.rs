//! What turning the vault on and off leaves in the memory it gives back
//! (#125): no block of memory freed while moving the profiles and secrets
//! between the keychain and the vault still holds one of their secrets in
//! the clear. An allocator around the system's looks at every block as it
//! is freed, while a move runs.
//!
//! The keychain here is a stand-in that wipes what it deletes, so that what
//! is found is the vault's own copies. (A real keychain's copies are made by
//! the system, out of this allocator's sight.) Its own binary: the
//! allocator sees every thread of the process, and no other test runs here.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use enodix_core::credentials::{
    vault_file, ActiveStore, CredentialProfile, CredentialStore, ProfileAuth, ScopedProfile,
    ScopedSecret, SecretValue, StoreContents, StoreError,
};
use zeroize::Zeroize;

/// What every secret of the test holds: easy to look for, never in a name.
const MARK: &[u8] = b"Wm9-secret-in-the-clear-Q7x";

struct Watching;

static WATCHING: AtomicBool = AtomicBool::new(false);
static FOUND: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        if WATCHING.load(Ordering::Relaxed) && layout.size() >= MARK.len() {
            // SAFETY: the block is still ours until it is given back below.
            let bytes = unsafe { std::slice::from_raw_parts(block, layout.size()) };
            if bytes.windows(MARK.len()).any(|window| window == MARK) {
                FOUND.fetch_add(1, Ordering::Relaxed);
            }
        }
        unsafe { System.dealloc(block, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Watching = Watching;

fn secret(what: &str) -> String {
    format!("{}-{what}", String::from_utf8_lossy(MARK))
}

fn wipe_profile(profile: &mut CredentialProfile) {
    if let ProfileAuth::Password { password } = &mut profile.auth {
        password.zeroize();
    }
    if let Some(enable) = &mut profile.enable_secret {
        enable.zeroize();
    }
}

/// A keychain that wipes what it deletes.
#[derive(Default)]
struct WipingKeychain {
    held: Mutex<StoreContents>,
}

impl CredentialStore for WipingKeychain {
    fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
        let held = self.held.lock().unwrap();
        Ok(held
            .profiles
            .iter()
            .filter(|entry| entry.scope == scope)
            .map(|entry| entry.profile.clone())
            .collect())
    }

    fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
        let held = self.held.lock().unwrap();
        Ok(held
            .profiles
            .iter()
            .find(|entry| entry.scope == scope && entry.profile.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.profile.clone()))
    }

    fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
        self.delete(scope, &profile.name)?;
        self.held.lock().unwrap().profiles.push(ScopedProfile {
            scope: scope.into(),
            profile: profile.clone(),
        });
        Ok(())
    }

    fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        let mut held = self.held.lock().unwrap();
        let Some(at) = held.profiles.iter().position(|entry| {
            entry.scope == scope && entry.profile.name.eq_ignore_ascii_case(name)
        }) else {
            return Ok(false);
        };
        let mut gone = held.profiles.remove(at);
        wipe_profile(&mut gone.profile);
        Ok(true)
    }

    fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
        let held = self.held.lock().unwrap();
        Ok(held
            .secrets
            .iter()
            .filter(|entry| entry.scope == scope)
            .map(|entry| entry.name.clone())
            .collect())
    }

    fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
        let held = self.held.lock().unwrap();
        Ok(held
            .secrets
            .iter()
            .find(|entry| entry.scope == scope && entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.value.clone()))
    }

    fn put_secret(&self, scope: &str, name: &str, value: &SecretValue) -> Result<(), StoreError> {
        self.delete_secret(scope, name)?;
        self.held.lock().unwrap().secrets.push(ScopedSecret {
            scope: scope.into(),
            name: name.into(),
            value: value.clone(),
        });
        Ok(())
    }

    fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
        let mut held = self.held.lock().unwrap();
        let Some(at) = held
            .secrets
            .iter()
            .position(|entry| entry.scope == scope && entry.name.eq_ignore_ascii_case(name))
        else {
            return Ok(false);
        };
        let gone = held.secrets.remove(at);
        // A secret value has no way in from outside the crate but its
        // text: forget it rather than free it unwiped.
        std::mem::forget(gone);
        Ok(true)
    }

    fn contents(&self) -> Result<StoreContents, StoreError> {
        Ok(self.held.lock().unwrap().clone())
    }
}

/// Runs `move_them` while freed memory is watched; how many freed blocks
/// held a secret.
fn freed_with_a_secret(move_them: impl FnOnce()) -> usize {
    FOUND.store(0, Ordering::SeqCst);
    WATCHING.store(true, Ordering::SeqCst);
    move_them();
    WATCHING.store(false, Ordering::SeqCst);
    FOUND.load(Ordering::SeqCst)
}

#[test]
fn moving_profiles_and_secrets_in_and_out_of_the_vault_frees_no_secret_in_the_clear() {
    let data = tempfile::TempDir::new().unwrap();
    let keychain = std::sync::Arc::new(WipingKeychain::default());
    for (scope, name) in [("ws-a", "core"), ("ws-a", "edge"), ("ws-b", "core")] {
        keychain
            .put(
                scope,
                &CredentialProfile {
                    name: name.into(),
                    username: "noc".into(),
                    auth: ProfileAuth::Password {
                        password: secret(name),
                    },
                    enable_secret: Some(secret("enable")),
                },
            )
            .unwrap();
    }
    keychain
        .put_secret("ws-a", "netbox", &SecretValue::new(secret("token")))
        .unwrap();
    struct Shared(std::sync::Arc<WipingKeychain>);
    impl CredentialStore for Shared {
        fn list(&self, scope: &str) -> Result<Vec<CredentialProfile>, StoreError> {
            self.0.list(scope)
        }
        fn get(&self, scope: &str, name: &str) -> Result<Option<CredentialProfile>, StoreError> {
            self.0.get(scope, name)
        }
        fn put(&self, scope: &str, profile: &CredentialProfile) -> Result<(), StoreError> {
            self.0.put(scope, profile)
        }
        fn delete(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
            self.0.delete(scope, name)
        }
        fn secret_names(&self, scope: &str) -> Result<Vec<String>, StoreError> {
            self.0.secret_names(scope)
        }
        fn get_secret(&self, scope: &str, name: &str) -> Result<Option<SecretValue>, StoreError> {
            self.0.get_secret(scope, name)
        }
        fn put_secret(
            &self,
            scope: &str,
            name: &str,
            value: &SecretValue,
        ) -> Result<(), StoreError> {
            self.0.put_secret(scope, name, value)
        }
        fn delete_secret(&self, scope: &str, name: &str) -> Result<bool, StoreError> {
            self.0.delete_secret(scope, name)
        }
        fn contents(&self) -> Result<StoreContents, StoreError> {
            self.0.contents()
        }
    }
    let store = ActiveStore::new(Box::new(Shared(keychain.clone())), vault_file(data.path()));
    let master = "correct horse battery staple";

    let turning_on = freed_with_a_secret(|| store.turn_on(master, master).unwrap());
    assert!(keychain.contents().unwrap().is_empty());
    assert_eq!(turning_on, 0, "blocks freed with a secret while turning on");

    let turning_off = freed_with_a_secret(|| store.turn_off(master).unwrap());
    assert_eq!(keychain.contents().unwrap().profiles.len(), 3);
    assert_eq!(
        turning_off, 0,
        "blocks freed with a secret while turning off"
    );
}
