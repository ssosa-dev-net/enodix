//! The session logger: writes what sessions show to plain text files.
//!
//! Logs live under a per-user log folder ([`LogFolder`]), never in a
//! workspace, so they cannot end up in a shared repository. A workspace
//! host's logs go to `workspaces/<workspace>/<tree path>/<host>/`, a quick
//! connection's to `quick connect/<address or port>/`; each connection of a
//! session writes one file named after its local start time,
//! `YYYY-MM-DD_HH-MM-SS.log`. Whether a host is logged follows its resolved
//! `logging` setting (on by default).
//!
//! The text is what the terminal shows, line by line: terminal control
//! sequences are dropped, and backspaces, carriage returns and erasing
//! within a line are applied ([`PlainText`]). Logs can hold secrets the
//! device shows on screen.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::workspace::{forbidden_in_file_names, is_device_name, HostId, Workspace};

/// The folder below the log folder for workspace hosts' logs.
const WORKSPACES: &str = "workspaces";

/// The folder below the log folder for quick connections' logs.
const QUICK_CONNECT: &str = "quick connect";

/// The longest line a log keeps; longer ones go on in the next line. Also
/// bounds how far device output can move the cursor (`ESC [ 999999999 C`).
const MAX_COLUMNS: usize = 4096;

/// The per-user folder all session logs go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFolder {
    root: PathBuf,
}

impl LogFolder {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder the logs of the workspace host `id` go to:
    /// `workspaces/<workspace folder name>/<tree path>/<host file name>`.
    /// `None` if the workspace has no such host.
    pub fn host_folder(&self, workspace: &Workspace, id: &HostId) -> Option<PathBuf> {
        workspace.host(id)?;
        let mut folder = self.root.join(WORKSPACES).join(workspace_name(workspace));
        let (parents, file) = id.as_str().rsplit_once('/').unwrap_or(("", id.as_str()));
        for part in parents.split('/').filter(|part| !part.is_empty()) {
            folder.push(portable(part));
        }
        let host = Path::new(file)
            .file_stem()
            .map_or_else(|| file.into(), |stem| stem.to_string_lossy());
        folder.push(portable(&host));
        Some(folder)
    }

    /// Creates the host's log folder if it is not there yet (to show it
    /// before any session wrote to it) and returns it.
    pub fn create_host_folder(&self, workspace: &Workspace, id: &HostId) -> io::Result<PathBuf> {
        let folder = self.host_folder(workspace, id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{id} is not in the workspace"),
            )
        })?;
        fs::create_dir_all(&folder)?;
        Ok(folder)
    }

    /// Where sessions to the workspace host `id` are logged; `None` if its
    /// resolved `logging` setting is off (or there is no such host).
    pub fn host_log(&self, workspace: &Workspace, id: &HostId) -> Option<SessionLog> {
        let resolved = workspace.resolve_host(id)?;
        if !resolved.settings.logging.value {
            return None;
        }
        Some(SessionLog {
            folder: self.host_folder(workspace, id)?,
        })
    }

    /// Where a session to something outside any workspace (a quick
    /// connection) is logged: `quick connect/<name>`, `name` being the
    /// address or serial port, made a portable folder name.
    pub fn quick_log(&self, name: &str) -> SessionLog {
        SessionLog {
            folder: self.root.join(QUICK_CONNECT).join(portable(name)),
        }
    }
}

/// Where one session is logged: a folder that gets a file per connection.
/// Nothing is written until the session connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLog {
    folder: PathBuf,
}

impl SessionLog {
    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

/// A session log could not be written; the session goes on, unlogged from
/// then on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the session log {} could not be written ({reason}); the rest of this session is not logged", .path.display())]
pub struct LogFailure {
    /// The log file, or the folder that could not be created.
    pub path: PathBuf,
    pub reason: String,
}

impl LogFailure {
    fn new(path: &Path, error: &io::Error) -> Self {
        Self {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    }
}

/// Writes one connection's output to its log file.
pub(crate) struct Logger {
    file: File,
    path: PathBuf,
    text: PlainText,
}

impl Logger {
    /// Creates the log file for a connection starting now.
    pub(crate) fn start(log: &SessionLog) -> Result<Self, LogFailure> {
        fs::create_dir_all(&log.folder).map_err(|error| LogFailure::new(&log.folder, &error))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let mut attempt = 1;
        loop {
            let name = match attempt {
                1 => format!("{stamp}.log"),
                n => format!("{stamp}_{n}.log"),
            };
            let path = log.folder.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        path,
                        text: PlainText::new(),
                    })
                }
                // Another connection started in the same second.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 1000 => {
                    attempt += 1;
                }
                Err(error) => return Err(LogFailure::new(&path, &error)),
            }
        }
    }

    /// Logs output: the lines it completes are written at once.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<(), LogFailure> {
        let lines = self.text.push(bytes);
        self.write_text(&lines)
    }

    /// Writes the unfinished last line and closes the file.
    pub(crate) fn finish(mut self) -> Result<(), LogFailure> {
        let rest = self.text.finish();
        self.write_text(&rest)?;
        self.file
            .sync_all()
            .map_err(|error| LogFailure::new(&self.path, &error))
    }

    fn write_text(&mut self, text: &str) -> Result<(), LogFailure> {
        if text.is_empty() {
            return Ok(());
        }
        self.file
            .write_all(text.as_bytes())
            .map_err(|error| LogFailure::new(&self.path, &error))
    }
}

/// Turns terminal output into plain text lines, as the terminal would show
/// each line: control sequences (colours, cursor movement, titles, modes)
/// are dropped; backspace and carriage return move within the line, so
/// that what is typed over replaces what was there; erase-in-line and
/// cursor left/right/column are applied. Trailing blanks are dropped and
/// lines end with `\n`. Bytes that are not UTF-8 become U+FFFD.
#[derive(Debug, Default)]
pub struct PlainText {
    line: Vec<char>,
    cursor: usize,
    /// The start of a UTF-8 character whose other bytes have not come yet.
    pending: Vec<u8>,
    escape: Escape,
}

/// Where the text is within a control sequence.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// After ESC.
    Esc,
    /// After ESC and intermediate bytes (such as `ESC (` choosing a charset).
    Intermediate,
    /// A control sequence (`ESC [`), with its parameter bytes so far.
    Csi(String),
    /// A string (OSC `ESC ]`, ended by BEL or ST; DCS, SOS, PM, APC ended
    /// by ST), and whether an ESC just came.
    Text { osc: bool, esc: bool },
}

impl PlainText {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes more output and returns the lines it completed, each ending
    /// with `\n` (empty if none).
    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut out = String::new();
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(bytes);
        let mut rest = &input[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.text(text, &mut out);
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    // Checked valid by `from_utf8`.
                    self.text(std::str::from_utf8(valid).unwrap_or(""), &mut out);
                    match error.error_len() {
                        Some(len) => {
                            self.char(char::REPLACEMENT_CHARACTER, &mut out);
                            rest = &after[len..];
                        }
                        None => {
                            // A character cut short: wait for the rest.
                            self.pending = after.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// The unfinished last line, ended with `\n`, if it shows anything;
    /// the text then starts afresh.
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.pending.is_empty() {
            self.pending.clear();
            self.char(char::REPLACEMENT_CHARACTER, &mut out);
        }
        self.escape = Escape::None;
        let line = self.take_line();
        if !line.is_empty() {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    fn text(&mut self, text: &str, out: &mut String) {
        for c in text.chars() {
            self.char(c, out);
        }
    }

    fn char(&mut self, c: char, out: &mut String) {
        match std::mem::take(&mut self.escape) {
            Escape::None => self.plain(c, out),
            Escape::Esc => {
                self.escape = match c {
                    '[' => Escape::Csi(String::new()),
                    ']' => Escape::Text {
                        osc: true,
                        esc: false,
                    },
                    'P' | 'X' | '^' | '_' => Escape::Text {
                        osc: false,
                        esc: false,
                    },
                    '\u{20}'..='\u{2f}' => Escape::Intermediate,
                    // A single-character sequence (keypad mode, save cursor,
                    // reverse index, ...).
                    _ => Escape::None,
                }
            }
            Escape::Intermediate => {
                if matches!(c, '\u{20}'..='\u{2f}') {
                    self.escape = Escape::Intermediate;
                }
            }
            Escape::Csi(mut params) => match c {
                '\u{20}'..='\u{3f}' => {
                    params.push(c);
                    self.escape = Escape::Csi(params);
                }
                '\u{40}'..='\u{7e}' => self.control(c, &params),
                // ESC breaks the sequence off and starts another.
                '\u{1b}' => self.escape = Escape::Esc,
                // Another control character still acts, as in a terminal,
                // and the sequence goes on.
                c if c < '\u{20}' => {
                    self.plain(c, out);
                    self.escape = Escape::Csi(params);
                }
                // Anything else breaks the sequence off.
                _ => {}
            },
            Escape::Text { osc, esc } => {
                if esc {
                    // ST (`ESC \`) ends it; another ESC starts a sequence.
                    if c != '\\' {
                        self.escape = Escape::Esc;
                        self.char(c, out);
                    }
                } else if c == '\u{1b}' {
                    self.escape = Escape::Text { osc, esc: true };
                } else if !(osc && c == '\u{7}') && c != '\u{9c}' {
                    self.escape = Escape::Text { osc, esc: false };
                }
            }
        }
    }

    /// A character outside any sequence.
    fn plain(&mut self, c: char, out: &mut String) {
        match c {
            '\u{1b}' => self.escape = Escape::Esc,
            // The 8-bit forms of CSI, OSC and the other strings.
            '\u{9b}' => self.escape = Escape::Csi(String::new()),
            '\u{9d}' => {
                self.escape = Escape::Text {
                    osc: true,
                    esc: false,
                }
            }
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                self.escape = Escape::Text {
                    osc: false,
                    esc: false,
                }
            }
            '\n' => self.end_line(out),
            '\r' => self.cursor = 0,
            '\u{8}' => self.cursor = self.cursor.saturating_sub(1),
            '\t' => self.put('\t', out),
            c if c.is_control() => {}
            c => self.put(c, out),
        }
    }

    /// Ends the current line.
    fn end_line(&mut self, out: &mut String) {
        out.push_str(&self.take_line());
        out.push('\n');
    }

    /// A control sequence's final character.
    fn control(&mut self, last: char, params: &str) {
        // Private sequences (`ESC [ ? ...`) only set modes.
        if params.starts_with(['?', '<', '=', '>']) {
            return;
        }
        let count = || {
            params
                .split(';')
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(1)
        };
        match last {
            // Device output decides the counts: the cursor stays within
            // the longest line kept.
            'C' => self.cursor = (self.cursor + count()).min(MAX_COLUMNS - 1),
            'D' => self.cursor = self.cursor.saturating_sub(count()),
            'G' => self.cursor = count().min(MAX_COLUMNS) - 1,
            // Erase characters: blanks over them.
            'X' => {
                let end = (self.cursor + count()).min(self.line.len());
                if self.cursor < end {
                    self.line[self.cursor..end].fill(' ');
                }
            }
            // Delete characters: the rest of the line moves left.
            'P' => {
                if self.cursor < self.line.len() {
                    let end = (self.cursor + count()).min(self.line.len());
                    self.line.drain(self.cursor..end);
                }
            }
            // Insert blanks: the rest of the line moves right.
            '@' => {
                if self.cursor < self.line.len() {
                    let blanks = count().min(MAX_COLUMNS - self.cursor);
                    let at = self.cursor;
                    self.line.splice(at..at, std::iter::repeat_n(' ', blanks));
                    self.line.truncate(MAX_COLUMNS);
                }
            }
            'K' => match params {
                "" | "0" => self.line.truncate(self.cursor),
                "1" => {
                    let end = (self.cursor + 1).min(self.line.len());
                    self.line[..end].fill(' ');
                }
                "2" => self.line.clear(),
                _ => {}
            },
            _ => {}
        }
    }

    /// Writes `c` at the cursor, over what is there. A line reaching
    /// [`MAX_COLUMNS`] goes on in the next.
    fn put(&mut self, c: char, out: &mut String) {
        if self.cursor >= MAX_COLUMNS {
            self.end_line(out);
        }
        if self.cursor > self.line.len() {
            self.line.resize(self.cursor, ' ');
        }
        if self.cursor == self.line.len() {
            self.line.push(c);
        } else {
            self.line[self.cursor] = c;
        }
        self.cursor += 1;
    }

    /// The current line without trailing blanks; the next starts empty.
    fn take_line(&mut self) -> String {
        let line: String = std::mem::take(&mut self.line).into_iter().collect();
        self.cursor = 0;
        line.trim_end().to_string()
    }
}

/// `name` as a file name every system accepts: characters file names
/// cannot hold become `_`, as does a trailing dot or space; a Windows
/// device name gets a leading `_`; an empty name is `_`.
fn portable(name: &str) -> String {
    let mut name: String = name
        .trim()
        .chars()
        .map(|c| if forbidden_in_file_names(c) { '_' } else { c })
        .collect();
    if name.ends_with('.') {
        name.pop();
        name.push('_');
    }
    if name.is_empty() {
        return "_".into();
    }
    if is_device_name(&name) {
        name.insert(0, '_');
    }
    name
}

/// The workspace's name in log paths: its folder's name.
fn workspace_name(workspace: &Workspace) -> String {
    let name = workspace
        .root()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() {
        "workspace".into()
    } else {
        portable(&name)
    }
}
