//! Windows Credential Manager used by several processes at once, through
//! the bare Win32 calls (`CredWriteW`, `CredReadW`, `CredEnumerateW`,
//! `CredDeleteW`) and nothing of enodix's (#138).
//!
//! ```text
//! cargo run -p enodix-core --example credential_manager_race -- [processes] [rounds] [filler]
//! ```
//!
//! Starts `processes` copies of itself (default 8). Each takes a random
//! prefix `enodix-test-race-<hex>` of its own, writes `filler` entries
//! under it that it leaves alone (default 0: more entries in the store),
//! then for `rounds` rounds (default 300) writes an entry, reads it back,
//! overwrites it, reads it back, lists its prefix (`<prefix>:*`), deletes
//! the entry, reads it again and lists again. Every answer that differs
//! from what the process itself just did is counted by kind and the first
//! few printed. Each process removes what is under its prefix when it
//! ends; the parent removes it again once they have all ended (also after
//! a failure). Only targets under `enodix-test-race-` are written, read,
//! listed or deleted.
//!
//! Environment variables change the run:
//!
//! - `ENODIX_RACE_THREADS=1`: the workers are threads of one process.
//! - `ENODIX_RACE_LOCK=1`: every call holds an exclusive lock on one file
//!   in the temporary folder, so no two calls overlap.
//! - `ENODIX_RACE_SETTLE=1`: a read that missed is repeated every 5 ms
//!   for up to 5 s, to see whether the store comes to show the write.
//! - `ENODIX_RACE_LINGER_MS=<ms>`: a read-back that was right is repeated
//!   that much later, to see whether it stays right.
//!
//! `credential_manager_race count` prints how many `enodix-test-*`
//! entries the store holds, and how many were last written more than an
//! hour ago, without reading any of their secrets' meaning;
//! `credential_manager_race clean` removes what a stopped run left
//! (`enodix-test-race-*` only).

#[cfg(not(windows))]
fn main() {
    eprintln!("Credential Manager exists on Windows only");
}

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(windows)]
mod win {
    use std::collections::BTreeMap;
    use std::process::Command;
    use std::ptr;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND, FILETIME};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW, CREDENTIALW,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    const RACE: &str = "enodix-test-race-";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn last_error() -> u32 {
        // SAFETY: no preconditions.
        unsafe { GetLastError() }
    }

    /// With `ENODIX_RACE_LOCK` set, every call holds an exclusive lock on
    /// one file in the temporary folder, so that no two calls of these
    /// processes overlap: the processes take turns, nothing else changes.
    fn turn() -> Option<std::fs::File> {
        std::env::var_os("ENODIX_RACE_LOCK")?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(std::env::temp_dir().join("enodix-test-race.lock"))
            .unwrap();
        file.lock().unwrap();
        Some(file)
    }

    fn write(target: &str, value: &str) -> Result<(), u32> {
        assert!(target.starts_with(RACE));
        let _turn = turn();
        let mut target = wide(target);
        let mut user = wide("race");
        let mut blob = value.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..CREDENTIALW::default()
        };
        // SAFETY: every pointer is valid for the call.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }

    fn blob(credential: &CREDENTIALW) -> String {
        if credential.CredentialBlob.is_null() {
            return String::new();
        }
        // SAFETY: Windows returned the blob with its size.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
        };
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn name(credential: &CREDENTIALW) -> String {
        let name = credential.TargetName;
        // SAFETY: a NUL-terminated name Windows returned.
        unsafe {
            let length = (0..).take_while(|&i| *name.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(name, length))
        }
    }

    fn read(target: &str) -> Result<Option<String>, u32> {
        assert!(target.starts_with(RACE));
        let _turn = turn();
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: NUL-terminated target; freed below.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            let code = last_error();
            return if code == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(code)
            };
        }
        // SAFETY: Windows returned a credential.
        let value = blob(unsafe { &*credential });
        // SAFETY: allocated by CredReadW.
        unsafe { CredFree(credential.cast()) };
        Ok(Some(value))
    }

    /// (target, secret, last written) of every credential matching `filter`.
    fn list(filter: &str) -> Result<Vec<(String, String, FILETIME)>, u32> {
        assert!(filter.starts_with("enodix-test-"));
        let _turn = turn();
        let filter = wide(filter);
        let mut count = 0;
        let mut credentials: *mut *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: NUL-terminated filter; freed below.
        if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut credentials) } == 0 {
            let code = last_error();
            return if code == ERROR_NOT_FOUND {
                Ok(Vec::new())
            } else {
                Err(code)
            };
        }
        // SAFETY: Windows returned `count` credentials.
        let found = unsafe {
            std::slice::from_raw_parts(credentials, count as usize)
                .iter()
                .map(|&credential| {
                    let credential = &*credential;
                    (name(credential), blob(credential), credential.LastWritten)
                })
                .collect()
        };
        // SAFETY: allocated by CredEnumerateW.
        unsafe { CredFree(credentials.cast()) };
        Ok(found)
    }

    fn delete(target: &str) -> Result<bool, u32> {
        assert!(target.starts_with(RACE));
        let _turn = turn();
        let target = wide(target);
        // SAFETY: NUL-terminated target.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let code = last_error();
            return if code == ERROR_NOT_FOUND {
                Ok(false)
            } else {
                Err(code)
            };
        }
        Ok(true)
    }

    fn remove_prefix(prefix: &str) -> usize {
        let mut removed = 0;
        for (target, _, _) in list(&format!("{prefix}:*")).unwrap_or_default() {
            if target.starts_with(RACE) && delete(&target) == Ok(true) {
                removed += 1;
            }
        }
        removed
    }

    /// Removes the prefix's entries when dropped (a panic included).
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            remove_prefix(&self.0);
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match args.first().map(String::as_str) {
            Some("count") => count(),
            // What a stopped run of this program left (its own names only).
            Some("clean") => {
                let mut removed = 0;
                for (target, _, _) in list(&format!("{RACE}*")).unwrap_or_default() {
                    if target.starts_with(RACE) && delete(&target) == Ok(true) {
                        removed += 1;
                    }
                }
                println!("removed {removed} {RACE}* entries");
            }
            Some("worker") => worker(&args[1], args[2].parse().unwrap(), args[3].parse().unwrap()),
            _ => parent(&args),
        }
    }

    fn filetime_seconds(time: FILETIME) -> u64 {
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        // 100 ns ticks since 1601 to seconds since 1970.
        (ticks / 10_000_000).saturating_sub(11_644_473_600)
    }

    fn count() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let found = list("enodix-test-*").expect("list");
        let old = found
            .iter()
            .filter(|(_, _, written)| now.saturating_sub(filetime_seconds(*written)) > 3600)
            .count();
        println!(
            "{} enodix-test-* entries, {old} last written more than an hour ago",
            found.len()
        );
    }

    fn parent(args: &[String]) {
        let processes: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(8);
        let rounds: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(300);
        let filler: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(0);
        let exe = std::env::current_exe().unwrap();
        let prefixes: Vec<String> = (0..processes)
            .map(|_| format!("{RACE}{:016x}", rand::random::<u64>()))
            .collect();
        let _cleanups: Vec<Cleanup> = prefixes.iter().cloned().map(Cleanup).collect();
        let started = Instant::now();
        if std::env::var_os("ENODIX_RACE_THREADS").is_some() {
            // The same workers as threads of this one process.
            let threads: Vec<_> = prefixes
                .iter()
                .cloned()
                .map(|prefix| std::thread::spawn(move || worker(&prefix, rounds, filler)))
                .collect();
            for thread in threads {
                thread.join().unwrap();
            }
            println!(
                "{processes} threads x {rounds} rounds, {filler} filler entries each, {:.1} s",
                started.elapsed().as_secs_f64()
            );
            return;
        }
        let children: Vec<_> = prefixes
            .iter()
            .map(|prefix| {
                Command::new(&exe)
                    .args(["worker", prefix, &rounds.to_string(), &filler.to_string()])
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let mut totals: BTreeMap<String, usize> = BTreeMap::new();
        for child in children {
            let output = child.wait_with_output().unwrap();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(rest) = line.strip_prefix("count ") {
                    let (kind, n) = rest.rsplit_once(' ').unwrap();
                    *totals.entry(kind.to_owned()).or_default() += n.parse::<usize>().unwrap();
                } else {
                    println!("{line}");
                }
            }
        }
        println!(
            "{processes} processes x {rounds} rounds, {filler} filler entries each, {:.1} s",
            started.elapsed().as_secs_f64()
        );
        for (kind, n) in &totals {
            println!("  {kind}: {n}");
        }
    }

    struct Tally {
        counts: BTreeMap<&'static str, usize>,
        shown: usize,
        slowest: u128,
    }

    impl Tally {
        fn see(&mut self, kind: &'static str, detail: impl FnOnce() -> String) {
            *self.counts.entry(kind).or_default() += 1;
            if self.shown < 5 {
                self.shown += 1;
                println!("[{}] {kind}: {}", std::process::id(), detail());
            }
        }
    }

    fn worker(prefix: &str, rounds: usize, filler: usize) {
        assert!(prefix.starts_with(RACE));
        let _cleanup = Cleanup(prefix.to_owned());
        let mut tally = Tally {
            counts: BTreeMap::new(),
            shown: 0,
            slowest: 0,
        };
        for i in 0..filler {
            if let Err(code) = write(&format!("{prefix}:filler:{i}"), "filler") {
                tally.see("filler write failed", || format!("os error {code}"));
            }
        }
        let target = format!("{prefix}:entry");
        let filter = format!("{prefix}:entry*");
        let pause = Duration::from_millis(0);
        for round in 0..rounds {
            let first = format!("first-{round}");
            let second = format!("second-{round}");
            tally.see_ops(1);
            match write(&target, &first) {
                Ok(()) => {}
                Err(code) => tally.see("write failed", || format!("os error {code}")),
            }
            check_read(&mut tally, &target, Some(&first), "read after write");
            match write(&target, &second) {
                Ok(()) => {}
                Err(code) => tally.see("overwrite failed", || format!("os error {code}")),
            }
            let good = matches!(read(&target), Ok(Some(ref got)) if *got == second);
            if !good {
                tally.see("read after overwrite", || {
                    format!("expected {second:?}, got {:?}", read(&target))
                });
                if *SETTLE {
                    settle(&mut tally, &target, Some(&second));
                }
            } else if let Some(linger) = *LINGER {
                // Read back right, then read again later: does it stay?
                std::thread::sleep(linger);
                check_read(&mut tally, &target, Some(&second), "lost after a good read");
            }
            check_list(&mut tally, &filter, Some(&second), "list after overwrite");
            match delete(&target) {
                Ok(true) => {}
                Ok(false) => tally.see("delete found nothing", String::new),
                Err(code) => tally.see("delete failed", || format!("os error {code}")),
            }
            check_read(&mut tally, &target, None, "read after delete");
            check_list(&mut tally, &filter, None, "list after delete");
            std::thread::sleep(pause);
        }
        for (kind, n) in &tally.counts {
            println!("count {kind} {n}");
        }
        if tally.slowest > 0 {
            println!(
                "[{}] slowest to settle: {} ms",
                std::process::id(),
                tally.slowest
            );
        }
    }

    impl Tally {
        fn see_ops(&mut self, rounds: usize) {
            *self.counts.entry("rounds").or_default() += rounds;
        }
    }

    /// How long the store takes to show what was written after a read
    /// that did not: read again every 5 ms for up to 5 s.
    fn settle(tally: &mut Tally, target: &str, expected: Option<&str>) {
        let started = Instant::now();
        let mut last = None;
        while started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
            match read(target) {
                Ok(got) if got.as_deref() == expected => {
                    let ms = started.elapsed().as_millis();
                    let bucket = match ms {
                        0..=20 => "settled within 20 ms",
                        21..=100 => "settled within 100 ms",
                        101..=500 => "settled within 500 ms",
                        _ => "settled within 5 s",
                    };
                    *tally.counts.entry(bucket).or_default() += 1;
                    if ms > tally.slowest {
                        tally.slowest = ms;
                    }
                    return;
                }
                Ok(got) => last = got,
                Err(_) => {}
            }
        }
        tally.see("never settled in 5 s", || {
            format!("expected {expected:?}, still {last:?}")
        });
    }

    fn check_read(tally: &mut Tally, target: &str, expected: Option<&str>, what: &'static str) {
        match read(target) {
            Ok(got) if got.as_deref() == expected => {}
            Ok(got) => {
                tally.see(what, || format!("expected {expected:?}, got {got:?}"));
                if *SETTLE {
                    settle(tally, target, expected);
                }
            }
            Err(code) => tally.see(what, || format!("os error {code}")),
        }
    }

    /// Whether a read that missed is followed until it shows what was
    /// written (`ENODIX_RACE_SETTLE=1`).
    static SETTLE: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("ENODIX_RACE_SETTLE").is_some());

    /// How long after a right read-back it is read again
    /// (`ENODIX_RACE_LINGER_MS`).
    static LINGER: std::sync::LazyLock<Option<Duration>> = std::sync::LazyLock::new(|| {
        std::env::var("ENODIX_RACE_LINGER_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis)
    });

    fn check_list(tally: &mut Tally, filter: &str, expected: Option<&str>, what: &'static str) {
        match list(filter) {
            Ok(found) => {
                let got: Vec<&str> = found.iter().map(|(_, value, _)| value.as_str()).collect();
                let wanted: Vec<&str> = expected.into_iter().collect();
                if got != wanted {
                    tally.see(what, || format!("expected {wanted:?}, got {got:?}"));
                }
            }
            Err(code) => tally.see(what, || format!("os error {code}")),
        }
    }
}
