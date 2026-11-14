//! `credentials` and `secret`: a host's login and a named secret, for
//! scripts. They come from the running app's credential agent when it can
//! answer, else from the store read here: the keychain, or the vault after
//! asking for the master password on the terminal (never from the command
//! line or the environment). Without a terminal, a vault is refused: a run
//! nobody attends cannot open it.
//!
//! Secrets go to stdout only, and the buffers that held them are wiped.

use std::path::{Path, PathBuf};

use netterm_core::app_state::resolved_path;
use netterm_core::credentials::{
    vault_file, AgentEndpoint, AgentStore, CredentialStore, HostCredentials, KeychainStore,
    NamedSecret, Profiles, Secrets, Vault, VaultError, VaultStatus,
};
use netterm_core::workspace::{HostId, Workspace};
use zeroize::Zeroize;

use crate::{print, Failure};

/// The app's user-visible name, for messages.
pub const PRODUCT: &str = env!("NETTERM_PRODUCT_NAME");
/// The app's identifier: its data folder's name and its keychain entries'
/// prefix.
const IDENTIFIER: &str = env!("NETTERM_IDENTIFIER");
/// Overrides the per-user data folder, as for the app.
const DATA_DIR_VARIABLE: &str = "NETTERM_DATA_DIR";
/// Overrides the keychain entries' prefix (the tests' own, never the app's
/// real entries).
const KEYCHAIN_PREFIX_VARIABLE: &str = "NETTERM_KEYCHAIN_PREFIX";
/// How many times a wrong master password may be typed.
const PASSWORD_ATTEMPTS: usize = 3;

/// `credentials <workspace> <host id>`.
pub fn host(args: &[String]) -> Result<(), Failure> {
    let [root, id] = two_arguments(args, "a host id")?;
    let workspace = open_workspace(root)?;
    let id = HostId::new(id.replace('\\', "/"));
    if workspace.resolve_host(&id).is_none() {
        return Err(Failure::Error(format!("there is no host {id}")));
    }
    if crate::job_scope(&workspace)?.is_some_and(|targets| !targets.allows(&id)) {
        return Err(Failure::Error(format!(
            "the host {id} is not among the hosts of the job this script runs in"
        )));
    }
    let store = open_store()?;
    let profile = Profiles::new(store.as_ref(), &workspace)
        .host_profile(&id)
        .map_err(|error| Failure::Error(error.to_string()))?;
    print_secret(&HostCredentials::new(id, profile))
}

/// `secret <workspace> <name>`.
pub fn secret(args: &[String]) -> Result<(), Failure> {
    let [root, name] = two_arguments(args, "a secret name")?;
    let name = name.trim();
    let workspace = open_workspace(root)?;
    crate::job_scope(&workspace)?;
    let store = open_store()?;
    let value = Secrets::new(store.as_ref(), &workspace)
        .get(name)
        .map_err(|error| Failure::Error(error.to_string()))?
        .ok_or_else(|| Failure::Error(format!("there is no secret called {name}")))?;
    print_secret(&NamedSecret::new(name, value))
}

fn two_arguments<'a>(args: &'a [String], second: &str) -> Result<[&'a str; 2], Failure> {
    if let Some(option) = args.iter().find(|arg| arg.starts_with('-')) {
        return Err(Failure::Usage(format!("unknown option {option:?}")));
    }
    match args {
        [] => Err(Failure::Usage("no workspace folder given".into())),
        [_] => Err(Failure::Usage(format!("{second} is needed"))),
        [root, value] => Ok([root, value]),
        [_, _, extra, ..] => Err(Failure::Usage(format!("unexpected argument {extra:?}"))),
    }
}

fn open_workspace(root: &str) -> Result<Workspace, Failure> {
    Workspace::open(resolved_path(Path::new(root)))
        .map_err(|error| Failure::Error(error.to_string()))
}

fn print_secret(value: &impl serde::Serialize) -> Result<(), Failure> {
    let mut json = serde_json::to_string_pretty(value)
        .map_err(|error| Failure::Error(format!("cannot write the answer: {error}")))?;
    json.push('\n');
    let printed = print(&json);
    json.zeroize();
    printed
}

/// The app's per-user data folder: as the app finds it (Tauri's app data
/// folder, named after the identifier), or `NETTERM_DATA_DIR`.
pub(crate) fn data_folder() -> Result<PathBuf, Failure> {
    if let Some(folder) = std::env::var_os(DATA_DIR_VARIABLE) {
        return Ok(PathBuf::from(folder));
    }
    platform_data_folder()
        .map(|folder| folder.join(IDENTIFIER))
        .ok_or_else(|| Failure::Error("cannot tell where this user's data folder is".into()))
}

#[cfg(windows)]
fn platform_data_folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_data_folder() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_data_folder() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// Where the credentials are read from: the running app if it can answer,
/// else the store itself.
pub(crate) fn open_store() -> Result<Box<dyn CredentialStore>, Failure> {
    let data = data_folder()?;
    let mut app_locked = false;
    let agent = AgentStore::connect(&AgentEndpoint::for_data_folder(&data))
        .map_err(|error| Failure::Error(error.to_string()))?;
    if let Some(agent) = agent {
        match agent
            .status()
            .map_err(|error| Failure::Error(error.to_string()))?
        {
            VaultStatus::Locked => app_locked = true,
            VaultStatus::Off | VaultStatus::Unlocked => return Ok(Box::new(agent)),
        }
    }
    let vault = vault_file(&data);
    if Vault::exists(&vault) {
        return Ok(Box::new(unlock(&vault, app_locked)?));
    }
    if !KeychainStore::is_supported() {
        return Err(Failure::Error(format!(
            "no credentials are stored: on this system {PRODUCT} keeps them only in its vault, which is not turned on (Preferences, Security)"
        )));
    }
    let prefix = std::env::var(KEYCHAIN_PREFIX_VARIABLE).unwrap_or_else(|_| IDENTIFIER.into());
    Ok(Box::new(KeychainStore::new(prefix)))
}

/// Opens the vault with the master password, asked on the terminal (up to
/// [`PASSWORD_ATTEMPTS`] times). Refused without a terminal.
fn unlock(vault: &Path, app_locked: bool) -> Result<Vault, Failure> {
    if !has_terminal() {
        return Err(no_terminal(app_locked));
    }
    let mut prompt = format!("Master password of {PRODUCT}'s vault: ");
    for _ in 0..PASSWORD_ATTEMPTS {
        let mut password =
            rpassword::prompt_password(&prompt).map_err(|_| no_terminal(app_locked))?;
        if password.is_empty() {
            return Err(Failure::Error("no master password given".into()));
        }
        let opened = Vault::unlock(vault, &password);
        password.zeroize();
        match opened {
            Ok(vault) => return Ok(vault),
            Err(VaultError::WrongPassword) => {
                prompt = format!("Wrong password. Master password of {PRODUCT}'s vault: ");
            }
            Err(error) => return Err(Failure::Error(error.to_string())),
        }
    }
    Err(Failure::Error(VaultError::WrongPassword.to_string()))
}

/// Whether someone can be asked: a console with a window (a process
/// started without one, such as a scheduled task's, may still have a hidden
/// console nobody can type into; a pseudo console has a window of its own).
#[cfg(windows)]
fn has_terminal() -> bool {
    // SAFETY: no arguments; it returns null when there is no window.
    !unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null()
}

/// Whether someone can be asked: a controlling terminal.
#[cfg(not(windows))]
fn has_terminal() -> bool {
    std::fs::File::open("/dev/tty").is_ok()
}

fn no_terminal(app_locked: bool) -> Failure {
    let how = if app_locked {
        format!("{PRODUCT} is running but its vault is locked: unlock it there")
    } else {
        format!("start {PRODUCT} and unlock it")
    };
    Failure::Error(format!(
        "the credentials are in {PRODUCT}'s vault, and there is no terminal to ask for its master password on: {how}, or run this from a terminal"
    ))
}
