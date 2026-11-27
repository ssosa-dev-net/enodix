//! The local transport (#22): a shell on this computer, on a pseudo
//! terminal (ConPTY on Windows, a pty on Unix), through the `portable-pty`
//! crate.
//!
//! A local shell shows the user's own computer, not a device: it is the one
//! session that is not logged unless asked, never takes part in multi-send
//! (the layout decides that from [`Transport::Local`]) and is skipped by
//! jobs, config backups and the Files tab, which only reach devices.
//!
//! A pty has a blocking read with no timeout, so, like the serial
//! transport, a reader thread feeds the async runtime once flow control has
//! room ([`Flow::room_to_read`]), and a writer thread takes typed input in
//! order. The reader is never waited for on close: ending the shell (or
//! dropping the pty) ends its read.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tokio::sync::mpsc;

use super::output::Flow;
use super::serial::wait_on_thread;
use super::{CloseReason, Command, Link, OpenError, SessionState, TerminalSize, TerminalType};

/// How much is read from the pty at a time.
const READ_SIZE: usize = 4096;

/// How long the reader waits for flow control before looking again at
/// whether the session still listens.
const READ_POLL: Duration = Duration::from_millis(100);

/// How long a shell that ended may still be printing what it wrote last.
const DRAIN: Duration = Duration::from_millis(300);

/// How long the end of the pty's output waits for the shell's exit status.
const EXIT_WAIT: Duration = Duration::from_secs(1);

/// How long closing waits for the shell to end after it was killed.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The environment variable that tells a program in a local shell which
/// workspace is open (`ENODIX_WORKSPACE`), so that the Python bridge finds
/// its inventory with nothing to set up.
pub const WORKSPACE_VARIABLE: &str = "ENODIX_WORKSPACE";

/// The environment variable naming the headless binary (`ENODIX_CLI`).
pub const CLI_VARIABLE: &str = "ENODIX_CLI";

/// Where a local shell starts: the open workspace's folder, else the home
/// folder (`HOME`, `USERPROFILE` on Windows), else where the app is.
pub fn local_start_folder(workspace: Option<&Path>) -> Option<PathBuf> {
    workspace.map(Path::to_path_buf).or_else(|| {
        ["HOME", "USERPROFILE"]
            .into_iter()
            .filter_map(std::env::var_os)
            .find(|home| !home.is_empty())
            .map(PathBuf::from)
    })
}

/// The environment of a local shell: `extra` (what the app needs the
/// programs it starts to know, such as its data folder), the open
/// workspace as [`WORKSPACE_VARIABLE`] and the headless binary as
/// [`CLI_VARIABLE`], so that the Python bridge works in it with nothing to
/// set up.
pub fn local_environment(
    workspace: Option<&Path>,
    cli: Option<&Path>,
    extra: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut environment = extra;
    if let Some(workspace) = workspace {
        environment.push((WORKSPACE_VARIABLE.into(), workspace.display().to_string()));
    }
    if let Some(cli) = cli {
        environment.push((CLI_VARIABLE.into(), cli.display().to_string()));
    }
    environment
}

/// A program to run on a pseudo terminal, and where and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTarget {
    /// The program: a path or a name found on the `PATH`.
    pub program: String,
    pub args: Vec<String>,
    /// Where it starts: the workspace folder when one is open, else the
    /// home folder (`None`: the folder the app was started in).
    pub folder: Option<PathBuf>,
    /// Variables added to the program's environment.
    pub environment: Vec<(String, String)>,
}

/// A shell the user can open, as the list of local shells shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalShell {
    /// What a preference and [`LocalShell::target`] name it by: stable
    /// (`powershell`, `cmd`, `wsl:Ubuntu`, `login`, `/bin/zsh`).
    pub id: String,
    /// What the interface shows: `PowerShell`, `Ubuntu (WSL)`.
    pub name: String,
    #[serde(skip)]
    program: String,
    #[serde(skip)]
    args: Vec<String>,
}

impl LocalShell {
    fn new(id: &str, name: &str, program: &str, args: &[&str]) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            program: program.into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
        }
    }

    /// The shell as a target, starting in `folder` with `environment`.
    pub fn target(
        &self,
        folder: Option<PathBuf>,
        environment: Vec<(String, String)>,
    ) -> LocalTarget {
        LocalTarget {
            program: self.program.clone(),
            args: self.args.clone(),
            folder,
            environment,
        }
    }
}

/// The shells this computer has, the first being what an unset preference
/// opens: on Windows PowerShell (`pwsh` if installed, else Windows
/// PowerShell), cmd and each installed WSL distribution; elsewhere the
/// user's login shell (`$SHELL`) and the others `/etc/shells` lists.
pub fn local_shells() -> Vec<LocalShell> {
    if cfg!(windows) {
        windows_shells()
    } else {
        unix_shells(
            std::env::var("SHELL").ok().as_deref(),
            &std::fs::read_to_string("/etc/shells").unwrap_or_default(),
            Path::exists,
        )
    }
}

/// The shell the preference `chosen` (an id, `None` or empty for none)
/// names among `shells`, else the first one.
pub fn pick_local_shell<'a>(
    shells: &'a [LocalShell],
    chosen: Option<&str>,
) -> Option<&'a LocalShell> {
    chosen
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .and_then(|id| {
            shells
                .iter()
                .find(|shell| shell.id.eq_ignore_ascii_case(id))
        })
        .or_else(|| shells.first())
}

/// The Unix shells: `$SHELL` first (named `login`, the user's own), then
/// each other existing one of `/etc/shells` (named by its path).
fn unix_shells(
    login: Option<&str>,
    etc_shells: &str,
    exists: impl Fn(&Path) -> bool,
) -> Vec<LocalShell> {
    let mut shells = Vec::new();
    let login = login.map(str::trim).filter(|path| !path.is_empty());
    if let Some(path) = login {
        shells.push(LocalShell::new(
            "login",
            &shell_name(path, true),
            path,
            &["-l"],
        ));
    }
    for path in etc_shells
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/') && !line.contains('#'))
    {
        if Some(path) == login || !exists(Path::new(path)) {
            continue;
        }
        if shells.iter().any(|shell| shell.id == path) {
            continue;
        }
        // `nologin` and `false` are in the list but are no shell.
        let name = shell_name(path, false);
        if name == "nologin" || name == "false" {
            continue;
        }
        shells.push(LocalShell::new(path, &name, path, &["-l"]));
    }
    if shells.is_empty() {
        shells.push(LocalShell::new("/bin/sh", "sh", "/bin/sh", &[]));
    }
    shells
}

fn shell_name(path: &str, login: bool) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if login {
        format!("{name} (login shell)")
    } else {
        name.into()
    }
}

fn windows_shells() -> Vec<LocalShell> {
    let mut shells = Vec::new();
    if on_path("pwsh.exe") {
        shells.push(LocalShell::new(
            "pwsh",
            "PowerShell",
            "pwsh.exe",
            &["-NoLogo"],
        ));
        shells.push(LocalShell::new(
            "powershell",
            "Windows PowerShell",
            "powershell.exe",
            &["-NoLogo"],
        ));
    } else {
        shells.push(LocalShell::new(
            "powershell",
            "PowerShell",
            "powershell.exe",
            &["-NoLogo"],
        ));
    }
    shells.push(LocalShell::new("cmd", "Command Prompt", "cmd.exe", &[]));
    for distribution in wsl_distributions() {
        shells.push(LocalShell::new(
            &format!("wsl:{distribution}"),
            &format!("{distribution} (WSL)"),
            "wsl.exe",
            &["-d", &distribution],
        ));
    }
    shells
}

/// Whether `program` is in a folder of the `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|folder| folder.join(program).is_file()))
        .unwrap_or(false)
}

/// The installed WSL distributions (`wsl.exe -l -q`, which writes UTF-16).
/// None if WSL is not installed or answers badly.
fn wsl_distributions() -> Vec<String> {
    let mut command = std::process::Command::new("wsl.exe");
    command.args(["-l", "-q"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    match command.output() {
        Ok(output) if output.status.success() => parse_wsl_list(&output.stdout),
        _ => Vec::new(),
    }
}

/// The names in the output of `wsl.exe -l -q`: UTF-16 with a byte order
/// mark, one per line (plain text is accepted too, for newer versions).
fn parse_wsl_list(output: &[u8]) -> Vec<String> {
    let utf16 = output.starts_with(&[0xFF, 0xFE]) || (output.len() >= 2 && output[1] == 0);
    let text = if utf16 && output.len().is_multiple_of(2) {
        let units: Vec<u16> = output
            .chunks(2)
            .filter(|pair| pair.len() == 2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(output).into_owned()
    };
    text.lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect()
}

/// What the writer thread does, in order.
enum Job {
    Write(Vec<u8>),
}

/// Runs one local session over `link` until it ends.
pub(super) async fn run(target: LocalTarget, link: &mut Link) -> CloseReason {
    let program = target.program.trim().to_string();
    let failed = |reason: String| {
        CloseReason::Failed(OpenError::LocalShell {
            program: program.clone(),
            reason,
        })
    };
    if program.is_empty() {
        return failed("no program was given".into());
    }

    let size = pty_size(link.size);
    let spawn_target = target.clone();
    let spawn_program = program.clone();
    let opening = tokio::task::spawn_blocking(move || open(&spawn_program, &spawn_target, size));
    let opened = match opening.await {
        Ok(Ok(opened)) => opened,
        Ok(Err(error)) => return failed(open_failure(&error)),
        Err(error) => return failed(error.to_string()),
    };
    let Opened {
        master,
        reader,
        writer,
        child,
    } = opened;
    let master = Arc::new(Mutex::new(master));
    let mut killer = child.clone_killer();

    // The shell ending is watched on its own, not through the pty: on
    // Windows ConPTY's output does not end when the child exits, only when
    // the pseudo console is closed (which happens when `master` drops).
    let (exit_tx, mut exit) = mpsc::unbounded_channel();
    let spawned = thread::Builder::new()
        .name("local waiter".into())
        .spawn(move || {
            let mut child = child;
            let status = child.wait().ok().map(|status| status.exit_code());
            let _ = exit_tx.send(status);
        });
    if let Err(error) = spawned {
        let _ = killer.kill();
        return failed(format!("cannot watch it: {error}"));
    }

    let (output_tx, mut output) = mpsc::unbounded_channel();
    let flow = link.flow();
    let spawned = thread::Builder::new()
        .name("local reader".into())
        .spawn(move || read_pty(reader, &flow, &output_tx));
    if let Err(error) = spawned {
        let _ = killer.kill();
        return failed(format!("cannot start reading it: {error}"));
    }
    let (jobs, queued) = std_mpsc::channel();
    let (write_failed_tx, mut write_failed) = mpsc::unbounded_channel();
    let spawned = thread::Builder::new()
        .name("local writer".into())
        .spawn(move || write_pty(writer, &queued, &write_failed_tx));
    if let Err(error) = spawned {
        let _ = killer.kill();
        return failed(format!("cannot start writing to it: {error}"));
    }

    link.state(SessionState::Connected);
    // The shell's exit status once it has ended (`Some(None)`: it ended
    // without one).
    let mut ended: Option<Option<u32>> = None;
    let reason = loop {
        tokio::select! {
            command = link.command() => match command {
                Command::Write(bytes) => {
                    let _ = jobs.send(Job::Write(bytes));
                }
                Command::Resize(size) => {
                    let master = master.lock().unwrap_or_else(PoisonError::into_inner);
                    let _ = master.resize(pty_size(size));
                }
                // No login questions, no breaks, no second channels.
                Command::Answer(_)
                | Command::HostKey(_)
                | Command::AgentForwarding(_)
                | Command::Break
                | Command::OpenChannel { .. }
                | Command::Transfer { .. }
                | Command::CancelTransfer => {}
                Command::Close => break CloseReason::ClosedLocally,
            },
            read = output.recv() => match read {
                Some(bytes) => {
                    if !link.forward_counted(bytes) {
                        break CloseReason::ClosedLocally;
                    }
                }
                // The read ended: the shell did, or the pty broke.
                None => {
                    let status = match ended {
                        Some(status) => status,
                        None => tokio::time::timeout(EXIT_WAIT, exit.recv())
                            .await
                            .ok()
                            .flatten()
                            .flatten(),
                    };
                    break CloseReason::LocalExited { exit_status: status };
                }
            },
            status = exit.recv(), if ended.is_none() => {
                ended = Some(status.flatten());
                // What it printed last may still be on its way.
                let deadline = tokio::time::Instant::now() + DRAIN;
                let mut listening = true;
                while listening {
                    match tokio::time::timeout_at(deadline, output.recv()).await {
                        Ok(Some(bytes)) => listening = link.forward_counted(bytes),
                        _ => break,
                    }
                }
                break CloseReason::LocalExited { exit_status: ended.flatten() };
            }
            Some(error) = write_failed.recv() => {
                break CloseReason::Failed(OpenError::LocalShell {
                    program: program.clone(),
                    reason: error.to_string(),
                });
            }
        }
    };

    drop(jobs);
    drop(output);
    // Wait for the shell to be gone (so a folder it held can be removed),
    // but never for the reader: a ConPTY read can outlive its child.
    if ended.is_none() {
        let _ = killer.kill();
        let _ = tokio::time::timeout(CLOSE_WAIT, exit.recv()).await;
    }
    // Dropping the master closes the pty and ends the reader's read.
    drop(master);
    reason
}

struct Opened {
    master: Box<dyn MasterPty + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

fn open(program: &str, target: &LocalTarget, size: PtySize) -> io::Result<Opened> {
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut command = CommandBuilder::new(program);
    command.args(&target.args);
    command.env("TERM", TerminalType::default().as_str());
    for (name, value) in &target.environment {
        command.env(name, value);
    }
    if let Some(folder) = target.folder.as_ref().filter(|folder| folder.is_dir()) {
        command.cwd(folder);
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| io::Error::other(error.to_string()))?;
    // The slave side stays with the child only.
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(Opened {
        master: pair.master,
        reader,
        writer,
        child,
    })
}

fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows.clamp(1, u32::from(u16::MAX)) as u16,
        cols: size.cols.clamp(1, u32::from(u16::MAX)) as u16,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Why a program could not be started, with a hint for the usual cause.
fn open_failure(error: &io::Error) -> String {
    let text = error.to_string();
    if text.to_lowercase().contains("no such file")
        || text.to_lowercase().contains("cannot find")
        || text.to_lowercase().contains("not found")
    {
        format!("{text} (is the shell installed, and is it on the PATH?)")
    } else {
        text
    }
}

/// The reader thread: reads the pty whenever flow control has room and
/// passes what came on (counted in `flow` at once, so the window is exact),
/// until the session stops listening or the read ends.
fn read_pty(
    mut reader: Box<dyn Read + Send>,
    flow: &Flow,
    output: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let mut buffer = vec![0; READ_SIZE];
    loop {
        loop {
            let room = wait_on_thread(
                async {
                    tokio::select! {
                        () = flow.room_to_read() => true,
                        () = output.closed() => false,
                    }
                },
                READ_POLL,
            );
            match room {
                Some(false) => return,
                Some(true) => break,
                None => {}
            }
        }
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                flow.read(count);
                if output.send(buffer[..count].to_vec()).is_err() {
                    return;
                }
            }
        }
    }
}

/// The writer thread: writes typed input in order, until the queue closes;
/// reports a failing pty on `failed`.
fn write_pty(
    mut writer: Box<dyn Write + Send>,
    queued: &std_mpsc::Receiver<Job>,
    failed: &mpsc::UnboundedSender<io::Error>,
) {
    while let Ok(Job::Write(bytes)) = queued.recv() {
        if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
            let _ = failed.send(error);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_shells_put_the_login_shell_first() {
        let shells = unix_shells(
            Some("/bin/zsh"),
            "# list\n/bin/sh\n/bin/zsh\n/bin/bash\n/usr/sbin/nologin\n/gone/fish\n",
            |path| !path.starts_with("/gone"),
        );
        let ids: Vec<_> = shells.iter().map(|shell| shell.id.as_str()).collect();
        assert_eq!(ids, ["login", "/bin/sh", "/bin/bash"]);
        assert_eq!(shells[0].name, "zsh (login shell)");
    }

    #[test]
    fn unix_shells_always_offer_something() {
        let shells = unix_shells(None, "", |_| false);
        assert_eq!(shells.len(), 1);
        assert_eq!(shells[0].id, "/bin/sh");
    }

    #[test]
    fn the_preference_picks_a_shell_or_the_first() {
        let shells = unix_shells(Some("/bin/zsh"), "/bin/bash\n", |_| true);
        assert_eq!(
            pick_local_shell(&shells, Some("/BIN/bash")).unwrap().id,
            "/bin/bash"
        );
        assert_eq!(pick_local_shell(&shells, Some("gone")).unwrap().id, "login");
        assert_eq!(pick_local_shell(&shells, Some("  ")).unwrap().id, "login");
        assert_eq!(pick_local_shell(&shells, None).unwrap().id, "login");
    }

    #[test]
    fn the_environment_names_the_workspace_and_the_binary_when_there_are_some() {
        let workspace = Path::new("/work/net");
        let cli = Path::new("/app/enodix-cli");
        let environment = local_environment(
            Some(workspace),
            Some(cli),
            vec![("ENODIX_DATA_DIR".into(), "/data".into())],
        );
        assert_eq!(
            environment,
            [
                ("ENODIX_DATA_DIR".to_string(), "/data".to_string()),
                ("ENODIX_WORKSPACE".to_string(), "/work/net".to_string()),
                ("ENODIX_CLI".to_string(), "/app/enodix-cli".to_string()),
            ]
        );
        assert!(local_environment(None, None, Vec::new()).is_empty());
    }

    #[test]
    fn a_shell_starts_in_the_workspace_else_at_home() {
        let workspace = Path::new("/work/net");
        assert_eq!(
            local_start_folder(Some(workspace)),
            Some(workspace.to_path_buf())
        );
        // Without a workspace it is the home folder, whatever the machine.
        assert_eq!(
            local_start_folder(None),
            ["HOME", "USERPROFILE"]
                .into_iter()
                .filter_map(std::env::var_os)
                .find(|home| !home.is_empty())
                .map(PathBuf::from)
        );
    }

    #[test]
    fn wsl_lists_are_read_as_utf16_or_plain_text() {
        let utf16: Vec<u8> = "\u{feff}Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_wsl_list(&utf16), ["Ubuntu", "Debian"]);
        assert_eq!(parse_wsl_list(b"Ubuntu\nDebian\n"), ["Ubuntu", "Debian"]);
        assert!(parse_wsl_list(b"").is_empty());
    }
}
