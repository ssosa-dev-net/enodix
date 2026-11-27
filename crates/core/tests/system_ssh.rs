//! The system ssh fallback (#27): the command it starts for a device the SSH
//! library cannot negotiate with. On Unix a stand-in script prints the
//! arguments it gets, so that what the real client would be told is observed
//! through a real local session. Where an `ssh` is installed, the real client
//! also reads the arguments (`-G`), which is what catches an option it refuses.

#[cfg(unix)]
mod support;

use enodix_core::session::{
    ask_client_algorithms, filter_known_algorithms, legacy_options, system_ssh_target,
    ClientAlgorithms, SystemSshHost, SYSTEM_SSH_PROGRAM,
};

const OLD_CIPHERS: &str = "3des-cbc\nblowfish-cbc\ncast128-cbc\narcfour\narcfour128\narcfour256\naes128-cbc\naes192-cbc\naes256-cbc\nrijndael-cbc@lysator.liu.se\naes128-ctr\naes192-ctr\naes256-ctr\naes128-gcm@openssh.com\naes256-gcm@openssh.com\nchacha20-poly1305@openssh.com\n";
const CURRENT_CIPHERS: &str = "3des-cbc\naes128-cbc\naes192-cbc\naes256-cbc\naes128-ctr\naes192-ctr\naes256-ctr\naes128-gcm@openssh.com\naes256-gcm@openssh.com\nchacha20-poly1305@openssh.com\n";

/// `ssh -Q cipher|mac|kex|key` of OpenSSH 7.4 (the release of CentOS 7),
/// written from that release's lists (no such client was at hand): the old
/// algorithms are all there.
fn old_client() -> ClientAlgorithms {
    ClientAlgorithms {
        cipher: Some(OLD_CIPHERS.into()),
        mac: Some("hmac-sha1\nhmac-sha1-96\nhmac-sha2-256\nhmac-sha2-512\nhmac-md5\nhmac-md5-96\nhmac-ripemd160\numac-64@openssh.com\n".into()),
        kex: Some("diffie-hellman-group1-sha1\ndiffie-hellman-group14-sha1\ndiffie-hellman-group-exchange-sha1\ndiffie-hellman-group-exchange-sha256\ncurve25519-sha256@libssh.org\n".into()),
        key: Some("ssh-ed25519\necdsa-sha2-nistp256\nssh-dss\nssh-rsa\n".into()),
    }
}

/// The same four queries as `OpenSSH_for_Windows_9.5p2` printed them
/// (abridged to the algorithms that matter here): no Blowfish, CAST, RC4 or
/// RIPEMD any more.
fn current_client() -> ClientAlgorithms {
    ClientAlgorithms {
        cipher: Some(CURRENT_CIPHERS.into()),
        mac: Some("hmac-sha1\nhmac-sha1-96\nhmac-sha2-256\nhmac-sha2-512\nhmac-md5\nhmac-md5-96\numac-64@openssh.com\n".into()),
        kex: Some("diffie-hellman-group1-sha1\ndiffie-hellman-group14-sha1\ndiffie-hellman-group14-sha256\ndiffie-hellman-group-exchange-sha1\ndiffie-hellman-group-exchange-sha256\ncurve25519-sha256\n".into()),
        key: Some("ssh-ed25519\necdsa-sha2-nistp256\nssh-dss\nssh-rsa\n".into()),
    }
}

/// `OpenSSH_10.6p1`: DSA is gone from the key types.
fn newest_client() -> ClientAlgorithms {
    ClientAlgorithms {
        key: Some("ssh-ed25519\necdsa-sha2-nistp256\nssh-rsa\n".into()),
        ..current_client()
    }
}

fn host(username: Option<&str>) -> SystemSshHost {
    SystemSshHost {
        host: "10.0.0.7".into(),
        port: 2222,
        username: username.map(Into::into),
    }
}

fn value<'a>(options: &'a [(&str, String)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find(|(option, _)| *option == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn an_old_client_is_allowed_everything_it_knows() {
    let options = legacy_options(&old_client());
    assert_eq!(options.len(), 5);
    assert!(value(&options, "Ciphers")
        .unwrap()
        .contains("blowfish-cbc,cast128-cbc,arcfour,arcfour128,arcfour256"));
    assert!(value(&options, "MACs").unwrap().contains("hmac-ripemd160"));
    assert_eq!(
        value(&options, "HostKeyAlgorithms"),
        Some("+ssh-rsa,ssh-dss")
    );
}

#[test]
fn a_current_client_is_told_only_what_it_lists() {
    let options = legacy_options(&current_client());
    assert_eq!(
        value(&options, "Ciphers"),
        Some("+aes128-cbc,aes192-cbc,aes256-cbc,3des-cbc")
    );
    assert_eq!(
        value(&options, "MACs"),
        Some("+hmac-sha1,hmac-sha1-96,hmac-md5,hmac-md5-96")
    );
    let newest = legacy_options(&newest_client());
    assert_eq!(value(&newest, "HostKeyAlgorithms"), Some("+ssh-rsa"));
}

#[test]
fn an_option_with_nothing_left_is_left_out() {
    let client = ClientAlgorithms {
        cipher: Some("aes128-ctr\nchacha20-poly1305@openssh.com\n".into()),
        ..current_client()
    };
    let options = legacy_options(&client);
    assert!(value(&options, "Ciphers").is_none());
    assert!(value(&options, "MACs").is_some());
    assert_eq!(filter_known_algorithms("arcfour", "aes128-ctr\n"), None);
}

#[test]
fn a_client_that_could_not_be_asked_gets_no_legacy_option() {
    assert!(legacy_options(&ClientAlgorithms::default()).is_empty());
    let target = system_ssh_target(
        None,
        &host(Some("admin")),
        None,
        Vec::new(),
        &ClientAlgorithms::default(),
    );
    assert!(!target.args.iter().any(|arg| arg == "-o"));
    assert_eq!(&target.args[target.args.len() - 2..], ["--", "10.0.0.7"]);
}

#[tokio::test]
async fn a_program_that_is_not_there_gives_no_answers() {
    let answers = ask_client_algorithms(Some("enodix-no-such-ssh-client-program")).await;
    assert_eq!(answers, ClientAlgorithms::default());
}

#[test]
fn it_runs_the_systems_ssh_by_default() {
    let target = system_ssh_target(
        None,
        &host(Some("admin")),
        None,
        Vec::new(),
        &current_client(),
    );
    assert_eq!(target.program, SYSTEM_SSH_PROGRAM);
}

#[test]
fn it_allows_the_outdated_algorithms_and_names_the_device_last() {
    let target = system_ssh_target(
        None,
        &host(Some("admin")),
        None,
        Vec::new(),
        &current_client(),
    );
    let line = target.args.join(" ");
    for expected in [
        "KexAlgorithms=+diffie-hellman-group1-sha1",
        "HostKeyAlgorithms=+ssh-rsa,ssh-dss",
        "Ciphers=+aes128-cbc",
        "MACs=+hmac-sha1",
        "hmac-md5",
        "-p 2222",
        "-l admin",
    ] {
        assert!(line.contains(expected), "{expected}: {line}");
    }
    assert!(!line.contains("blowfish"), "{line}");
    assert_eq!(&target.args[target.args.len() - 2..], ["--", "10.0.0.7"]);
}

#[test]
fn it_leaves_the_user_to_the_client_when_none_is_given() {
    let target = system_ssh_target(None, &host(None), None, Vec::new(), &current_client());
    assert!(!target.args.iter().any(|arg| arg == "-l"));
}

fn on_path(program: &str) -> bool {
    let names = [program.to_owned(), format!("{program}.exe")];
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .any(|folder| names.iter().any(|name| folder.join(name).is_file()))
        })
        .unwrap_or(false)
}

/// The real client reads the core's arguments for a made-up host: `-G`
/// prints the configuration and connects to nothing (no agent, no network).
/// A client that refuses an option exits with 255.
#[tokio::test]
async fn the_installed_client_accepts_the_arguments() {
    let mut programs: Vec<String> = Vec::new();
    if on_path("ssh") {
        programs.push(SYSTEM_SSH_PROGRAM.into());
    }
    if cfg!(windows) {
        let windows = std::path::Path::new(r"C:\Windows\System32\OpenSSH\ssh.exe");
        if windows.is_file() {
            programs.push(windows.to_string_lossy().into_owned());
        }
    }
    if programs.is_empty() {
        eprintln!("skipped: no ssh client on the PATH");
        return;
    }
    for program in programs {
        let client = ask_client_algorithms(Some(&program)).await;
        assert!(client.kex.is_some(), "{program} could not be asked");
        let target = system_ssh_target(
            Some(&program),
            &host(Some("admin")),
            None,
            Vec::new(),
            &client,
        );
        let output = std::process::Command::new(&target.program)
            .arg("-G")
            .args(&target.args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{program} refused {:?}: {}",
            target.args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_client_is_started_in_a_local_session_with_those_arguments() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use enodix_core::session::{
        SessionEvent, SessionManager, SessionState, SessionTarget, TerminalSize,
    };
    use support::{known_hosts, next_state};
    use tempfile::TempDir;

    let folder = TempDir::new().unwrap();
    let script = folder.path().join("fake-ssh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'ARGS:%s\\n' \"$*\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let target = system_ssh_target(
        script.to_str(),
        &host(Some("admin")),
        Some(folder.path().into()),
        Vec::new(),
        &current_client(),
    );
    let manager = SessionManager::new(known_hosts());
    let (_id, mut events) = manager.open(
        SessionTarget::Local(target),
        TerminalSize {
            cols: 200,
            rows: 24,
        },
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);

    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !String::from_utf8_lossy(&received).contains("-- 10.0.0.7") {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => {
                    events.acknowledge(bytes.len());
                    received.extend_from_slice(&bytes);
                }
                other => panic!("expected output, got {other:?}"),
            }
        }
    })
    .await
    .expect("the client's arguments never arrived");
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("ARGS:-o KexAlgorithms=+"), "{text}");
    assert!(text.contains("-p 2222 -l admin -- 10.0.0.7"), "{text}");
}
