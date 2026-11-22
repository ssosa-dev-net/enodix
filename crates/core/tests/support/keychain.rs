//! The real Windows Credential Manager in tests (#138): the sweep of what
//! test processes killed part-way left behind.
//!
//! Every test that writes Credential Manager uses a random
//! `enodix-test-<hex>` prefix and removes everything under it when it
//! ends, a failed (panicking) test included. A test process that is killed
//! (a command that timed out, a stopped run) removes nothing, and its
//! entries stay: some hundreds of them made Credential Manager refuse every
//! write ("Not enough memory resources are available to process this
//! command", os error 8). [`sweep_leftovers`] removes them at the start of
//! the next run.
//!
//! The headless binary's tests use this file too (`#[path]`).
//!
//! Only names starting with [`TEST_PREFIX`] are looked at; nothing else in
//! the user's Credential Manager is read, changed or removed.
#![cfg(windows)]

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use enodix_core::credentials::KeychainStore;

/// What every test prefix starts with.
pub const TEST_PREFIX: &str = "enodix-test-";

/// How long ago a prefix's every entry must have been written for its
/// entries to be leftovers. No test runs this long, so no running test's
/// entries are taken.
pub const LEFTOVER_AGE: Duration = Duration::from_secs(60 * 60);

/// [`sweep`] once per test process, before its first entry is written.
/// A failure is printed, not fatal: the test goes on and says what it
/// meets.
pub fn sweep_leftovers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| match sweep(LEFTOVER_AGE) {
        Ok(0) => {}
        Ok(removed) => {
            eprintln!("removed {removed} leftover enodix-test-* prefixes from Credential Manager")
        }
        Err(error) => eprintln!("could not sweep leftover enodix-test-* entries: {error}"),
    });
}

/// Removes the test prefixes whose every entry was last written more than
/// `age` ago: a prefix with one younger entry is a running test's, and is
/// kept whole. A prefix is removed through [`KeychainStore::remove_all`]
/// (so it takes turns with every other process), and only if every name
/// under it starts with the prefix exactly as written (Credential
/// Manager's filter ignores case). Returns how many prefixes were removed.
pub fn sweep(age: Duration) -> Result<usize, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // Per prefix: whether every entry is old and named exactly so.
    let mut prefixes: BTreeMap<String, bool> = BTreeMap::new();
    for (target, written) in entries(&format!("{TEST_PREFIX}*"))? {
        let Some(prefix) = prefix_of(&target) else {
            continue;
        };
        let old = now.saturating_sub(written) > age.as_secs();
        *prefixes.entry(prefix.to_owned()).or_insert(true) &= old;
    }
    let mut removed = 0;
    for (prefix, all_old) in prefixes {
        if !all_old {
            continue;
        }
        // The filter ignores case: every name it matches must start with
        // the prefix exactly, or the prefix is left alone.
        let exact = [format!("{prefix}:*"), format!("{prefix}.secret:*")]
            .iter()
            .map(|filter| entries(filter))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .all(|(target, _)| {
                target.starts_with(&format!("{prefix}:"))
                    || target.starts_with(&format!("{prefix}.secret:"))
            });
        if !exact {
            continue;
        }
        KeychainStore::new(prefix)
            .remove_all()
            .map_err(|error| error.to_string())?;
        removed += 1;
    }
    Ok(removed)
}

/// The prefix a test entry's name was written under: what comes before the
/// first `:`, without a named secret's `.secret`. `None` for a name not
/// starting with [`TEST_PREFIX`] as written, or with no `:`.
pub fn prefix_of(target: &str) -> Option<&str> {
    if !target.starts_with(TEST_PREFIX) {
        return None;
    }
    let (prefix, _) = target.split_once(':')?;
    let prefix = prefix.strip_suffix(".secret").unwrap_or(prefix);
    (prefix.len() > TEST_PREFIX.len()).then_some(prefix)
}

/// The name and last-written time (seconds since 1970) of every generic
/// credential matching `filter`, which must start with [`TEST_PREFIX`].
/// Their secrets are not looked at.
pub fn entries(filter: &str) -> Result<Vec<(String, u64)>, String> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredEnumerateW, CredFree, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    assert!(filter.starts_with(TEST_PREFIX), "{filter}");
    let wide: Vec<u16> = filter.encode_utf16().chain(Some(0)).collect();
    let mut count = 0;
    let mut credentials: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: a NUL-terminated filter; Windows allocates the list, freed
    // below.
    if unsafe { CredEnumerateW(wide.as_ptr(), 0, &mut count, &mut credentials) } == 0 {
        // SAFETY: no preconditions.
        let code = unsafe { GetLastError() };
        return if code == ERROR_NOT_FOUND {
            Ok(Vec::new())
        } else {
            Err(std::io::Error::from_raw_os_error(code as i32).to_string())
        };
    }
    // SAFETY: Windows returned `count` credentials, each with a
    // NUL-terminated name.
    let found = unsafe {
        std::slice::from_raw_parts(credentials, count as usize)
            .iter()
            .map(|&credential| &*credential)
            .filter(|credential| {
                credential.Type == CRED_TYPE_GENERIC && !credential.TargetName.is_null()
            })
            .map(|credential| {
                let name = credential.TargetName;
                let length = (0..).take_while(|&i| *name.add(i) != 0).count();
                let target = String::from_utf16_lossy(std::slice::from_raw_parts(name, length));
                let time = credential.LastWritten;
                let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
                // 100 ns ticks since 1601 to seconds since 1970.
                let written = (ticks / 10_000_000).saturating_sub(11_644_473_600);
                (target, written)
            })
            .collect()
    };
    // SAFETY: allocated by CredEnumerateW.
    unsafe { CredFree(credentials.cast()) };
    Ok(found)
}
