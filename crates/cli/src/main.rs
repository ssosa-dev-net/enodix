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

use netterm_core::app_state::resolved_path;
use netterm_core::inventory::{export, Limit};
use netterm_core::workspace::Workspace;

mod credentials;

use credentials::PRODUCT;

const NAME: &str = env!("CARGO_BIN_NAME");

fn usage() -> String {
    format!(
        "\
Usage:
  {NAME} inventory <workspace folder> [--folder <path> | --filter <name> | --host <id>...]
  {NAME} credentials <workspace folder> <host id>
  {NAME} secret <workspace folder> <name>
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

Credentials come from {PRODUCT} while it runs and its vault, if it has
one, is unlocked. Otherwise they are read here: from the keychain, or from
the vault after asking for its master password on the terminal. With a
vault and neither {PRODUCT} unlocked nor a terminal, they are refused.
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
        Some(other) => Err(Failure::Usage(format!("unknown command {other:?}"))),
    }
}

/// `inventory <workspace> [limit]`.
fn inventory(args: &[String]) -> Result<(), Failure> {
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

    let workspace = Workspace::open(resolved_path(Path::new(root)))
        .map_err(|error| Failure::Error(error.to_string()))?;
    let exported = export(&workspace, &limit).map_err(|error| Failure::Error(error.to_string()))?;
    for problem in &exported.problems {
        eprintln!("{NAME}: warning: {problem}");
    }
    let mut json = serde_json::to_string_pretty(&exported)
        .map_err(|error| Failure::Error(format!("cannot write the inventory: {error}")))?;
    json.push('\n');
    print(&json)
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
