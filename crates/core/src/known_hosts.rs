//! Known host keys: the SSH host keys the user has trusted, per host and
//! port. They live in the per-user application data folder (never in a
//! workspace), in a file named [`FILE_NAME`] in OpenSSH's `known_hosts`
//! format, so other tools can read it.
//!
//! The file is read again on every check, so keys trusted by another window
//! (or edited by hand) count at once. Lines this module does not understand
//! (comments, plain entries under hashed host names) are ignored and kept
//! as they are when it writes the file. The marker lines are read but never
//! written by the app: `@cert-authority` lines by
//! [`KnownHosts::certificate_authorities`] (the authorities whose host
//! certificates are trusted) and `@revoked` lines by [`KnownHosts::revoked`]
//! (keys never to accept). Their host patterns may be hashed, as
//! `ssh-keygen -H` writes them. An entry for the host whose key cannot be
//! read fails the check rather than counting as no key.
//!
//! Any stored key differing from the presented one counts as a change, even
//! of another key type (OpenSSH compares within a type): russh's host-key
//! algorithm preference is fixed, so a type switch is itself suspicious.
//! Writes are serialised within one process only; two app instances
//! trusting keys at the same moment can lose one of them (known limit).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use russh::keys::{HashAlg, PublicKey};
use serde::Serialize;

/// The known-hosts file's name inside the data folder.
pub const FILE_NAME: &str = "known_hosts";

/// The port at which OpenSSH writes a host by its bare name.
const SSH_PORT: u16 = 22;

/// A host key as the user sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostKey {
    /// The key type, such as `ssh-ed25519` or `ssh-rsa`.
    pub algorithm: String,
    /// The SHA-256 fingerprint as OpenSSH shows it: `SHA256:` and base64.
    pub fingerprint: String,
}

impl HostKey {
    pub fn of(key: &PublicKey) -> Self {
        Self {
            algorithm: key.algorithm().as_str().to_owned(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        }
    }
}

/// What the known keys say about the key a server presents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// It is a key trusted for this host and port.
    Trusted,
    /// No key is known for this host and port.
    Unknown,
    /// Other keys are known for this host and port: the key has changed.
    Changed { stored: Vec<HostKey> },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KnownHostsError {
    #[error("cannot read the known host keys in {file}: {reason}")]
    Read { file: String, reason: String },
    #[error("cannot save the host key to {file}: {reason}")]
    Write { file: String, reason: String },
}

/// The trusted host keys. Cloning gives another handle to the same keys.
#[derive(Clone)]
pub struct KnownHosts {
    store: Arc<Mutex<Store>>,
}

enum Store {
    File(PathBuf),
    Memory(Vec<Line>),
}

impl KnownHosts {
    /// The keys kept in `data_folder` (the per-user application data
    /// folder). The file is created when the first key is remembered.
    pub fn in_data_folder(data_folder: &Path) -> Self {
        Self::with(Store::File(data_folder.join(FILE_NAME)))
    }

    /// Keys kept only as long as this value (and its clones) lives.
    pub fn in_memory() -> Self {
        Self::with(Store::Memory(Vec::new()))
    }

    fn with(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Whether `key` is trusted for `host` (name or address, compared
    /// ignoring case) at `port`. A file that exists but cannot be read is an
    /// error, never "unknown": that would hide a changed key.
    pub fn check(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
    ) -> Result<HostKeyStatus, KnownHostsError> {
        let store = self.lock();
        let lines = store.load()?;
        let pattern = pattern(host, port);
        let mut stored: Vec<&PublicKey> = Vec::new();
        for line in &lines {
            match line {
                Line::Entry { patterns, key, .. } if matches_exactly(patterns, &pattern) => {
                    // A damaged entry must not make a changed key look new.
                    let key = key.as_ref().ok_or_else(|| {
                        store.read_error(format!("the key stored for {pattern} cannot be read"))
                    })?;
                    stored.push(key);
                }
                _ => {}
            }
        }
        Ok(if stored.iter().any(|s| s.key_data() == key.key_data()) {
            HostKeyStatus::Trusted
        } else if stored.is_empty() {
            HostKeyStatus::Unknown
        } else {
            HostKeyStatus::Changed {
                stored: stored.into_iter().map(HostKey::of).collect(),
            }
        })
    }

    /// The keys trusted for `host` at `port`, in file order: none if the
    /// host is unknown. A damaged entry for them is an error, as in
    /// [`Self::check`].
    pub fn trusted(&self, host: &str, port: u16) -> Result<Vec<PublicKey>, KnownHostsError> {
        let store = self.lock();
        let lines = store.load()?;
        let pattern = pattern(host, port);
        let mut trusted = Vec::new();
        for line in &lines {
            if let Line::Entry { patterns, key, .. } = line {
                if matches_exactly(patterns, &pattern) {
                    let key = key.as_ref().ok_or_else(|| {
                        store.read_error(format!("the key stored for {pattern} cannot be read"))
                    })?;
                    trusted.push(key.clone());
                }
            }
        }
        Ok(trusted)
    }

    /// The certificate authorities trusted for `host` at `port`: the keys of
    /// the `@cert-authority` lines whose patterns match it, in file order.
    /// Patterns are read as OpenSSH reads them: `*` and `?` wildcards,
    /// ignoring case, a matching `!` pattern ruling the line out; a host at
    /// another port than 22 is matched as `[host]:port`. A damaged key on a
    /// matching line is an error, as in [`Self::check`].
    pub fn certificate_authorities(
        &self,
        host: &str,
        port: u16,
    ) -> Result<Vec<PublicKey>, KnownHostsError> {
        self.marked(CERT_AUTHORITY, "certificate authority", host, port)
    }

    /// The keys revoked for `host` at `port`: the keys of the `@revoked`
    /// lines whose patterns match it, read as in
    /// [`Self::certificate_authorities`]. A revoked key is refused whether
    /// it is the host's key, the key inside its host certificate, or the
    /// authority that signed the certificate, as OpenSSH refuses it.
    pub fn revoked(&self, host: &str, port: u16) -> Result<Vec<PublicKey>, KnownHostsError> {
        self.marked(REVOKED, "revoked", host, port)
    }

    /// The keys of the lines with `marker` whose patterns match `host` at
    /// `port`, in file order; `what` names them in an error.
    fn marked(
        &self,
        marker: &str,
        what: &str,
        host: &str,
        port: u16,
    ) -> Result<Vec<PublicKey>, KnownHostsError> {
        let store = self.lock();
        let lines = store.load()?;
        let name = pattern(host, port);
        let mut keys = Vec::new();
        for line in &lines {
            let Line::Other(text) = line else { continue };
            let Some((patterns, key)) = marker_line(text, marker) else {
                continue;
            };
            if matches_patterns(patterns, &name) {
                let key = key.parse::<PublicKey>().map_err(|_| {
                    store.read_error(format!("the {what} key for {name} cannot be read"))
                })?;
                keys.push(key);
            }
        }
        Ok(keys)
    }

    /// Trusts `key` for `host` at `port` from now on, replacing any key
    /// stored for them.
    pub fn remember(&self, host: &str, port: u16, key: &PublicKey) -> Result<(), KnownHostsError> {
        let mut store = self.lock();
        // A name the file would read as more than one plain host (a comma
        // separates hosts, whitespace ends the field, a wildcard or `!`
        // matches others, a leading `#`, `@` or `|` is a comment, a marker
        // or a hashed name): trusting a key for `core1,x` would trust it
        // for `core1` too. A shared workspace can name a host so.
        if host.is_empty()
            || host
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || ",*?!".contains(c))
            || host.starts_with(['#', '@', '|'])
        {
            return Err(store.write_error(format!(
                "{host:?} is not a host name or address that can be remembered"
            )));
        }
        let mut lines = store.load()?;
        let pattern = pattern(host, port);
        lines.retain_mut(|line| match line {
            Line::Entry { patterns, .. } => {
                patterns.retain(|p| !p.eq_ignore_ascii_case(&pattern));
                !patterns.is_empty()
            }
            Line::Other(_) => true,
        });
        let key_text = key.to_openssh().map_err(|error| store.write_error(error))?;
        lines.push(Line::Entry {
            patterns: vec![pattern],
            key: Some(key.clone()),
            key_text,
        });
        store.save(lines)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One line of the file.
#[derive(Clone)]
enum Line {
    /// Host patterns (comma-separated in the file) and their key, `None`
    /// if the key cannot be read (damaged, or a format not supported).
    Entry {
        patterns: Vec<String>,
        key: Option<PublicKey>,
        /// Everything after the patterns, as it was written.
        key_text: String,
    },
    /// Anything else, kept exactly.
    Other(String),
}

impl Line {
    fn parse(text: &str) -> Self {
        let other = || Self::Other(text.to_owned());
        let trimmed = text.trim();
        let Some((patterns, rest)) = trimmed.split_once(char::is_whitespace) else {
            return other();
        };
        // Comments, markers (`@revoked`, `@cert-authority`) and hashed names
        // never match a host here.
        if patterns.starts_with(['#', '@', '|']) {
            return other();
        }
        let key_text = rest.trim();
        Self::Entry {
            patterns: patterns.split(',').map(str::to_owned).collect(),
            key: key_text.parse::<PublicKey>().ok(),
            key_text: key_text.to_owned(),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Entry {
                patterns, key_text, ..
            } => format!("{} {key_text}", patterns.join(",")),
            Self::Other(text) => text.clone(),
        }
    }
}

impl Store {
    fn load(&self) -> Result<Vec<Line>, KnownHostsError> {
        match self {
            Self::Memory(lines) => Ok(lines.clone()),
            Self::File(file) => match fs::read_to_string(file) {
                Ok(text) => Ok(text.lines().map(Line::parse).collect()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(error) => Err(self.read_error(error)),
            },
        }
    }

    fn save(&mut self, lines: Vec<Line>) -> Result<(), KnownHostsError> {
        match self {
            Self::Memory(stored) => {
                *stored = lines;
                Ok(())
            }
            Self::File(file) => {
                let text: String = lines.iter().map(|line| line.text() + "\n").collect();
                write_atomically(file, &text).map_err(|error| self.write_error(error))
            }
        }
    }

    fn read_error(&self, error: impl std::fmt::Display) -> KnownHostsError {
        KnownHostsError::Read {
            file: self.name(),
            reason: error.to_string(),
        }
    }

    fn write_error(&self, error: impl std::fmt::Display) -> KnownHostsError {
        KnownHostsError::Write {
            file: self.name(),
            reason: error.to_string(),
        }
    }

    fn name(&self) -> String {
        match self {
            Self::File(file) => file.display().to_string(),
            Self::Memory(_) => "memory".into(),
        }
    }
}

/// Writes `text` to a temporary file beside `file`, then puts it in place,
/// so that a crash never leaves half a file. Creates the folder if needed.
fn write_atomically(file: &Path, text: &str) -> io::Result<()> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    // Named per process, so that two app instances never share it.
    let mut temporary = file.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text)?;
    fs::rename(&temporary, file)
}

/// How OpenSSH names a host and port in the file: the bare name for port
/// 22, else `[name]:port`. Names are kept lower-case.
fn pattern(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == SSH_PORT {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// Whether a line's patterns name exactly `pattern`. Wildcards and
/// negations are not interpreted: they never match.
fn matches_exactly(patterns: &[String], pattern: &str) -> bool {
    patterns.iter().any(|p| p.eq_ignore_ascii_case(pattern))
}

/// The marker of the lines naming certificate authorities.
const CERT_AUTHORITY: &str = "@cert-authority";
/// The marker of the lines naming revoked keys.
const REVOKED: &str = "@revoked";

/// The patterns and key of a line with `marker`, `None` for any other line.
fn marker_line<'a>(text: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let rest = text.trim().strip_prefix(marker)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let (patterns, key) = rest.trim_start().split_once(char::is_whitespace)?;
    Some((patterns, key.trim()))
}

/// Whether comma-separated OpenSSH host `patterns` match `name`: one
/// pattern matches and no `!` pattern does. A hashed pattern
/// (`|1|salt|hash`) stands for one name, compared without regard to case.
fn matches_patterns(patterns: &str, name: &str) -> bool {
    let mut matched = false;
    for pattern in patterns.split(',') {
        match pattern.strip_prefix('!') {
            Some(negated) if matches_one(negated, name) => return false,
            Some(_) => {}
            None => matched |= matches_one(pattern, name),
        }
    }
    matched
}

/// Whether one pattern, plain or hashed, matches `name`.
fn matches_one(pattern: &str, name: &str) -> bool {
    match pattern.strip_prefix("|1|") {
        Some(hashed) => matches_hashed(hashed, name),
        None => wildcard(pattern, name),
    }
}

/// Whether `salt|hash` (both base64) is the HMAC-SHA1 of `name` under
/// `salt`, as OpenSSH hashes the names of its known hosts. A hash that
/// cannot be read matches nothing.
fn matches_hashed(hashed: &str, name: &str) -> bool {
    use base64ct::{Base64, Encoding};
    use hmac::{KeyInit, Mac};
    let Some((salt, hash)) = hashed.split_once('|') else {
        return false;
    };
    let (Ok(salt), Ok(hash)) = (Base64::decode_vec(salt), Base64::decode_vec(hash)) else {
        return false;
    };
    let Ok(mut mac) = hmac::Hmac::<sha1::Sha1>::new_from_slice(&salt) else {
        return false;
    };
    // OpenSSH hashes the name as it was given, lower-cased.
    mac.update(name.to_ascii_lowercase().as_bytes());
    mac.verify_slice(&hash).is_ok()
}

/// `*` and `?` wildcard matching, ignoring ASCII case.
fn wildcard(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let name: Vec<char> = name.to_ascii_lowercase().chars().collect();
    let (mut p, mut n) = (0, 0);
    // Where the last `*` was, and the name position it stood for.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, n));
            p += 1;
        } else if let Some((star_p, star_n)) = star {
            p = star_p + 1;
            n = star_n + 1;
            star = Some((star_p, star_n + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_as_openssh_reads_them() {
        assert!(matches_patterns("*.lab", "r1.lab"));
        assert!(matches_patterns("R?.LAB", "r1.lab"));
        assert!(matches_patterns("*.lab,!r2.lab", "r1.lab"));
        assert!(!matches_patterns("*.lab,!r2.lab", "r2.lab"));
        assert!(!matches_patterns("!r2.lab", "r1.lab"));
        assert!(!matches_patterns("*.lab", "[r1.lab]:2222"));
        assert!(matches_patterns("[*.lab]:2222", "[r1.lab]:2222"));
        assert!(matches_patterns("*", "anything"));
        assert!(!matches_patterns("r1", "r10"));
    }

    #[test]
    fn only_lines_with_the_marker_are_read() {
        let authority = |text| marker_line(text, CERT_AUTHORITY);
        assert_eq!(
            authority("@cert-authority *.lab ssh-ed25519 AAAA ca"),
            Some(("*.lab", "ssh-ed25519 AAAA ca"))
        );
        assert_eq!(authority("@revoked r1 ssh-ed25519 AAAA"), None);
        assert_eq!(authority("@cert-authorityx r1 ssh-ed25519 AAAA"), None);
        assert_eq!(authority("# @cert-authority r1 ssh-ed25519 AAAA"), None);
        assert_eq!(
            marker_line("@revoked * ssh-ed25519 AAAA", REVOKED),
            Some(("*", "ssh-ed25519 AAAA"))
        );
    }

    /// `r1.lab` hashed as `ssh-keygen -H` hashes it, with the salt 1 to 20
    /// (HMAC-SHA1, both base64).
    const HASHED_R1: &str = "|1|AQIDBAUGBwgJCgsMDQ4PEBESExQ=|wej/4NCvhF38Eq06V2Ja/+XAtxQ=";

    #[test]
    fn hashed_patterns_match_the_one_name_they_hash() {
        assert!(matches_patterns(HASHED_R1, "r1.lab"));
        assert!(matches_patterns(HASHED_R1, "R1.LAB"));
        assert!(!matches_patterns(HASHED_R1, "r2.lab"));
        assert!(!matches_patterns(&format!("*.lab,!{HASHED_R1}"), "r1.lab"));
        assert!(matches_patterns(&format!("*.lab,!{HASHED_R1}"), "r2.lab"));
        assert!(!matches_patterns("|1|not base64|x", "r1.lab"));
        assert!(!matches_patterns("|1|nohash", "r1.lab"));
    }
}
