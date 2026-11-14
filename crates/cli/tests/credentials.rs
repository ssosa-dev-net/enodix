//! The headless binary's `credentials` and `secret` commands: the built
//! binary is started as a process against a temporary workspace and data
//! folder. "The app running" is the core's credential agent served in this
//! test process, as the app serves it; "no terminal" is a process started
//! without a console (Windows) or a controlling terminal (Unix); "a
//! terminal" is a pseudo terminal the test types into.
//!
//! Every run gets its own data folder and its own keychain prefix
//! (`netterm-test-<hex>`), so nothing here touches the app's real entries.

use std::fs;
use std::hash::BuildHasher;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use netterm_core::credentials::{
    vault_file, ActiveStore, AgentEndpoint, AgentServer, CredentialProfile, CredentialStore,
    KdfParams, MemoryStore, ProfileAuth, ScopedProfile, ScopedSecret, SecretValue, StoreContents,
    Vault,
};
use serde_json::{json, Value};
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_netterm-cli");
const SCOPE: &str = "cli-credentials-test";
const PASSWORD: &str = "correct horse battery";

/// A workspace and a data folder, each new.
struct Setup {
    _temp: TempDir,
    workspace: PathBuf,
    data: PathBuf,
    keychain_prefix: String,
}

impl Setup {
    /// `r1` uses the password profile `lab`, `r2` the key profile `keys`,
    /// `r3` the agent profile `agent`, `r4` none, `r5` one never stored.
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(workspace.join("site")).unwrap();
        fs::write(workspace.join("_workspace.yaml"), format!("id: {SCOPE}\n")).unwrap();
        fs::write(
            workspace.join("site/_folder.yaml"),
            "credential_profile: lab\n",
        )
        .unwrap();
        fs::write(workspace.join("site/r1.yaml"), "address: 192.0.2.1\n").unwrap();
        for (file, profile) in [("r2", "keys"), ("r3", "agent"), ("r5", "nowhere")] {
            fs::write(
                workspace.join(format!("{file}.yaml")),
                format!("address: 192.0.2.9\ncredential_profile: {profile}\n"),
            )
            .unwrap();
        }
        fs::write(workspace.join("r4.yaml"), "address: 192.0.2.4\n").unwrap();
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let random: u64 = std::collections::hash_map::RandomState::new().hash_one(temp.path());
        Self {
            _temp: temp,
            workspace,
            data,
            keychain_prefix: format!("netterm-test-{random:016x}"),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BINARY);
        command
            .args(args)
            .env("NETTERM_DATA_DIR", &self.data)
            .env("NETTERM_KEYCHAIN_PREFIX", &self.keychain_prefix);
        command
    }

    /// Runs the binary with no terminal to ask on.
    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.command(args);
        without_terminal(&mut command);
        command.output().expect("the binary starts")
    }

    fn ws(&self) -> &str {
        self.workspace.to_str().unwrap()
    }

    fn serve(&self, store: Arc<dyn CredentialStore>) -> AgentServer {
        AgentServer::start(AgentEndpoint::for_data_folder(&self.data), store).unwrap()
    }

    fn create_vault(&self) {
        drop(Vault::create_with(&vault_file(&self.data), PASSWORD, cheap(), contents()).unwrap());
    }
}

#[cfg(windows)]
fn without_terminal(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: the process gets no console at all.
    command.creation_flags(0x0800_0000);
}

#[cfg(unix)]
fn without_terminal(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // A session of its own has no controlling terminal.
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

fn cheap() -> KdfParams {
    KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    }
}

fn profiles() -> Vec<CredentialProfile> {
    vec![
        CredentialProfile {
            name: "LAB".into(),
            username: "admin".into(),
            auth: ProfileAuth::Password {
                password: "s3cret-pass".into(),
            },
            enable_secret: Some("en4ble".into()),
        },
        CredentialProfile {
            name: "keys".into(),
            username: "netops".into(),
            auth: ProfileAuth::PrivateKey {
                key_file: "C:/keys/id_ed25519".into(),
                passphrase: Some("phr4se".into()),
            },
            enable_secret: None,
        },
        CredentialProfile {
            name: "agent".into(),
            username: "auto".into(),
            auth: ProfileAuth::Agent,
            enable_secret: None,
        },
    ]
}

fn contents() -> StoreContents {
    StoreContents {
        profiles: profiles()
            .into_iter()
            .map(|profile| ScopedProfile {
                scope: SCOPE.into(),
                profile,
            })
            .collect(),
        secrets: vec![ScopedSecret {
            scope: SCOPE.into(),
            name: "netbox-token".into(),
            value: SecretValue::new("t0ken"),
        }],
    }
}

fn fill(store: &dyn CredentialStore) {
    for entry in contents().profiles {
        store.put(&entry.scope, &entry.profile).unwrap();
    }
    for entry in contents().secrets {
        store
            .put_secret(&entry.scope, &entry.name, &entry.value)
            .unwrap();
    }
}

fn memory_store() -> Arc<MemoryStore> {
    let store = MemoryStore::new();
    fill(&store);
    Arc::new(store)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Succeeds and prints JSON.
fn answered(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{}", stderr(output));
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

/// Fails with status 1, printing nothing, and says `expected`.
fn refused(output: &Output, expected: &str) {
    assert_eq!(output.status.code(), Some(1), "{}", stderr(output));
    assert!(output.stdout.is_empty());
    let message = stderr(output);
    assert!(message.contains(expected), "{message}");
}

fn r1_credentials() -> Value {
    json!({
        "format": 1,
        "host": "site/r1.yaml",
        "profile": "LAB",
        "username": "admin",
        "auth": { "kind": "password", "password": "s3cret-pass" },
        "enable_secret": "en4ble",
    })
}

fn netbox_token() -> Value {
    json!({ "format": 1, "name": "netbox-token", "value": "t0ken" })
}

/// Every way of logging in, and the secret, as the store holds them.
fn check_everything_served(setup: &Setup) {
    let ws = setup.ws();
    assert_eq!(
        answered(&setup.run(&["credentials", ws, "site/r1.yaml"])),
        r1_credentials()
    );
    let keys = answered(&setup.run(&["credentials", ws, "r2.yaml"]));
    assert_eq!(keys["username"], "netops");
    assert_eq!(
        keys["auth"],
        json!({ "kind": "private_key", "key_file": "C:/keys/id_ed25519", "passphrase": "phr4se" })
    );
    let agent = answered(&setup.run(&["credentials", ws, "r3.yaml"]));
    assert_eq!(agent["auth"], json!({ "kind": "agent" }));
    assert_eq!(
        answered(&setup.run(&["secret", ws, "NETBOX-token"])),
        json!({ "format": 1, "name": "NETBOX-token", "value": "t0ken" })
    );
}

#[test]
fn with_the_app_running_it_answers_from_the_app() {
    let setup = Setup::new();
    let _app = setup.serve(memory_store());
    check_everything_served(&setup);
    assert_eq!(
        answered(&setup.run(&["secret", setup.ws(), "netbox-token"])),
        netbox_token()
    );
}

#[test]
fn with_the_app_running_and_its_vault_unlocked_it_answers_without_asking() {
    let setup = Setup::new();
    setup.create_vault();
    let store = Arc::new(ActiveStore::new(
        Box::new(MemoryStore::new()),
        vault_file(&setup.data),
    ));
    store.unlock(PASSWORD).unwrap();
    let _app = setup.serve(store);
    // No terminal: it never asks.
    check_everything_served(&setup);
}

#[test]
fn with_the_app_running_but_locked_and_no_terminal_it_is_refused() {
    let setup = Setup::new();
    setup.create_vault();
    let store = Arc::new(ActiveStore::new(
        Box::new(MemoryStore::new()),
        vault_file(&setup.data),
    ));
    let _app = setup.serve(store);
    let output = setup.run(&["credentials", setup.ws(), "site/r1.yaml"]);
    refused(&output, "running but its vault is locked: unlock it there");
}

#[test]
fn with_a_vault_no_app_and_no_terminal_it_is_refused() {
    let setup = Setup::new();
    setup.create_vault();
    for args in [
        ["credentials", setup.ws(), "site/r1.yaml"],
        ["secret", setup.ws(), "netbox-token"],
    ] {
        let output = setup.run(&args);
        refused(&output, "no terminal to ask for its master password on");
        assert!(stderr(&output).contains("start netterm and unlock it"));
    }
}

#[test]
fn it_says_what_is_missing() {
    let setup = Setup::new();
    let _app = setup.serve(memory_store());
    let ws = setup.ws();
    refused(
        &setup.run(&["credentials", ws, "nope.yaml"]),
        "there is no host nope.yaml",
    );
    refused(
        &setup.run(&["credentials", ws, "r4.yaml"]),
        "r4.yaml has no credential profile",
    );
    refused(
        &setup.run(&["credentials", ws, "r5.yaml"]),
        "the credential profile nowhere is not set up on this computer",
    );
    refused(
        &setup.run(&["secret", ws, "nothing"]),
        "there is no secret called nothing",
    );
    let missing = setup.workspace.join("gone");
    refused(
        &setup.run(&["secret", missing.to_str().unwrap(), "x"]),
        "gone",
    );
    for args in [
        &["credentials", ws][..],
        &["secret"][..],
        &["secret", ws, "a", "b"][..],
        &["credentials", ws, "--host", "r1.yaml"][..],
    ] {
        assert_eq!(setup.run(args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn a_host_id_may_use_backslashes() {
    let setup = Setup::new();
    let _app = setup.serve(memory_store());
    assert_eq!(
        answered(&setup.run(&["credentials", setup.ws(), "site\\r1.yaml"])),
        r1_credentials()
    );
}

#[test]
fn run_by_a_script_job_it_gives_only_the_jobs_hosts_credentials() {
    let setup = Setup::new();
    let _app = setup.serve(memory_store());
    let targets = setup.data.join("targets.json");
    let write = |workspace: &Path| {
        fs::write(
            &targets,
            json!({ "format": 1, "workspace": workspace, "hosts": ["site/r1.yaml"] }).to_string(),
        )
        .unwrap();
    };
    let as_job = |args: &[&str]| {
        let mut command = setup.command(args);
        command.env("NETTERM_JOB_TARGETS", &targets);
        without_terminal(&mut command);
        command.output().unwrap()
    };
    write(&setup.workspace);
    let ws = setup.ws();
    assert_eq!(
        answered(&as_job(&["credentials", ws, "site/r1.yaml"])),
        r1_credentials()
    );
    refused(
        &as_job(&["credentials", ws, "r2.yaml"]),
        "the host r2.yaml is not among the hosts of the job this script runs in",
    );
    assert_eq!(
        answered(&as_job(&["secret", ws, "netbox-token"])),
        netbox_token()
    );
    // Written for another workspace: this one is not read at all.
    write(&setup.data);
    refused(
        &as_job(&["credentials", ws, "site/r1.yaml"]),
        "reads no other workspace",
    );
    refused(
        &as_job(&["secret", ws, "netbox-token"]),
        "reads no other workspace",
    );
}

/// With the app closed and no vault, the binary reads the keychain itself
/// (entries under this test's own prefix, removed afterwards).
#[cfg(windows)]
#[test]
fn with_the_app_closed_it_reads_the_keychain() {
    use netterm_core::credentials::KeychainStore;

    struct Cleanup(KeychainStore);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for entry in contents().profiles {
                let _ = self.0.delete(&entry.scope, &entry.profile.name);
            }
            for entry in contents().secrets {
                let _ = self.0.delete_secret(&entry.scope, &entry.name);
            }
        }
    }

    let setup = Setup::new();
    let keychain = Cleanup(KeychainStore::new(setup.keychain_prefix.clone()));
    fill(&keychain.0);
    check_everything_served(&setup);
    drop(keychain);
    refused(
        &setup.run(&["credentials", setup.ws(), "site/r1.yaml"]),
        "the credential profile lab is not set up on this computer",
    );
}

/// Elsewhere there is no keychain: with the app closed and no vault, the
/// binary says the vault is where credentials are kept.
#[cfg(not(windows))]
#[test]
fn with_the_app_closed_and_no_vault_it_points_to_the_vault() {
    let setup = Setup::new();
    refused(
        &setup.run(&["credentials", setup.ws(), "site/r1.yaml"]),
        "keeps them only in its vault, which is not turned on",
    );
    refused(
        &setup.run(&["secret", setup.ws(), "netbox-token"]),
        "keeps them only in its vault, which is not turned on",
    );
}

/// A pseudo terminal running the binary, its screen collected as text.
struct Terminal {
    screen: Arc<Mutex<String>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Terminal {
    fn start(setup: &Setup, args: &[&str]) -> Self {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 240,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(BINARY);
        command.args(args);
        command.env("NETTERM_DATA_DIR", &setup.data);
        command.env("NETTERM_KEYCHAIN_PREFIX", &setup.keychain_prefix);
        command.cwd(setup.workspace.parent().unwrap());
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let screen = Arc::new(Mutex::new(String::new()));
        let collected = screen.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                collected
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        });
        Self {
            screen,
            writer,
            child,
            _master: pair.master,
        }
    }

    /// The screen's text without control sequences.
    fn text(&self) -> String {
        plain(&self.screen.lock().unwrap())
    }

    /// Waits until the screen shows `expected` `count` times in all.
    fn wait_for(&mut self, expected: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            // A pseudo console may ask where the cursor is before it
            // shows anything.
            if self.screen.lock().unwrap().contains("\x1b[6n") {
                self.screen.lock().unwrap().clear();
                self.type_text("\x1b[1;1R");
            }
            if self.text().matches(expected).count() >= count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no {expected:?} on the screen: {:?}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn type_text(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).unwrap();
        self.writer.flush().unwrap();
    }

    /// Waits for the binary to exit; its status code.
    fn exit_code(&mut self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // What it printed last reaches the screen shortly after.
                std::thread::sleep(Duration::from_millis(300));
                return status.exit_code();
            }
            assert!(
                Instant::now() < deadline,
                "still running: {:?}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// `text` without escape sequences and carriage returns.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    for c in chars.by_ref() {
                        if c == '\x07' {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// With a vault and no app, the binary asks for the master password on the
/// terminal, without showing it, and answers once it is right.
#[test]
fn with_a_vault_and_no_app_it_asks_on_the_terminal() {
    let setup = Setup::new();
    setup.create_vault();
    let before = files(&setup.data);

    let mut terminal = Terminal::start(&setup, &["credentials", setup.ws(), "site/r1.yaml"]);
    terminal.wait_for("Master password of netterm's vault: ", 1);
    terminal.type_text("not-it\r");
    terminal.wait_for("Wrong password. Master password", 1);
    terminal.type_text(&format!("{PASSWORD}\r"));
    assert_eq!(terminal.exit_code(), 0, "{}", terminal.text());
    let screen = terminal.text();
    assert!(!screen.contains("not-it"), "the password showed: {screen}");
    assert!(!screen.contains(PASSWORD), "the password showed: {screen}");
    let json = &screen[screen.find('{').expect("JSON on the screen")..=screen.rfind('}').unwrap()];
    let json: String = json.split_whitespace().collect::<Vec<_>>().join(" ");
    let printed: Value = serde_json::from_str(&json).expect("the answer is JSON");
    assert_eq!(printed, r1_credentials());

    // Nothing was written along the way.
    assert_eq!(files(&setup.data), before);
}

#[test]
fn three_wrong_master_passwords_give_up() {
    let setup = Setup::new();
    setup.create_vault();
    let mut terminal = Terminal::start(&setup, &["secret", setup.ws(), "netbox-token"]);
    for attempt in 1..=3 {
        terminal.wait_for("Master password of netterm's vault: ", attempt);
        terminal.type_text("wrong\r");
    }
    assert_eq!(terminal.exit_code(), 1);
    assert!(
        terminal.text().contains("the master password is not right"),
        "{}",
        terminal.text()
    );
    assert!(!terminal.text().contains("t0ken"));
}

/// Every file under `folder` with its contents.
fn files(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push((path.clone(), fs::read(&path).unwrap()));
        }
    }
    found.sort();
    found
}
