//! The harness: the app on the mock runtime ([`TestApp`]), its commands
//! called as the interface calls them, with a time limit, and a session's
//! channels read as a terminal reads them ([`TestSession`]).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use enodix_core::session::SerialPorts;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponse, InvokeResponseBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::Manager;
use tempfile::TempDir;

use crate::{with_app, Start, WebviewWindow, MAIN_WINDOW};

/// How long a command may take to answer. One that takes longer is taken
/// for a deadlock (0.8.0's system ssh tab never answered at all).
pub const COMMAND_WAIT: Duration = Duration::from_secs(20);

/// How long a session may take to show what a test waits for.
pub const SESSION_WAIT: Duration = Duration::from_secs(30);

/// The cursor position request a terminal is asked (ConPTY starts every
/// program with one and holds it until it is answered).
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

/// What a terminal answers it with.
const CURSOR_REPORT: &[u8] = b"\x1b[1;1R";

/// The size every test session opens at: wide, so that nothing a test
/// looks for is wrapped.
pub const COLS: u32 = 200;
pub const ROWS: u32 = 30;

/// Where each channel's messages go: the channel id the "page" made, to
/// the test reading it.
type Routes = Arc<Mutex<HashMap<u32, Sender<InvokeResponseBody>>>>;

/// How a page passes a channel to a command: this and the channel's id.
const CHANNEL_PREFIX: &str = "__CHANNEL__:";

/// Channel ids, as a page's `new Channel()` makes them.
static NEXT_CHANNEL: AtomicU32 = AtomicU32::new(1);

/// The app as `run` builds it, with its main window, on the mock runtime.
pub struct TestApp {
    // In this order: the window, then the app, then its data folder.
    main: WebviewWindow,
    _app: tauri::App<MockRuntime>,
    routes: Routes,
    _data: TempDir,
}

impl TestApp {
    /// The app with its own empty data folder and its credentials in
    /// memory (never the keychain).
    pub fn start() -> Self {
        Self::start_with(None)
    }

    /// The same, opening serial ports through `ports` (never the
    /// machine's own).
    pub fn with_serial_ports(ports: Arc<dyn SerialPorts>) -> Self {
        Self::start_with(Some(ports))
    }

    fn start_with(serial_ports: Option<Arc<dyn SerialPorts>>) -> Self {
        let data = TempDir::new().unwrap();
        let routes = Routes::default();
        let delivered = routes.clone();
        let builder = mock_builder().channel_interceptor(move |_webview, channel, _index, body| {
            let routes = delivered.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(test) = routes.get(&channel.0) {
                // A test that stopped reading no longer cares.
                let _ = test.send(body.clone());
            }
            true
        });
        let start = Start {
            data_folder: Some(data.path().to_path_buf()),
            credential_store: Some("memory".into()),
            serial_ports,
            updater_config: None,
            updater_plugin: false,
        };
        let mut app = with_app(builder, start)
            .build(mock_context(noop_assets()))
            .expect("the app builds");
        // The app's setup runs with the event loop's first turn; the mock
        // runtime's loop does nothing more.
        #[allow(deprecated)]
        app.run_iteration(|_, _| {});
        let main = app
            .get_webview_window(MAIN_WINDOW)
            .expect("the setup made the main window");
        Self {
            main,
            _app: app,
            routes,
            _data: data,
        }
    }

    /// Calls `command` from the main window with `args` (named as the
    /// interface names them), and returns what it answered. Panics if it
    /// has not answered within [`COMMAND_WAIT`], or ended without
    /// answering (it panicked).
    pub fn invoke(&self, command: &str, args: Value) -> Result<Value, Value> {
        let url = if cfg!(windows) {
            "https://tauri.localhost/"
        } else {
            "tauri://localhost/"
        };
        let request = InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.into(),
        };
        let (answer, answered) = mpsc::channel();
        let webview = self.main.as_ref().clone();
        // A command that is not `async` runs on the thread that calls it:
        // never the test's own, so that a deadlock cannot hold the test.
        std::thread::spawn(move || {
            webview.on_message(
                request,
                Box::new(move |_, _, response, _, _| {
                    let _ = answer.send(response);
                }),
            );
        });
        match answered.recv_timeout(COMMAND_WAIT) {
            Ok(InvokeResponse::Ok(body)) => Ok(body.deserialize::<Value>().unwrap()),
            Ok(InvokeResponse::Err(error)) => Err(error.0),
            Err(RecvTimeoutError::Timeout) => panic!(
                "{command} did not answer within {COMMAND_WAIT:?}: a lock it waits for is never \
                 let go (a deadlock?)"
            ),
            Err(RecvTimeoutError::Disconnected) => {
                panic!("{command} ended without an answer: it panicked (see above)")
            }
        }
    }

    /// Calls `command` and returns its answer, which must not be an error.
    pub fn call(&self, command: &str, args: Value) -> Value {
        self.invoke(command, args)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    /// A channel as a page makes one: the value to pass for it, and where
    /// what is sent on it arrives.
    pub fn channel(&self) -> (Value, Receiver<InvokeResponseBody>) {
        let id = NEXT_CHANNEL.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sender);
        (json!(format!("{CHANNEL_PREFIX}{id}")), receiver)
    }

    /// Opens the workspace folder `root`, as the workspace switcher does.
    pub fn open_workspace(&self, root: &Path) -> Value {
        self.call("workspace_open", json!({ "path": root, "confirmed": true }))
    }

    /// Opens a tab in the main window and returns its id.
    pub fn open_tab(&self) -> u64 {
        let changed = self.call("layout_edit", json!({ "edit": { "kind": "openTab" } }));
        changed["created"]
            .as_u64()
            .unwrap_or_else(|| panic!("no tab was created: {changed}"))
    }

    /// Checks that the workspace still answers: a command left holding it
    /// would make this one wait for ever.
    pub fn workspace_answers(&self) -> Value {
        self.call("workspace_current", json!({}))
    }

    /// Opens a session to `target` (a `ConnectTarget` as the interface
    /// sends it) in a new tab, as a terminal does.
    pub fn open_session(&self, target: Value) -> TestSession<'_> {
        let tab = self.open_tab();
        let (on_output, output) = self.channel();
        let (on_state, states) = self.channel();
        let (on_prompt, _) = self.channel();
        let (on_host_key, host_keys) = self.channel();
        let (on_agent_forwarding, _) = self.channel();
        let (on_algorithms, _) = self.channel();
        let (on_log_failure, _) = self.channel();
        let (on_backup, _) = self.channel();
        let opened = self.call(
            "session_open",
            json!({
                "target": target,
                "tab": tab,
                "name": "test",
                "title": "test",
                "size": { "cols": COLS, "rows": ROWS },
                "onOutput": on_output,
                "onState": on_state,
                "onPrompt": on_prompt,
                "onHostKey": on_host_key,
                "onAgentForwarding": on_agent_forwarding,
                "onAlgorithms": on_algorithms,
                "onLogFailure": on_log_failure,
                "onBackup": on_backup,
            }),
        );
        TestSession {
            app: self,
            id: opened["id"].as_u64().expect("an id"),
            consumer: opened["consumer"].as_u64().expect("a consumer key"),
            opened,
            output,
            states,
            host_keys,
            seen: Vec::new(),
            answered: 0,
        }
    }
}

/// A session opened through `session_open`, read as a terminal reads it:
/// its output acknowledged, the cursor position requests answered.
pub struct TestSession<'a> {
    app: &'a TestApp,
    pub id: u64,
    consumer: u64,
    /// What `session_open` answered.
    pub opened: Value,
    output: Receiver<InvokeResponseBody>,
    states: Receiver<InvokeResponseBody>,
    host_keys: Receiver<InvokeResponseBody>,
    /// Every byte of output so far.
    seen: Vec<u8>,
    /// Cursor position requests answered so far.
    answered: usize,
}

impl TestSession<'_> {
    /// The next state the session reports.
    pub fn next_state(&self) -> Value {
        json_of(next(&self.states, "a state"))
    }

    /// Waits until the session is connected; panics if it closes first.
    pub fn connected(&self) {
        loop {
            let state = self.next_state();
            match state["state"].as_str() {
                Some("connected") => return,
                Some("closed") => panic!("the session closed: {state}"),
                _ => {}
            }
        }
    }

    /// Waits until the session closes, and returns its closed state.
    pub fn closed(&self) -> Value {
        loop {
            let state = self.next_state();
            if state["state"] == "closed" {
                return state;
            }
        }
    }

    /// The next host key question.
    pub fn host_key_question(&self) -> Value {
        json_of(next(&self.host_keys, "a host key question"))
    }

    /// Reads output until `text` shows (escape sequences left out), and
    /// returns all the output so far, as text.
    pub fn output_showing(&mut self, text: &str) -> String {
        let until = Instant::now() + SESSION_WAIT;
        loop {
            let shown = String::from_utf8_lossy(&without_escapes(&self.seen)).into_owned();
            if shown.contains(text) {
                return shown;
            }
            let left = until.saturating_duration_since(Instant::now());
            let bytes = match self.output.recv_timeout(left) {
                Ok(InvokeResponseBody::Raw(bytes)) => bytes,
                Ok(InvokeResponseBody::Json(json)) => panic!("output as JSON: {json}"),
                Err(_) => panic!("{text:?} never showed; the output was {shown:?}"),
            };
            self.app.call(
                "session_acknowledge",
                json!({ "id": self.id, "consumer": self.consumer, "bytes": bytes.len() }),
            );
            self.seen.extend_from_slice(&bytes);
            let asked = self
                .seen
                .windows(CURSOR_QUERY.len())
                .filter(|part| *part == CURSOR_QUERY)
                .count();
            while self.answered < asked {
                self.send(CURSOR_REPORT, "reply");
                self.answered += 1;
            }
        }
    }

    /// Types `text` in the terminal.
    pub fn type_text(&self, text: &str) {
        self.send(text.as_bytes(), "typed");
    }

    fn send(&self, bytes: &[u8], origin: &str) {
        self.app.call(
            "session_input",
            json!({ "id": self.id, "data": bytes, "origin": origin, "paste": null }),
        );
    }

    /// Closes the session, as closing its tab does.
    pub fn close(&self) {
        self.app.call("session_close", json!({ "id": self.id }));
    }
}

/// The next message on `channel`, which must come within [`SESSION_WAIT`].
fn next(channel: &Receiver<InvokeResponseBody>, what: &str) -> InvokeResponseBody {
    channel
        .recv_timeout(SESSION_WAIT)
        .unwrap_or_else(|_| panic!("{what} never came"))
}

fn json_of(body: InvokeResponseBody) -> Value {
    match body {
        InvokeResponseBody::Json(json) => serde_json::from_str(&json).unwrap(),
        InvokeResponseBody::Raw(bytes) => panic!("raw bytes where JSON was expected: {bytes:?}"),
    }
}

/// `bytes` without the terminal's escape sequences (CSI and OSC), which
/// ConPTY puts between the characters of what a program prints.
fn without_escapes(bytes: &[u8]) -> Vec<u8> {
    let mut text = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        if byte != 0x1b {
            text.push(byte);
            continue;
        }
        match rest.split_first() {
            // CSI: parameters, then a final byte from `@` to `~`.
            Some((b'[', after)) => {
                let end = after.iter().position(|b| (0x40..=0x7e).contains(b));
                rest = end.map_or(&[][..], |end| &after[end + 1..]);
            }
            // OSC: up to BEL or ST (`ESC \`).
            Some((b']', after)) => {
                let end = after.iter().enumerate().find(|(i, b)| {
                    **b == 0x07 || (**b == 0x1b && after.get(i + 1) == Some(&b'\\'))
                });
                rest = match end {
                    Some((i, 0x07)) => &after[i + 1..],
                    Some((i, _)) => &after[i + 2..],
                    None => &[],
                };
            }
            // Any other escape: the one byte after it.
            Some((_, after)) => rest = after,
            None => {}
        }
    }
    text
}
