//! Each kind of session opened through `session_open`, as a tab opens it:
//! SSH, telnet and serial against the test servers, a shell on this
//! computer and the system ssh tab (#70). Every command must answer within
//! [`COMMAND_WAIT`], and the workspace must still answer after each open:
//! 0.8.0's system ssh tab locked the workspace twice and never did.
//!
//! [`COMMAND_WAIT`]: super::app::COMMAND_WAIT

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::async_runtime::block_on;
use tempfile::TempDir;

use super::app::{TestApp, COLS, ROWS};
use super::support::serial::VirtualSerialPorts;
use super::support::telnet::{TestTelnetServer, USERNAME_PROMPT};
use super::support::{closed_port, TestSshServer, PASSWORD, PROMPT, USERNAME};

/// A workspace folder made of `files` (path, text).
fn workspace(files: &[(&str, String)]) -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    for (path, text) in files {
        fs::write(root.join(path), text).unwrap();
    }
    (temp, root)
}

/// Opens the workspace at `root` and returns its root as the app has it.
fn open(app: &TestApp, root: &Path) -> String {
    let view = app.open_workspace(root);
    let problems = &view["problems"];
    assert!(problems.as_array().is_none_or(Vec::is_empty), "{problems}");
    view["root"].as_str().expect("a root").to_owned()
}

fn host(id: &str) -> Value {
    Value::String(id.into())
}

#[test]
fn an_ssh_host_opens_asks_for_its_key_and_logs_in_with_its_profile() {
    let server = block_on(TestSshServer::start());
    let (_temp, root) = workspace(&[(
        "r1.yaml",
        format!(
            "address: 127.0.0.1\nport: {}\ncredential_profile: lab\n",
            server.port()
        ),
    )]);
    let app = TestApp::start();
    open(&app, &root);
    app.call(
        "profile_save",
        json!({
            "previous": null,
            "draft": {
                "name": "lab",
                "username": USERNAME,
                "auth": { "kind": "password", "password": PASSWORD },
            },
        }),
    );

    let mut session = app.open_session(json!({ "kind": "host", "id": host("r1.yaml") }));
    assert_eq!(session.opened["transport"], "ssh");
    assert_eq!(session.opened["encrypted"], true);
    assert_eq!(session.opened["record"]["host"], "r1.yaml");
    app.workspace_answers();

    let question = session.host_key_question();
    assert_eq!(question["port"], server.port());
    app.call(
        "session_decide_host_key",
        json!({ "id": session.id, "decision": "trustOnce" }),
    );
    session.connected();
    session.output_showing(&String::from_utf8_lossy(PROMPT));
    // The size the tab opened at reached the device.
    session.type_text("size\r");
    session.output_showing(&format!("{COLS}x{ROWS}"));
    app.workspace_answers();
    session.close();
}

#[test]
fn a_telnet_host_opens_and_the_device_asks_for_the_login() {
    let server = block_on(TestTelnetServer::start());
    let (_temp, root) = workspace(&[(
        "sw1.yaml",
        format!(
            "address: 127.0.0.1\nport: {}\nprotocol: telnet\n",
            server.port()
        ),
    )]);
    let app = TestApp::start();
    open(&app, &root);

    let mut session = app.open_session(json!({ "kind": "telnet", "id": host("sw1.yaml") }));
    assert_eq!(session.opened["transport"], "telnet");
    assert_eq!(session.opened["encrypted"], false);
    app.workspace_answers();
    session.connected();
    session.output_showing(&String::from_utf8_lossy(USERNAME_PROMPT));
    app.workspace_answers();
    session.close();
}

#[test]
fn a_serial_host_opens_its_port_with_its_settings() {
    let ports = VirtualSerialPorts::new();
    let device = ports.add("COM7", "USB Serial Port");
    let (_temp, root) = workspace(&[(
        "console.yaml",
        "address: 192.0.2.10\nprotocol: serial\nserial:\n  port: COM7\n  baud_rate: 115200\n"
            .into(),
    )]);
    let app = TestApp::with_serial_ports(Arc::new(ports));
    open(&app, &root);

    let mut session = app.open_session(json!({ "kind": "serial", "id": host("console.yaml") }));
    assert_eq!(session.opened["transport"], "serial");
    app.workspace_answers();
    session.connected();
    let opened = device.opened();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0].baud_rate, 115_200);

    device.send(b"Password: ").unwrap();
    session.output_showing("Password: ");
    session.type_text("secret\r");
    assert_eq!(block_on(device.received_ending_with(b"\r")), b"secret\r");
    app.workspace_answers();
    session.close();
}

#[test]
fn a_local_shell_starts_in_the_workspace_and_is_told_where_it_is() {
    let (_temp, root) = workspace(&[("r1.yaml", "address: 192.0.2.1\n".into())]);
    let app = TestApp::start();
    let root = open(&app, &root);

    // The shell the preferences name: none yet, so the first (PowerShell
    // on Windows, the login shell elsewhere).
    let mut session = app.open_session(json!({ "kind": "local", "shell": null }));
    assert_eq!(session.opened["transport"], "local");
    app.workspace_answers();
    session.connected();
    // The workspace's folder reaches the shell (`local_start`, which locks
    // the workspace).
    if cfg!(windows) {
        session.type_text("echo \"[$env:ENODIX_WORKSPACE]\"\r");
    } else {
        session.type_text("echo \"[$ENODIX_WORKSPACE]\"\r");
    }
    session.output_showing(&format!("[{root}]"));
    app.workspace_answers();
    session.close();
}

#[test]
fn the_system_ssh_tab_opens_and_runs_the_systems_client() {
    // Nothing listens there: the client says so and ends, having asked
    // nothing of anyone (no agent, no known hosts).
    let port = block_on(closed_port());
    let (_temp, root) =
        workspace(&[("old-r1.yaml", format!("address: 127.0.0.1\nport: {port}\n"))]);
    let app = TestApp::start();
    open(&app, &root);

    let mut session = app.open_session(json!({ "kind": "systemSsh", "id": host("old-r1.yaml") }));
    assert_eq!(session.opened["transport"], "local");
    assert!(
        session.opened["record"]["insecure"].is_string(),
        "{}",
        session.opened
    );
    assert_eq!(session.opened["record"]["host"], "old-r1.yaml");
    app.workspace_answers();
    session.connected();
    session.output_showing(&format!("port {port}"));
    // The client ends, and with it the tab's session.
    let closed = session.closed();
    assert!(closed["reason"].is_string(), "{closed}");
    app.workspace_answers();
}
