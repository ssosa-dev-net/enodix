//! The headless binary's `tunnel` command: its JSON line, what it refuses
//! (before connecting, and a jump host it cannot reach), how it exits when
//! its standard input ends, and its command line. The built binary runs
//! against a temporary workspace and data folder; jump-host logins come
//! from the core's credential agent served in this test process, as the
//! running app serves them. A minimal jump host ([`JumpHost`]: a password
//! login and `direct-tcpip`, nothing else) gives listening entries, for the
//! JSON's shape and the extra ports (`--port`). Tunnels that carry SSH are
//! covered by the core's `tests/tunnels.rs` and, through this binary, by
//! the Python suite (`python/tests/test_tunnels.py`).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::net::{TcpListener, TcpStream};

use enodix_core::credentials::{
    vault_file, AgentEndpoint, AgentServer, CredentialProfile, CredentialStore, KdfParams,
    MemoryStore, ProfileAuth, StoreContents, Vault,
};
use serde_json::Value;
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_enodix-cli");
const SCOPE: &str = "cli-tunnel-test";
/// A host key (the core's RSA test fixture's public key), as the app
/// would have trusted it. [`JumpHost`] shows it too.
const DEVICE_KEY: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCZkcvfK8OMup2sAEnf8XDRIomHqI2xAUiqsv9r8XwTcQgtU4YYl/Y3tBJFjUpEciAeo/RWWi0xY0uSJsDR03BTm8hl9Xt6mEYc4WFIdM/iWRiq/Bvsctze/g0eDf72MSneiB8VS5VLn5fEoHofnjSsMgYaoFNlAea+fGHcPXvCoP104XhU6TCpUOGvlvUaNEDWmzkENTFlrgiofSEU+Esc7HzgpBtsfpRaURx1kjjzzcVx6dTT5wliue5Iaw1d/sKL57L2GcN/Z4V8K17U6jXlzp/m56/v95yZJpeGZ4Pk5dbaVN30wqXBLzpMsWFeR4KBBjB3yE3x6b4CFpUnTfSn";

struct Setup {
    _temp: TempDir,
    workspace: PathBuf,
    data: PathBuf,
}

impl Setup {
    /// A workspace with `files` (path, text) and an id, and an empty data
    /// folder.
    fn new(files: &[(&str, String)]) -> Self {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("_workspace.yaml"), format!("id: {SCOPE}\n")).unwrap();
        for (path, text) in files {
            let file = workspace.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }
        let data = temp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        Self {
            _temp: temp,
            workspace,
            data,
        }
    }

    fn ws(&self) -> &str {
        self.workspace.to_str().unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BINARY);
        command
            .args(args)
            .env("ENODIX_DATA_DIR", &self.data)
            .env("ENODIX_KEYCHAIN_PREFIX", "enodix-test-tunnel-never-used")
            .env_remove("ENODIX_JOB_TARGETS");
        without_terminal(&mut command);
        command
    }

    /// Serves the profile `lab` as the running app would.
    fn serve(&self) -> AgentServer {
        let store = MemoryStore::new();
        store
            .put(
                SCOPE,
                &CredentialProfile {
                    name: "lab".into(),
                    username: "admin".into(),
                    auth: ProfileAuth::Password {
                        password: "s3cret".into(),
                    },
                    enable_secret: None,
                },
            )
            .unwrap();
        AgentServer::start(AgentEndpoint::for_data_folder(&self.data), Arc::new(store)).unwrap()
    }

    /// Trusts [`DEVICE_KEY`] for 127.0.0.1 at `port`, as the app would.
    fn trust_device(&self, port: u16) {
        fs::write(
            self.data.join("known_hosts"),
            format!("[127.0.0.1]:{port} {DEVICE_KEY}\n"),
        )
        .unwrap();
    }

    /// Trusts [`DEVICE_KEY`] for 127.0.0.1 at each of `ports`.
    fn trust(&self, ports: &[u16]) {
        let lines: String = ports
            .iter()
            .map(|port| format!("[127.0.0.1]:{port} {DEVICE_KEY}\n"))
            .collect();
        fs::write(self.data.join("known_hosts"), lines).unwrap();
    }

    /// Starts `tunnel <ws> <args>`, with its stdin open.
    fn start(&self, args: &[&str]) -> Tunnel {
        let mut all = vec!["tunnel", self.ws()];
        all.extend_from_slice(args);
        let mut child = self
            .command(&all)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "it printed nothing: {}", rest(&mut child));
        Tunnel {
            child,
            _stdout: stdout,
            answer: serde_json::from_str(&line).expect("one line of JSON"),
        }
    }
}

struct Tunnel {
    child: Child,
    _stdout: BufReader<ChildStdout>,
    answer: Value,
}

impl Tunnel {
    fn entry(&self, host: &str) -> &Value {
        self.answer["tunnels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["host"] == host)
            .unwrap_or_else(|| panic!("no entry for {host}: {}", self.answer))
    }

    fn reason(&self, host: &str) -> String {
        let entry = self.entry(host);
        assert_eq!(entry["status"], "refused", "{entry}");
        entry["reason"].as_str().unwrap().to_owned()
    }

    /// Closes its stdin: it must exit with status 0, soon.
    fn end(mut self) {
        drop(self.child.stdin.take());
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}: {}", rest(&mut self.child));
                return;
            }
            assert!(Instant::now() < until, "it did not exit when stdin ended");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What it said on stderr (ending it first).
fn rest(child: &mut Child) -> String {
    let _ = child.kill();
    let _ = child.wait();
    let mut said = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut said);
    }
    said
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
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

/// A loopback port nothing listens on.
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An SSH server that only logs in the profile `lab` (password) and
/// forwards `direct-tcpip` channels: a jump host. Its host key is the
/// fixture's, [`DEVICE_KEY`]. Stops when dropped.
struct JumpHost {
    port: u16,
    _runtime: tokio::runtime::Runtime,
}

impl JumpHost {
    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let key = russh::keys::load_secret_key(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../core/tests/fixtures/keys/rsa_client"
            ),
            None,
        )
        .unwrap();
        let config = Arc::new(russh::server::Config {
            auth_rejection_time: Duration::ZERO,
            keys: vec![key],
            ..Default::default()
        });
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        runtime.spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(session) = russh::server::run_stream(config, socket, Jump).await {
                        let _ = session.await;
                    }
                });
            }
        });
        Self {
            port,
            _runtime: runtime,
        }
    }
}

struct Jump;

impl russh::server::Handler for Jump {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<russh::server::Auth, Self::Error> {
        Ok(if user == "admin" && password == "s3cret" {
            russh::server::Auth::Accept
        } else {
            russh::server::Auth::reject()
        })
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let address = (host_to_connect.to_owned(), port_to_connect as u16);
        match tokio::net::TcpStream::connect(address).await {
            Ok(mut socket) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut socket).await;
                });
            }
            Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}

/// A loopback TCP service that greets each connection with `hello\n`,
/// then echoes; its port.
fn echo_service() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut socket in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = socket.write_all(b"hello\n");
                let mut buffer = [0; 1024];
                while let Ok(n) = socket.read(&mut buffer) {
                    if n == 0 || socket.write_all(&buffer[..n]).is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// A workspace with the jump host `bastion.yaml` and `r1`, `r2` behind
/// it (their SSH port is `device_port`), everything trusted, and the app
/// serving `lab`.
fn behind_a_jump_host(jump: &JumpHost, device_port: u16) -> (Setup, AgentServer) {
    let lab = "credential_profile: lab\n";
    let behind = format!("address: 127.0.0.1\nport: {device_port}\njump_host: bastion.yaml\n");
    let setup = Setup::new(&[
        (
            "bastion.yaml",
            format!("address: 127.0.0.1\nport: {}\n{lab}", jump.port),
        ),
        ("r1.yaml", behind.clone()),
        ("r2.yaml", behind),
    ]);
    setup.trust(&[jump.port, device_port]);
    let app = setup.serve();
    (setup, app)
}

/// The fields of a JSON object, sorted.
fn fields(entry: &Value) -> Vec<&str> {
    let mut fields: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    fields
}

#[test]
fn without_ports_a_listening_entry_is_as_before() {
    let jump = JumpHost::start();
    let (setup, _app) = behind_a_jump_host(&jump, closed_port());
    let tunnel = setup.start(&["--host", "r1.yaml"]);
    assert_eq!(tunnel.answer["format"], 1);
    let entry = tunnel.entry("r1.yaml");
    assert_eq!(entry["status"], "listening", "{entry}");
    assert_eq!(
        fields(entry),
        ["address", "host", "host_keys", "port", "status"]
    );
    assert_eq!(entry["address"], "127.0.0.1");
    assert_eq!(entry["host_keys"], serde_json::json!([DEVICE_KEY]));
    tunnel.end();
}

/// `--port`, repeated: each listening host has its extra ports, and one
/// reaches the device's service through the jump host. The SSH port and a
/// port named twice add nothing.
#[test]
fn extra_ports_are_listed_per_host_and_reach_the_device() {
    let jump = JumpHost::start();
    let device_port = closed_port();
    let service = echo_service();
    let (setup, _app) = behind_a_jump_host(&jump, device_port);
    let (service_arg, device_arg) = (service.to_string(), device_port.to_string());
    let tunnel = setup.start(&[
        "--port",
        &service_arg,
        "--port",
        "443",
        "--port",
        &device_arg,
        "--port",
        &service_arg,
    ]);
    assert_eq!(tunnel.answer["format"], 1);
    for host in ["r1.yaml", "r2.yaml"] {
        let entry = tunnel.entry(host);
        assert_eq!(entry["status"], "listening", "{entry}");
        assert_eq!(
            fields(entry),
            [
                "address",
                "extra_ports",
                "host",
                "host_keys",
                "port",
                "status"
            ]
        );
        let extra = entry["extra_ports"].as_object().unwrap();
        let mut named: Vec<&str> = extra.keys().map(String::as_str).collect();
        named.sort_unstable();
        let mut wanted = vec!["443", service_arg.as_str()];
        wanted.sort_unstable();
        assert_eq!(named, wanted, "{entry}");
        assert!(extra
            .values()
            .all(|port| port.as_u64().is_some_and(|p| p > 0)));
        assert_ne!(extra["443"], entry["port"]);
    }

    let listener = tunnel.entry("r2.yaml")["extra_ports"][&service_arg]
        .as_u64()
        .unwrap() as u16;
    let mut socket = TcpStream::connect(("127.0.0.1", listener)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line, "hello\n");
    socket.write_all(b"<hello/>\n").unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line, "<hello/>\n");
    drop((socket, reader));
    tunnel.end();
}

#[test]
fn a_bad_port_is_a_usage_error_and_opens_nothing() {
    let jump = JumpHost::start();
    let (setup, _app) = behind_a_jump_host(&jump, closed_port());
    for value in ["0", "65536", "-1", "+830", "830.0", "ssh", ""] {
        let output = setup
            .command(&["tunnel", setup.ws(), "--port", "830", "--port", value])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{value:?}");
        assert!(output.stdout.is_empty(), "{value:?}");
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(said.contains("--port"), "{value:?}: {said}");
    }
    let output = setup
        .command(&["tunnel", setup.ws(), "--port"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--port needs a value"));
}

#[test]
fn with_nothing_behind_a_jump_host_it_lists_nothing_and_opens_no_store() {
    let setup = Setup::new(&[("r1.yaml", "address: 192.0.2.1\n".into())]);
    // A vault with no terminal and no app would be refused if it were
    // opened.
    drop(
        Vault::create_with(
            &vault_file(&setup.data),
            "password",
            KdfParams {
                memory_kib: 8 * 1024,
                iterations: 1,
                parallelism: 1,
            },
            StoreContents::default(),
        )
        .unwrap(),
    );
    let tunnel = setup.start(&[]);
    assert_eq!(
        tunnel.answer,
        serde_json::json!({"format": 1, "tunnels": []})
    );
    tunnel.end();
}

#[test]
fn hosts_that_cannot_be_tunnelled_are_listed_with_why() {
    let bastion_port = closed_port();
    let device_port = closed_port();
    let lab = "credential_profile: lab\n";
    let setup = Setup::new(&[
        (
            "bastion.yaml",
            format!("address: 127.0.0.1\nport: {bastion_port}\n{lab}connect_timeout: 5\n"),
        ),
        ("open.yaml", "address: 192.0.2.9\n".into()),
        ("direct.yaml", format!("address: 192.0.2.1\n{lab}")),
        (
            "behind/unreachable.yaml",
            format!("address: 127.0.0.1\nport: {device_port}\njump_host: bastion.yaml\n"),
        ),
        (
            "behind/no-login.yaml",
            "address: 192.0.2.3\njump_host: open.yaml\n".into(),
        ),
        (
            "behind/broken.yaml",
            "address: 192.0.2.4\njump_host: nowhere.yaml\n".into(),
        ),
        (
            "behind/telnet.yaml",
            "address: 192.0.2.5\nprotocol: telnet\njump_host: bastion.yaml\n".into(),
        ),
        (
            "behind/untrusted.yaml",
            "address: 192.0.2.6\njump_host: bastion.yaml\n".into(),
        ),
    ]);
    setup.trust_device(device_port);
    let _app = setup.serve();
    let tunnel = setup.start(&[]);
    assert_eq!(tunnel.answer["format"], 1);
    let hosts: Vec<&str> = tunnel.answer["tunnels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["host"].as_str().unwrap())
        .collect();
    // In the inventory's order; hosts reached directly are not listed.
    assert_eq!(
        hosts,
        [
            "behind/broken.yaml",
            "behind/no-login.yaml",
            "behind/telnet.yaml",
            "behind/unreachable.yaml",
            "behind/untrusted.yaml",
        ]
    );
    let reason = tunnel.reason("behind/unreachable.yaml");
    assert!(reason.contains("jump host bastion.yaml"), "{reason}");
    assert!(reason.contains("refused"), "{reason}");
    assert!(tunnel
        .reason("behind/no-login.yaml")
        .contains("open.yaml has no stored credential profile"));
    assert!(tunnel.reason("behind/broken.yaml").contains("nowhere.yaml"));
    assert!(tunnel
        .reason("behind/telnet.yaml")
        .contains("only SSH is tunnelled"));
    let reason = tunnel.reason("behind/untrusted.yaml");
    assert!(reason.contains("connect to it once in the app"), "{reason}");
    tunnel.end();
}

#[test]
fn it_takes_the_inventory_s_limits() {
    let setup = Setup::new(&[
        (
            "a/r1.yaml",
            "address: 192.0.2.1\njump_host: gone.yaml\n".into(),
        ),
        (
            "b/r2.yaml",
            "address: 192.0.2.2\njump_host: gone.yaml\n".into(),
        ),
    ]);
    // Hosts behind jump hosts: the store is opened (the app's, here; a
    // system with no keychain and no vault has none to open).
    let _app = setup.serve();
    let tunnel = setup.start(&["--folder", "b"]);
    let hosts: Vec<&Value> = tunnel.answer["tunnels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["host"])
        .collect();
    assert_eq!(hosts, ["b/r2.yaml"]);
    tunnel.end();
    let tunnel = setup.start(&["--host", "a/r1.yaml"]);
    assert_eq!(tunnel.answer["tunnels"][0]["host"], "a/r1.yaml");
    tunnel.end();
    // As `--option=<value>` too, and the workspace after `--`.
    let tunnel = setup.start(&["--host=a/r1.yaml", "--port=830"]);
    assert_eq!(tunnel.answer["tunnels"][0]["host"], "a/r1.yaml");
    tunnel.end();
    let mut child = setup
        .command(&["tunnel", "--folder=b", "--", setup.ws()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let answer: Value = serde_json::from_str(&line).expect("one line of JSON");
    assert_eq!(answer["tunnels"][0]["host"], "b/r2.yaml");
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
}

#[test]
fn a_script_job_s_scope_applies() {
    let setup = Setup::new(&[("r1.yaml", "address: 192.0.2.1\n".into())]);
    let targets = setup.data.join("targets.json");
    fs::write(
        &targets,
        serde_json::json!({
            "format": 1,
            "workspace": Path::new("C:/somewhere/else"),
            "hosts": ["r1.yaml"],
        })
        .to_string(),
    )
    .unwrap();
    let output = setup
        .command(&["tunnel", setup.ws()])
        .env("ENODIX_JOB_TARGETS", &targets)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("reads no other workspace"));
}

#[test]
fn a_command_line_it_does_not_understand() {
    let setup = Setup::new(&[]);
    for args in [
        vec!["tunnel"],
        vec!["tunnel", setup.ws(), "--folder"],
        vec!["tunnel", setup.ws(), "--folder", "a", "--filter", "b"],
        vec!["tunnel", setup.ws(), "--port", "0"],
        vec!["tunnel", setup.ws(), "--hosts", "1"],
        vec!["tunnel", setup.ws(), "--port=0"],
        vec!["tunnel", setup.ws(), "--port="],
        // After `--` nothing is an option, `--port` neither.
        vec!["tunnel", "--", setup.ws(), "--port", "830"],
    ] {
        let output = setup.command(&args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    let output = setup
        .command(&["tunnel", setup.ws(), "--folder", "nowhere"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let help = setup.command(&["--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("tunnel <workspace folder>"));
    assert!(help.contains("[--port <number>...]"), "{help}");
}
