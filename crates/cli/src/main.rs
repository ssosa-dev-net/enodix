//! The headless binary: the core from the command line, with no window. It
//! needs no running app, but asks it for credentials while it runs (see
//! `credentials`). Its command line is a contract (the Python bridge
//! builds on it, `tests/` checks it); see `usage` and the cli notes.
//!
//! Exit status: 0 done, 1 refused or failed (the reason on stderr), 2 a
//! command line it does not understand.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use netterm_core::app_state::{resolved_path, same_folder};
use netterm_core::inventory::{export_within, InventoryExport, Limit};
use netterm_core::job::script::{ScriptTargets, TARGETS_VARIABLE};
use netterm_core::workspace::Workspace;

mod credentials;
mod tunnel;

use credentials::PRODUCT;

pub(crate) const NAME: &str = env!("CARGO_BIN_NAME");

fn usage() -> String {
    format!(
        "\
Usage:
  {NAME} inventory <workspace folder> [--folder <path> | --filter <name> | --host <id>...]
  {NAME} credentials <workspace folder> <host id>
  {NAME} secret <workspace folder> <name>
  {NAME} tunnel <workspace folder> [--folder <path> | --filter <name> | --host <id>...]
                [--port <number>...]
  {NAME} --help | --version

inventory   prints the workspace's hosts as JSON, each with the settings that
            apply to it, its jump hosts, folders, tags and custom fields (no
            secrets). Limit it to the hosts in a folder and below
            (--folder), to a saved filter's (--filter), or to host ids
            (--host, repeated). Problems with workspace files go to stderr
            and into the JSON's \"problems\"; they do not stop the export.
credentials prints, as JSON, the login of the credential profile a host
            uses: username, then password, private key file and passphrase,
            or nothing for the SSH agent; and any enable secret.
secret      prints, as JSON, the value of the workspace's named secret.
tunnel      opens a loopback tunnel to each host behind jump hosts (in the
            same limits as inventory), through its jump chain as {PRODUCT}
            would connect, with the jump hosts' stored logins and the host
            keys {PRODUCT} trusts; it asks no host-key or login question
            (only the vault's master password, as for credentials). Prints one JSON line: for
            each such host, the 127.0.0.1 port that reaches its SSH port
            and its trusted host keys, or why it is not tunnelled. Then
            serves the ports, reporting failures on stderr, until its
            standard input ends: keep it open while the tunnels are used.
            Each --port (repeated, 1 to 65535: NETCONF's 830, an API's
            443) is a device port tunnelled too, for every host, through
            the same jump hosts; a listening host then has \"extra_ports\",
            from device port to the 127.0.0.1 port that reaches it. Nothing
            checks that the device listens on it.

Credentials come from {PRODUCT} while it runs and its vault, if it has
one, is unlocked. Otherwise they are read here: from the keychain, or from
the vault after asking for its master password on the terminal. With a
vault and neither {PRODUCT} unlocked nor a terminal, they are refused.

Run by a script job of {PRODUCT} ({TARGETS_VARIABLE} names the job's
hosts), every command reads only that job's workspace, inventory shows only
the job's hosts, and credentials are given only for them (the workspace's
named secrets stay readable): a guard against mistakes, not a sandbox.
"
    )
}

/// What went wrong, and the exit status it gives.
pub(crate) enum Failure {
    /// The command line makes no sense: status 2, with a hint.
    Usage(String),
    /// The command was understood but cannot be done: status 1.
    Error(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("{NAME}: {message}\nRun `{NAME} --help` for how to use it.");
            ExitCode::from(2)
        }
        Err(Failure::Error(message)) => {
            eprintln!("{NAME}: error: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<(), Failure> {
    match args.first().map(String::as_str) {
        None => Err(Failure::Usage("no command given".into())),
        Some("--help" | "-h" | "help") => print(&usage()),
        Some("--version" | "-V") => print(&format!("{NAME} {}\n", env!("CARGO_PKG_VERSION"))),
        Some("inventory") => inventory(&args[1..]),
        Some("credentials") => credentials::host(&args[1..]),
        Some("secret") => credentials::secret(&args[1..]),
        Some("tunnel") => tunnel::run(&args[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown command {other:?}"))),
    }
}

/// `inventory <workspace> [limit]`.
fn inventory(args: &[String]) -> Result<(), Failure> {
    let (_, exported) = scoped_export(args)?;
    for problem in &exported.problems {
        eprintln!("{NAME}: warning: {problem}");
    }
    let mut json = serde_json::to_string_pretty(&exported)
        .map_err(|error| Failure::Error(format!("cannot write the inventory: {error}")))?;
    json.push('\n');
    print(&json)
}

/// The workspace and the export of `<workspace> [limit]` (`args`), within
/// the script job's hosts if this runs for one.
pub(crate) fn scoped_export(args: &[String]) -> Result<(Workspace, InventoryExport), Failure> {
    let (root, limit) = workspace_and_limit(args)?;
    let workspace = Workspace::open(resolved_path(Path::new(root)))
        .map_err(|error| Failure::Error(error.to_string()))?;
    let scope = job_scope(&workspace)?;
    let exported = export_within(
        &workspace,
        &limit,
        scope.as_ref().map(|targets| targets.hosts.as_slice()),
    )
    .map_err(|error| Failure::Error(error.to_string()))?;
    Ok((workspace, exported))
}

/// `<workspace> [--folder <path> | --filter <name> | --host <id>...]`, as
/// `inventory` and `tunnel` take them.
pub(crate) fn workspace_and_limit(args: &[String]) -> Result<(&str, Limit), Failure> {
    let mut root: Option<&str> = None;
    let mut folder: Option<&str> = None;
    let mut filter: Option<&str> = None;
    let mut hosts: Vec<String> = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |option: &str| {
            args.next()
                .map(String::as_str)
                .filter(|value| !value.starts_with('-'))
                .ok_or_else(|| Failure::Usage(format!("{option} needs a value")))
        };
        match arg.as_str() {
            "--folder" => set_once(&mut folder, "--folder", value("--folder")?)?,
            "--filter" => set_once(&mut filter, "--filter", value("--filter")?)?,
            "--host" => hosts.push(value("--host")?.to_owned()),
            option if option.starts_with('-') => {
                return Err(Failure::Usage(format!("unknown option {option:?}")))
            }
            path if root.is_none() => root = Some(path),
            extra => return Err(Failure::Usage(format!("unexpected argument {extra:?}"))),
        }
    }
    let root = root.ok_or_else(|| Failure::Usage("no workspace folder given".into()))?;
    let limit = match (folder, filter, hosts.is_empty()) {
        (None, None, true) => Limit::All,
        (Some(folder), None, true) => Limit::Folder(folder.to_owned()),
        (None, Some(filter), true) => Limit::Filter(filter.to_owned()),
        (None, None, false) => Limit::Hosts(hosts),
        _ => {
            return Err(Failure::Usage(
                "give only one of --folder, --filter and --host".into(),
            ))
        }
    };
    Ok((root, limit))
}

/// The hosts of the script job this process runs for, if it runs for one
/// ([`TARGETS_VARIABLE`] set); refused for another workspace than the
/// job's, or when the job's file cannot be read (never more than the job's
/// hosts by mistake).
pub(crate) fn job_scope(workspace: &Workspace) -> Result<Option<ScriptTargets>, Failure> {
    let Some(targets) = ScriptTargets::from_environment() else {
        return Ok(None);
    };
    let targets = targets.map_err(|error| Failure::Error(error.to_string()))?;
    if !same_folder(&targets.workspace, workspace.root()) {
        return Err(Failure::Error(format!(
            "this runs for a script job of the workspace {}, and reads no other workspace",
            targets.workspace.display()
        )));
    }
    Ok(Some(targets))
}

fn set_once<'a>(slot: &mut Option<&'a str>, option: &str, value: &'a str) -> Result<(), Failure> {
    if slot.replace(value).is_some() {
        return Err(Failure::Usage(format!("{option} given twice")));
    }
    Ok(())
}

/// Writes `text` to stdout. A reader that went away (`| head`) is no error.
pub(crate) fn print(text: &str) -> Result<(), Failure> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(Failure::Error(format!(
            "cannot write to the output: {error}"
        ))),
        _ => Ok(()),
    }
}
