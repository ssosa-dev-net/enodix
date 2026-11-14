//! Which SSH algorithms sessions offer, the legacy-crypto switch, and what
//! a failed negotiation reports.
//!
//! With the switch off, a session offers only modern algorithms
//! ([`offered_algorithms`]`(false)`). With it on, it also offers, after the
//! modern ones so that a device that speaks both still gets a modern one,
//! the outdated algorithms old equipment needs and `russh` provides
//! ([`LEGACY_ALGORITHMS`]): SHA-1 key exchanges, the `ssh-rsa` (SHA-1) and
//! `ssh-dss` (DSA) host keys, CBC ciphers, 3DES and SHA-1 MACs.
//!
//! Support is best effort: `russh` is neither forked nor patched, so the
//! algorithms it lacks stay unsupported even with the switch on
//! ([`UNSUPPORTED_ALGORITHMS`]: MD5-based and truncated MACs, UMAC, RC4,
//! Blowfish, CAST, ...). A device that offers only those cannot be reached.

use std::fmt;

use russh::keys::{Algorithm, EcdsaCurve, HashAlg};
use russh::{cipher, kex, mac, Preferred};

/// One side's algorithms, per kind, as named on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlgorithmLists {
    pub key_exchange: Vec<String>,
    pub host_key: Vec<String>,
    pub cipher: Vec<String>,
    pub mac: Vec<String>,
}

/// A kind of algorithm the two sides of an SSH connection agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AlgorithmKind {
    KeyExchange,
    HostKey,
    Cipher,
    Mac,
}

impl fmt::Display for AlgorithmKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::KeyExchange => "key-exchange",
            Self::HostKey => "host-key",
            Self::Cipher => "cipher",
            Self::Mac => "MAC",
        })
    }
}

/// The algorithms the legacy-crypto switch adds to the offer, all of which
/// `russh` provides.
pub const LEGACY_ALGORITHMS: &[&str] = &[
    // Key exchange: SHA-1.
    "diffie-hellman-group14-sha1",
    "diffie-hellman-group-exchange-sha1",
    "diffie-hellman-group1-sha1",
    // Host keys: RSA signatures with SHA-1, and DSA.
    "ssh-rsa",
    "ssh-dss",
    // Ciphers: CBC mode, and 3DES.
    "aes256-cbc",
    "aes192-cbc",
    "aes128-cbc",
    "3des-cbc",
    // MACs: SHA-1.
    "hmac-sha1-etm@openssh.com",
    "hmac-sha1",
];

/// Outdated algorithms old devices may offer that stay unsupported even
/// with the legacy-crypto switch on, because `russh` does not provide them
/// (the list is what is known, not complete).
pub const UNSUPPORTED_ALGORITHMS: &[&str] = &[
    // MACs: MD5-based, truncated (96-bit) and UMAC.
    "hmac-md5",
    "hmac-md5-96",
    "hmac-md5-etm@openssh.com",
    "hmac-md5-96-etm@openssh.com",
    "hmac-sha1-96",
    "hmac-sha1-96-etm@openssh.com",
    "hmac-ripemd160",
    "hmac-ripemd160@openssh.com",
    "umac-64@openssh.com",
    "umac-128@openssh.com",
    "umac-64-etm@openssh.com",
    "umac-128-etm@openssh.com",
    // Ciphers: RC4, Blowfish, CAST, and the old Rijndael name.
    "arcfour",
    "arcfour128",
    "arcfour256",
    "blowfish-cbc",
    "cast128-cbc",
    "rijndael-cbc@lysator.liu.se",
    // Key exchange: GSS-API and RSA key exchange.
    "gss-group1-sha1-*",
    "gss-group14-sha1-*",
    "gss-gex-sha1-*",
    "rsa1024-sha1",
    // Host keys: X.509 certificates.
    "x509v3-sign-rsa",
    "x509v3-sign-dss",
];

/// Whether `name` is one of the outdated algorithms the legacy-crypto
/// switch adds.
pub fn is_legacy(name: &str) -> bool {
    LEGACY_ALGORITHMS.contains(&name)
}

/// The modern key exchanges, in order of preference.
const MODERN_KEX: &[kex::Name] = &[
    kex::MLKEM768X25519_SHA256,
    kex::CURVE25519,
    kex::CURVE25519_PRE_RFC_8731,
    kex::ECDH_SHA2_NISTP256,
    kex::ECDH_SHA2_NISTP384,
    kex::ECDH_SHA2_NISTP521,
    kex::DH_GEX_SHA256,
    kex::DH_G18_SHA512,
    kex::DH_G17_SHA512,
    kex::DH_G16_SHA512,
    kex::DH_G15_SHA512,
    kex::DH_G14_SHA256,
];

const LEGACY_KEX: &[kex::Name] = &[kex::DH_G14_SHA1, kex::DH_GEX_SHA1, kex::DH_G1_SHA1];

/// Not algorithms: what the client supports (extension negotiation,
/// OpenSSH's strict key exchange). Always offered, after the key exchanges.
const KEX_EXTENSIONS: &[kex::Name] = &[
    kex::EXTENSION_SUPPORT_AS_CLIENT,
    kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT,
];

const MODERN_HOST_KEYS: &[Algorithm] = &[
    Algorithm::Ed25519,
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP256,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP384,
    },
    Algorithm::Ecdsa {
        curve: EcdsaCurve::NistP521,
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha512),
    },
    Algorithm::Rsa {
        hash: Some(HashAlg::Sha256),
    },
];

const LEGACY_HOST_KEYS: &[Algorithm] = &[Algorithm::Rsa { hash: None }, Algorithm::Dsa];

const MODERN_CIPHERS: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    cipher::AES_128_GCM,
    cipher::AES_256_CTR,
    cipher::AES_192_CTR,
    cipher::AES_128_CTR,
];

const LEGACY_CIPHERS: &[cipher::Name] = &[
    cipher::AES_256_CBC,
    cipher::AES_192_CBC,
    cipher::AES_128_CBC,
    cipher::TRIPLE_DES_CBC,
];

/// Ciphers that authenticate as they encrypt, so they need no MAC.
const AEAD_CIPHERS: &[cipher::Name] = &[
    cipher::CHACHA20_POLY1305,
    cipher::AES_256_GCM,
    cipher::AES_128_GCM,
];

const MODERN_MACS: &[mac::Name] = &[
    mac::HMAC_SHA512_ETM,
    mac::HMAC_SHA256_ETM,
    mac::HMAC_SHA512,
    mac::HMAC_SHA256,
];

const LEGACY_MACS: &[mac::Name] = &[mac::HMAC_SHA1_ETM, mac::HMAC_SHA1];

/// What a session offers to the device, legacy algorithms included or not.
pub(super) fn preferred(legacy_crypto: bool) -> Preferred {
    fn list<T: Clone>(modern: &[T], legacy: &[T], legacy_crypto: bool) -> Vec<T> {
        let mut all = modern.to_vec();
        if legacy_crypto {
            all.extend_from_slice(legacy);
        }
        all
    }
    let mut kex = list(MODERN_KEX, LEGACY_KEX, legacy_crypto);
    kex.extend_from_slice(KEX_EXTENSIONS);
    Preferred {
        kex: kex.into(),
        key: list(MODERN_HOST_KEYS, LEGACY_HOST_KEYS, legacy_crypto).into(),
        cipher: list(MODERN_CIPHERS, LEGACY_CIPHERS, legacy_crypto).into(),
        mac: list(MODERN_MACS, LEGACY_MACS, legacy_crypto).into(),
        ..Preferred::default()
    }
}

/// The algorithms a session offers to the device, in order of preference:
/// modern ones only with the legacy-crypto switch off, and then the
/// [`LEGACY_ALGORITHMS`] with it on.
pub fn offered_algorithms(legacy_crypto: bool) -> AlgorithmLists {
    let preferred = preferred(legacy_crypto);
    AlgorithmLists {
        key_exchange: preferred
            .kex
            .iter()
            .filter(|name| !KEX_EXTENSIONS.contains(name))
            .map(|name| name.as_ref().to_owned())
            .collect(),
        host_key: preferred.key.iter().map(ToString::to_string).collect(),
        cipher: preferred
            .cipher
            .iter()
            .map(|name| name.as_ref().to_owned())
            .collect(),
        mac: preferred
            .mac
            .iter()
            .map(|name| name.as_ref().to_owned())
            .collect(),
    }
}

/// The first kind of algorithm `ours` and `theirs` have nothing in common
/// for, choosing as SSH does; `None` if they can agree on everything. A MAC
/// is needed only if the cipher agreed on does not authenticate by itself.
///
/// An empty list of theirs counts as unknown, not as a mismatch: when the
/// device's offer could not be read, only the list that failed is known.
fn first_mismatch(ours: &AlgorithmLists, theirs: &AlgorithmLists) -> Option<AlgorithmKind> {
    fn common<'a>(ours: &'a [String], theirs: &[String]) -> Option<&'a str> {
        if theirs.is_empty() {
            return Some("");
        }
        ours.iter()
            .find(|name| theirs.contains(name))
            .map(String::as_str)
    }
    if common(&ours.key_exchange, &theirs.key_exchange).is_none() {
        return Some(AlgorithmKind::KeyExchange);
    }
    if common(&ours.host_key, &theirs.host_key).is_none() {
        return Some(AlgorithmKind::HostKey);
    }
    let Some(cipher) = common(&ours.cipher, &theirs.cipher) else {
        return Some(AlgorithmKind::Cipher);
    };
    let needs_mac = !AEAD_CIPHERS.iter().any(|aead| aead.as_ref() == cipher);
    if needs_mac && common(&ours.mac, &theirs.mac).is_none() {
        return Some(AlgorithmKind::Mac);
    }
    None
}

/// The client and the device had no algorithm in common for some kind, so
/// the connection could not be set up. Says what the device offered and
/// whether the legacy-crypto switch would help.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiationFailure {
    /// The first kind of algorithm nothing was in common for.
    pub kind: AlgorithmKind,
    /// What the device offered (its client-to-server lists for cipher and
    /// MAC).
    pub offered: AlgorithmLists,
    /// Whether the session had the legacy-crypto switch on.
    pub legacy_crypto: bool,
}

impl NegotiationFailure {
    pub(super) fn new(kind: AlgorithmKind, offered: AlgorithmLists, legacy_crypto: bool) -> Self {
        Self {
            kind,
            offered,
            legacy_crypto,
        }
    }

    /// The kind of algorithm nothing would be in common for even with the
    /// legacy-crypto switch on (the device offers only algorithms that are
    /// not supported); `None` if the switch makes every kind work.
    pub fn unsupported_even_with_legacy_crypto(&self) -> Option<AlgorithmKind> {
        first_mismatch(&offered_algorithms(true), &self.offered)
    }

    /// Whether turning the legacy-crypto switch on would let the session
    /// agree with the device.
    pub fn legacy_crypto_would_help(&self) -> bool {
        !self.legacy_crypto && self.unsupported_even_with_legacy_crypto().is_none()
    }
}

impl fmt::Display for NegotiationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no {} algorithm in common with the device", self.kind)?;
        if self.legacy_crypto_would_help() {
            f.write_str("; turning on legacy crypto for this host would help")?;
        } else if let Some(kind) = self.unsupported_even_with_legacy_crypto() {
            if self.legacy_crypto {
                write!(f, ", even with legacy crypto on")?;
            } else {
                write!(f, "; legacy crypto would not help")?;
            }
            write!(f, " (none of the device's {kind} algorithms is supported)")?;
        }
        let list = |names: &[String]| {
            if names.is_empty() {
                "(unknown)".to_owned()
            } else {
                names.join(", ")
            }
        };
        write!(
            f,
            ". The device offered key exchange: {}; host key: {}; cipher: {}; MAC: {}",
            list(&self.offered.key_exchange),
            list(&self.offered.host_key),
            list(&self.offered.cipher),
            list(&self.offered.mac),
        )
    }
}

/// The algorithms one connection of a session agreed on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiatedAlgorithms {
    /// The jump host (its [`super::JumpHost::name`]) this connection went
    /// to; `None` for the device itself.
    pub jump_host: Option<String>,
    pub key_exchange: String,
    pub host_key: String,
    pub cipher: String,
    /// The MAC, or `none` with a cipher that needs none. When the two
    /// directions differ, both, client-to-server first.
    pub mac: String,
    /// The outdated algorithms among these ([`LEGACY_ALGORITHMS`]); empty
    /// when the connection is modern throughout.
    pub legacy: Vec<String>,
}

impl NegotiatedAlgorithms {
    pub(super) fn from_names(jump_host: Option<String>, names: &russh::Names) -> Self {
        let client_mac = names.client_mac.as_ref();
        let server_mac = names.server_mac.as_ref();
        let mac = if client_mac == server_mac {
            client_mac.to_owned()
        } else {
            format!("{client_mac}, {server_mac}")
        };
        let host_key = names.key.as_str();
        let mut legacy: Vec<String> = Vec::new();
        for name in [
            names.kex.as_ref(),
            host_key,
            names.cipher.as_ref(),
            client_mac,
            server_mac,
        ] {
            if is_legacy(name) && !legacy.iter().any(|known| known == name) {
                legacy.push(name.to_owned());
            }
        }
        Self {
            jump_host,
            key_exchange: names.kex.as_ref().to_owned(),
            host_key: host_key.to_owned(),
            cipher: names.cipher.as_ref().to_owned(),
            mac,
            legacy,
        }
    }
}

/// The device's offer, read from the start of what it sent: its version
/// line, then its key-exchange init. `None` if that is not all there.
pub(super) fn parse_offer(bytes: &[u8]) -> Option<AlgorithmLists> {
    // Lines before the version line are allowed; the version line ends the
    // text part.
    let mut rest = bytes;
    loop {
        let end = rest.iter().position(|&b| b == b'\n')?;
        let line = &rest[..end];
        rest = &rest[end + 1..];
        if line.starts_with(b"SSH-") {
            break;
        }
    }
    let length = usize::try_from(u32::from_be_bytes(rest.get(..4)?.try_into().ok()?)).ok()?;
    let packet = rest.get(4..4 + length)?;
    let padding = usize::from(*packet.first()?);
    let payload = packet.get(1..length.checked_sub(padding)?)?;
    const KEXINIT: u8 = 20;
    if payload.first() != Some(&KEXINIT) {
        return None;
    }
    // After the message number, a 16-byte cookie, then the name-lists.
    let mut lists = payload.get(17..)?;
    let mut next = || -> Option<Vec<String>> {
        let length = usize::try_from(u32::from_be_bytes(lists.get(..4)?.try_into().ok()?)).ok()?;
        let names = std::str::from_utf8(lists.get(4..4 + length)?).ok()?;
        lists = &lists[4 + length..];
        Some(
            names
                .split(',')
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    };
    let key_exchange = next()?
        .into_iter()
        // Not algorithms: what the server supports.
        .filter(|name| !name.starts_with("ext-info-") && !name.starts_with("kex-strict-"))
        .collect();
    let host_key = next()?;
    // The client-to-server lists: russh agrees on the cipher from that
    // one, and devices offer the same in both directions in practice.
    let cipher = next()?;
    let _cipher_from_server = next()?;
    let mac = next()?;
    Some(AlgorithmLists {
        key_exchange,
        host_key,
        cipher,
        mac,
    })
}
