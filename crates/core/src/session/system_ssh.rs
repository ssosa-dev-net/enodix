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

/// The outdated algorithms the client is told it may use: the option, the
/// `ssh -Q` query that lists what the client knows for it, and the wanted
/// algorithms (`+` adds to the client's own list). OpenSSH refuses a whole
/// option, and exits before connecting, when it names one algorithm the
/// client no longer knows (Blowfish, RC4, CAST, DSA), so only what the
/// client lists is passed. `PubkeyAcceptedKeyTypes` is the name every
/// version takes (newer ones accept it as an alias of
/// `PubkeyAcceptedAlgorithms`, which versions before 8.5 lack); both option
/// kinds of key are listed by `-Q key`.
const LEGACY_OPTIONS: &[(&str, &str, &str)] = &[
    (
        "KexAlgorithms",
        "kex",
        "diffie-hellman-group1-sha1,diffie-hellman-group14-sha1,diffie-hellman-group-exchange-sha1",
    ),
    ("HostKeyAlgorithms", "key", "ssh-rsa,ssh-dss"),
    ("PubkeyAcceptedKeyTypes", "key", "ssh-rsa"),
    (
        "Ciphers",
        "cipher",
        "aes128-cbc,aes192-cbc,aes256-cbc,3des-cbc,blowfish-cbc,cast128-cbc,arcfour,arcfour128,arcfour256",
    ),
    (
        "MACs",
        "mac",
        "hmac-sha1,hmac-sha1-96,hmac-md5,hmac-md5-96,hmac-ripemd160",
    ),
];

/// What the installed client says it knows: the text of `ssh -Q cipher`,
/// `mac`, `kex` and `key`, one algorithm per line. `None` for what could not
/// be asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientAlgorithms {
    pub cipher: Option<String>,
    pub mac: Option<String>,
    pub kex: Option<String>,
    pub key: Option<String>,
}

impl ClientAlgorithms {
    fn known(&self, query: &str) -> Option<&str> {
        match query {
            "cipher" => self.cipher.as_deref(),
            "mac" => self.mac.as_deref(),
            "kex" => self.kex.as_deref(),
            _ => self.key.as_deref(),
        }
    }
}

/// The wanted algorithms (comma separated) that `known` (one per line, the
/// client's answer to `-Q`) lists, as the value of a `+` option. `None` when
/// nothing of them remains.
pub fn filter_known_algorithms(wanted: &str, known: &str) -> Option<String> {
    let kept: Vec<&str> = wanted
        .split(',')
        .filter(|name| known.lines().any(|line| line.trim() == *name))
        .collect();
    (!kept.is_empty()).then(|| format!("+{}", kept.join(",")))
}

/// The `-o` options (name, value) that name only what the client knows. An
/// option with nothing left, or whose list could not be asked, is left out.
pub fn legacy_options(client: &ClientAlgorithms) -> Vec<(&'static str, String)> {
    LEGACY_OPTIONS
        .iter()
        .filter_map(|(option, query, wanted)| {
            let value = filter_known_algorithms(wanted, client.known(query)?)?;
            Some((*option, value))
        })
        .collect()
}

/// Asks the client (`program`, else [`SYSTEM_SSH_PROGRAM`]) what it knows,
/// on a blocking thread, with a time limit. A client that cannot be run or
/// answers badly gives `None` for that list, so no legacy option is used
/// and the client's own message shows.
pub async fn ask_client_algorithms(program: Option<&str>) -> ClientAlgorithms {
    let program = program.unwrap_or(SYSTEM_SSH_PROGRAM).to_owned();
    let asking = tokio::task::spawn_blocking(move || ClientAlgorithms {
        cipher: query(&program, "cipher"),
        mac: query(&program, "mac"),
        kex: query(&program, "kex"),
        key: query(&program, "key"),
    });
    match tokio::time::timeout(std::time::Duration::from_secs(10), asking).await {
        Ok(Ok(answers)) => answers,
        _ => ClientAlgorithms::default(),
    }
}

fn query(program: &str, what: &str) -> Option<String> {
    let mut command = std::process::Command::new(program);
    command
        .args(["-Q", what])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

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
/// covered). `client` is what [`ask_client_algorithms`] found.
pub fn system_ssh_target(
    program: Option<&str>,
    host: &SystemSshHost,
    folder: Option<PathBuf>,
    environment: Vec<(String, String)>,
    client: &ClientAlgorithms,
) -> LocalTarget {
    let mut args = Vec::new();
    for (option, value) in legacy_options(client) {
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
