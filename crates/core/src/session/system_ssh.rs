//! The way out for a device the SSH library cannot negotiate with (#27):
//! the system's own OpenSSH client, run in a local pseudo terminal.
//!
//! This is not an SSH session of enodix. It does not check the host key
//! against the app's, apply the host's other settings or log in with the
//! app's credentials; the user types the password in the terminal. It
//! exists because MD5, RC4, Blowfish and SSH-1 are broken and the project
//! neither implements nor forks them, so the one honest option for a device
//! that offers nothing else is a client that is somebody else's to
//! maintain, chosen on purpose and plainly marked as insecure.

use std::path::PathBuf;

use super::LocalTarget;

/// The OpenSSH client's name; a path works too.
pub const SYSTEM_SSH_PROGRAM: &str = "ssh";

/// What the interface says next to a session started this way.
pub const SYSTEM_SSH_WARNING: &str =
    "Insecure: the system's ssh client with outdated algorithms. Not checked or logged by the app.";

/// The outdated algorithms the client is told it may use, as `-o` options
/// (`+` adds to the client's own list). A newer client no longer knows some
/// of them (Blowfish, RC4, CAST): it then refuses the option and says so.
const LEGACY_OPTIONS: &[(&str, &str)] = &[
    (
        "KexAlgorithms",
        "+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1,diffie-hellman-group-exchange-sha1",
    ),
    ("HostKeyAlgorithms", "+ssh-rsa,ssh-dss"),
    ("PubkeyAcceptedAlgorithms", "+ssh-rsa"),
    (
        "Ciphers",
        "+aes128-cbc,aes192-cbc,aes256-cbc,3des-cbc,blowfish-cbc,cast128-cbc,arcfour,arcfour128,arcfour256",
    ),
    (
        "MACs",
        "+hmac-sha1,hmac-sha1-96,hmac-md5,hmac-md5-96,hmac-ripemd160",
    ),
];

/// A device to reach with the system's ssh client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSshHost {
    pub host: String,
    pub port: u16,
    /// `None`: the client's own choice (its configuration, else the user
    /// name of this computer).
    pub username: Option<String>,
}

/// The local program that runs the system's ssh client for `host`, with the
/// outdated algorithms allowed. `program` is [`SYSTEM_SSH_PROGRAM`] unless
/// the user points at another client (`plink` takes other options and is not
/// covered).
pub fn system_ssh_target(
    program: Option<&str>,
    host: &SystemSshHost,
    folder: Option<PathBuf>,
    environment: Vec<(String, String)>,
) -> LocalTarget {
    let mut args = Vec::new();
    for (option, value) in LEGACY_OPTIONS {
        args.push("-o".to_owned());
        args.push(format!("{option}={value}"));
    }
    args.push("-p".into());
    args.push(host.port.to_string());
    if let Some(username) = host.username.as_deref().filter(|name| !name.is_empty()) {
        args.push("-l".into());
        args.push(username.into());
    }
    // `--` so that a host name never reads as an option.
    args.push("--".into());
    args.push(host.host.clone());
    LocalTarget {
        program: program.unwrap_or(SYSTEM_SSH_PROGRAM).into(),
        args,
        folder,
        environment,
    }
}
