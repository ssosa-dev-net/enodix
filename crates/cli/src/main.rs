//! The headless binary: the core from the command line, with no window. It
//! needs no running app, but asks it for credentials while it runs (see
//! `credentials`). Its command line is a contract (the Python bridge
//! builds on it, `tests/` checks it); see `usage`.
//!
//! Exit status: 0 done, 1 refused or failed (the reason on stderr), 2 a
//! command line it does not understand.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use enodix_core::app_state::{resolved_path, same_folder};
use enodix_core::inventory::{export_within, InventoryExport, Limit};
use enodix_core::job::script::{ScriptTargets, TARGETS_VARIABLE};
use enodix_core::workspace::{folder_kind, FolderKind, Workspace};

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
  {NAME} clear-keychain --yes
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
clear-keychain
            removes every credential profile and named secret {PRODUCT}
            keeps in the system keychain, of every workspace, and says how
            many it removed. Without --yes it only says how many it would
            remove, and removes nothing. The vault is not touched.
            Uninstalling {PRODUCT} with its data deleted runs it.

A value that starts with - goes after --, which ends the options
(`{NAME} secret -- <workspace folder> -ro`); an option's value can always be
given as --folder=<value> (likewise --filter=, --host= and --port=).

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
        Some("clear-keychain") => credentials::clear_keychain(&args[1..]),
        Some(other) => Err(Failure::Usage(format!("unknown command {other:?}"))),
    }
}

/// `inventory <workspace> [limit]`.
fn inventory(args: &[String]) -> Result<(), Failure> {
    let (workspace, exported) = scoped_export(args)?;
    // Only read, never made a workspace (#131): say why it lists nothing.
    if folder_kind(workspace.root()).is_ok_and(|kind| kind == FolderKind::Other) {
        eprintln!(
            "{NAME}: warning: {} is not a workspace: no host files and no _workspace.yaml there \
             (nothing was written into it)",
            workspace.root().display()
        );
    }
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
    let mut options_ended = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if options_ended || !arg.starts_with('-') {
            if root.replace(arg.as_str()).is_some() {
                return Err(Failure::Usage(format!("unexpected argument {arg:?}")));
            }
            continue;
        }
        if arg == END_OF_OPTIONS {
            options_ended = true;
            continue;
        }
        let (option, attached) = option_and_value(arg);
        let mut value = || option_value(option, attached, &mut args);
        match option {
            "--folder" => set_once(&mut folder, "--folder", value()?)?,
            "--filter" => set_once(&mut filter, "--filter", value()?)?,
            "--host" => hosts.push(value()?.to_owned()),
            _ => return Err(unknown_option(arg)),
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

/// Ends the options: every argument after it is a value, even one that
/// starts with `-` (a secret called `-ro`, a workspace folder `-lab`).
pub(crate) const END_OF_OPTIONS: &str = "--";

/// `--name=value` as the option and its value; any other argument as
/// itself, with no value.
pub(crate) fn option_and_value(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((option, value)) if option.starts_with("--") => (option, Some(value)),
        _ => (arg, None),
    }
}

/// The value of `option`: `attached` (from `--option=<value>`, any value
/// but an empty one), else the next argument, which must not start with
/// `-` (it would be an option given by mistake).
pub(crate) fn option_value<'a>(
    option: &str,
    attached: Option<&'a str>,
    rest: &mut std::slice::Iter<'a, String>,
) -> Result<&'a str, Failure> {
    if let Some(value) = attached {
        return Some(value)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Failure::Usage(format!("{option} needs a value")));
    }
    match rest.next() {
        Some(value) if !value.starts_with('-') => Ok(value),
        Some(_) => Err(Failure::Usage(format!(
            "{option} needs a value (one that starts with - is given as {option}=<value>)"
        ))),
        None => Err(Failure::Usage(format!("{option} needs a value"))),
    }
}

pub(crate) fn unknown_option(arg: &str) -> Failure {
    Failure::Usage(format!(
        "unknown option {arg:?} (a value that starts with - goes after {END_OF_OPTIONS})"
    ))
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
