//! `credentials` and `secret`: a host's login and a named secret, for
//! scripts. They come from the running app's credential agent when it can
//! answer, else from the store read here: the keychain, or the vault after
//! asking for the master password on the terminal (never from the command
//! line or the environment). Without a terminal, a vault is refused: a run
//! nobody attends cannot open it.
//!
//! Secrets go to stdout only, and the buffers that held them are wiped.

use std::path::{Path, PathBuf};

use enodix_core::app_state::resolved_path;
use enodix_core::credentials::{
    vault_file, AgentEndpoint, AgentStore, CredentialStore, HostCredentials, KeychainStore,
    NamedSecret, Profiles, Secrets, Vault, VaultError, VaultStatus,
};
use enodix_core::job::script::ScriptTargets;
use enodix_core::workspace::{HostId, Workspace};
use zeroize::Zeroize;

use crate::{print, unknown_option, Failure, END_OF_OPTIONS};

/// The app's user-visible name, for messages.
pub const PRODUCT: &str = env!("ENODIX_PRODUCT_NAME");
/// The app's identifier: its data folder's name and its keychain entries'
/// prefix.
const IDENTIFIER: &str = env!("ENODIX_IDENTIFIER");
/// Overrides the per-user data folder, as for the app.
const DATA_DIR_VARIABLE: &str = "ENODIX_DATA_DIR";
/// Overrides the keychain entries' prefix (the tests' own, never the app's
/// real entries).
const KEYCHAIN_PREFIX_VARIABLE: &str = "ENODIX_KEYCHAIN_PREFIX";
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

/// `clear-keychain --yes`: removes every credential profile and named
/// secret this app keeps in the system keychain, of every workspace, and
/// says how many. Without `--yes` it says what it would remove, removes
/// nothing and fails (status 1). The uninstaller runs it with `--yes` when
/// the app's data is deleted. Neither the running app nor the vault is
/// asked: the vault is a file in the data folder, which goes with it.
pub fn clear_keychain(args: &[String]) -> Result<(), Failure> {
    let confirmed = match args {
        [] => false,
        [yes] if yes == "--yes" => true,
        [yes, extra, ..] if yes == "--yes" => {
            return Err(Failure::Usage(format!("unexpected argument {extra:?}")))
        }
        [option, ..] if option.starts_with('-') => return Err(unknown_option(option)),
        [extra, ..] => return Err(Failure::Usage(format!("unexpected argument {extra:?}"))),
    };
    if ScriptTargets::from_environment().is_some() {
        return Err(Failure::Error(
            "this runs for a script job, and removes no saved logins".into(),
        ));
    }
    if !KeychainStore::is_supported() {
        return print(&format!(
            "{PRODUCT} keeps nothing in a keychain on this system: nothing removed\n"
        ));
    }
    let store = keychain();
    if !confirmed {
        let found = store
            .count_all()
            .map_err(|error| Failure::Error(error.to_string()))?;
        return Err(Failure::Error(format!(
            "this would remove {} and {} from the keychain, of every workspace; nothing was removed: run it with --yes to remove them",
            counted(found.profiles, "credential profile"),
            counted(found.secrets, "named secret"),
        )));
    }
    let removed = store
        .remove_all()
        .map_err(|error| Failure::Error(error.to_string()))?;
    print(&format!(
        "removed {} and {} from the keychain\n",
        counted(removed.profiles, "credential profile"),
        counted(removed.secrets, "named secret"),
    ))
}

fn counted(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        count => format!("{count} {noun}s"),
    }
}

/// The app's entries in the keychain, or the tests' own
/// ([`KEYCHAIN_PREFIX_VARIABLE`]).
fn keychain() -> KeychainStore {
    let prefix = std::env::var(KEYCHAIN_PREFIX_VARIABLE).unwrap_or_else(|_| IDENTIFIER.into());
    KeychainStore::new(prefix)
}

/// `<workspace> <value>`; after `--`, an argument that starts with `-` is
/// a value too.
fn two_arguments<'a>(args: &'a [String], second: &str) -> Result<[&'a str; 2], Failure> {
    let mut values: Vec<&'a str> = Vec::with_capacity(2);
    let mut options_ended = false;
    for arg in args {
        if !options_ended && arg == END_OF_OPTIONS {
            options_ended = true;
        } else if !options_ended && arg.starts_with('-') {
            return Err(unknown_option(arg));
        } else {
            values.push(arg);
        }
    }
    match values.as_slice() {
        [] => Err(Failure::Usage("no workspace folder given".into())),
        [_] => Err(Failure::Usage(format!("{second} is needed"))),
        [root, value] => Ok([*root, *value]),
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
/// folder, named after the identifier), or `ENODIX_DATA_DIR`.
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
    Ok(Box::new(keychain()))
}

/// Opens the vault with the master password, asked on the terminal (up to
/// [`PASSWORD_ATTEMPTS`] times). Refused without a terminal. A file that
/// cannot be the vault is said to be damaged first: no password would help.
fn unlock(vault: &Path, app_locked: bool) -> Result<Vault, Failure> {
    Vault::check(vault).map_err(|error| Failure::Error(error.to_string()))?;
    if !has_terminal() {
        return Err(no_terminal(app_locked, NO_TERMINAL));
    }
    if !seen_by_someone() {
        return Err(no_terminal(app_locked, NOBODY_TO_TYPE));
    }
    let mut prompt = format!("Master password of {PRODUCT}'s vault: ");
    for _ in 0..PASSWORD_ATTEMPTS {
        let mut password = rpassword::prompt_password(&prompt)
            .map_err(|_| no_terminal(app_locked, NO_TERMINAL))?;
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

/// Whether someone can type into the console: it is a pseudo console (a
/// terminal program, Windows Terminal or an SSH server, shows it and types
/// into it, wherever this runs), or its window can be on a screen at all:
/// this process's window station is the interactive one. A scheduled task
/// run "whether the user is logged on or not" (or a service) is on another,
/// which no screen shows: its console has a window, and nobody will ever
/// type into it.
#[cfg(windows)]
fn seen_by_someone() -> bool {
    pseudo_console() || on_a_shown_window_station()
}

#[cfg(windows)]
fn pseudo_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;

    // SAFETY: no arguments.
    let window = unsafe { GetConsoleWindow() };
    let mut class = [0u16; 64];
    // SAFETY: the buffer and its length are `class`'s.
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    // The class of the stand-in window a pseudo console gives its programs.
    usize::try_from(length).is_ok_and(|length| {
        String::from_utf16_lossy(&class[..length.min(class.len())]) == "PseudoConsoleWindow"
    })
}

#[cfg(windows)]
fn on_a_shown_window_station() -> bool {
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
    };
    /// WinUser.h: the window station has visible windows.
    const WSF_VISIBLE: u32 = 1;

    // SAFETY: no arguments; the handle is not to be closed.
    let station = unsafe { GetProcessWindowStation() };
    if station.is_null() {
        return false;
    }
    let mut flags = USEROBJECTFLAGS::default();
    let mut needed = 0;
    // SAFETY: the buffer and its size are `flags`.
    let read = unsafe {
        GetUserObjectInformationW(
            station,
            UOI_FLAGS,
            (&mut flags as *mut USEROBJECTFLAGS).cast(),
            std::mem::size_of::<USEROBJECTFLAGS>() as u32,
            &mut needed,
        )
    };
    // Cannot tell: ask, as before this was checked.
    read == 0 || flags.dwFlags & WSF_VISIBLE != 0
}

/// A terminal is someone's.
#[cfg(not(windows))]
fn seen_by_someone() -> bool {
    true
}

/// Why nobody is asked: no terminal at all.
const NO_TERMINAL: &str = "there is no terminal to ask for its master password on";
/// Why nobody is asked: a console nobody sees.
const NOBODY_TO_TYPE: &str = "nobody can type the vault's master password here (this runs where no screen shows it, as a scheduled task does while nobody is logged on)";

fn no_terminal(app_locked: bool, why: &str) -> Failure {
    let how = if app_locked {
        format!("{PRODUCT} is running but its vault is locked: unlock it there")
    } else {
        format!("start {PRODUCT} and unlock it")
    };
    Failure::Error(format!(
        "the credentials are in {PRODUCT}'s vault, and {why}: {how}, or run this from a terminal"
    ))
}
