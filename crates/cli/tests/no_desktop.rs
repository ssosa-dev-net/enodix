//! The headless binary where nobody can type: under the Windows Task
//! Scheduler with "run whether the user is logged on or not", a program runs
//! in a window station of its own that no screen shows. It still gets a
//! console, with a window nobody sees or types into, so "is there a console
//! window" says yes. Played here by starting the binary, with a new console,
//! on a desktop of a window station this test makes (any user may make one;
//! only the interactive one, WinSta0, is ever shown). Asked for the vault's
//! master password there, it must refuse at once and say what to do, not
//! wait for typing that never comes.
//!
//! One test in a file of its own: making the desktop switches this process's
//! window station for a moment, which no other test may see.
#![cfg(windows)]

use std::fs::{self, File};
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;

use netterm_core::credentials::{vault_file, KdfParams, StoreContents, Vault};
use tempfile::TempDir;
use windows_sys::Win32::Foundation::{
    SetHandleInformation, GENERIC_ALL, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW,
    GetProcessWindowStation, GetUserObjectInformationW, SetProcessWindowStation, UOI_NAME,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, TerminateProcess, WaitForSingleObject, CREATE_NEW_CONSOLE,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
};

const BINARY: &str = env!("CARGO_BIN_EXE_netterm-cli");
/// Every right on a window station (WinUser.h), READ_CONTROL included.
const WINSTA_ALL_ACCESS: u32 = 0x037F | 0x0002_0000;
/// Long enough for a busy machine; a binary that waits for typing waits
/// for ever.
const GIVE_UP_AFTER_MS: u32 = 60_000;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// A window station of this test's own (not shown anywhere) with a desktop.
struct Station {
    desktop_name: String,
    station: isize,
    desktop: isize,
}

impl Station {
    fn new() -> Self {
        unsafe {
            // Unnamed: the system names it (a named one may be refused to a
            // user who is not an administrator), as browsers' sandboxes do.
            let station = CreateWindowStationW(ptr::null(), 0, WINSTA_ALL_ACCESS, ptr::null());
            assert!(!station.is_null(), "CreateWindowStationW: {}", last_error());
            let name = object_name(station);
            let original = GetProcessWindowStation();
            assert_ne!(SetProcessWindowStation(station), 0, "{}", last_error());
            let desktop = CreateDesktopW(
                wide("Default").as_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                GENERIC_ALL,
                ptr::null(),
            );
            let made = last_error();
            assert_ne!(SetProcessWindowStation(original), 0, "{}", last_error());
            assert!(!desktop.is_null(), "CreateDesktopW: {made}");
            Self {
                desktop_name: format!("{name}\\Default"),
                station: station as isize,
                desktop: desktop as isize,
            }
        }
    }

    /// Runs the binary there with a new console, `env` added to this
    /// process's environment; its exit code and what it wrote to stderr.
    /// Panics if it is still running after [`GIVE_UP_AFTER_MS`].
    fn run(&self, args: &[&str], env: &[(&str, &Path)], folder: &Path) -> (u32, String) {
        let err_path = folder.join("stderr.txt");
        let input = inheritable(File::open("NUL").unwrap());
        let output = inheritable(File::create(folder.join("stdout.txt")).unwrap());
        let errors = inheritable(File::create(&err_path).unwrap());

        let mut line = format!("\"{BINARY}\"");
        for arg in args {
            line.push_str(&format!(" \"{arg}\""));
        }
        let mut line = wide(&line);
        let mut variables: Vec<(String, String)> = std::env::vars().collect();
        for (name, value) in env {
            variables.retain(|(other, _)| !other.eq_ignore_ascii_case(name));
            variables.push(((*name).into(), value.display().to_string()));
        }
        variables.sort_by_key(|(name, _)| name.to_uppercase());
        let mut block: Vec<u16> = Vec::new();
        for (name, value) in &variables {
            block.extend(format!("{name}={value}").encode_utf16());
            block.push(0);
        }
        block.push(0);
        let mut desktop = wide(&self.desktop_name);

        unsafe {
            let mut startup: STARTUPINFOW = std::mem::zeroed();
            startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            startup.lpDesktop = desktop.as_mut_ptr();
            startup.dwFlags = STARTF_USESTDHANDLES;
            startup.hStdInput = input.as_raw_handle() as HANDLE;
            startup.hStdOutput = output.as_raw_handle() as HANDLE;
            startup.hStdError = errors.as_raw_handle() as HANDLE;
            let mut process: PROCESS_INFORMATION = std::mem::zeroed();
            let started = CreateProcessW(
                ptr::null(),
                line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT,
                block.as_ptr().cast(),
                ptr::null(),
                &startup,
                &mut process,
            );
            assert_ne!(started, 0, "CreateProcessW: {}", last_error());
            let handle = OwnedHandle::from_raw_handle(process.hProcess);
            drop(OwnedHandle::from_raw_handle(process.hThread));
            drop((input, output, errors));
            let waited = WaitForSingleObject(handle.as_raw_handle() as HANDLE, GIVE_UP_AFTER_MS);
            if waited != WAIT_OBJECT_0 {
                TerminateProcess(handle.as_raw_handle() as HANDLE, 1);
                WaitForSingleObject(handle.as_raw_handle() as HANDLE, GIVE_UP_AFTER_MS);
                panic!(
                    "still running after {} s (waiting for a password nobody can type): {}",
                    GIVE_UP_AFTER_MS / 1000,
                    fs::read_to_string(&err_path).unwrap_or_default()
                );
            }
            let mut code = 0;
            assert_ne!(
                GetExitCodeProcess(handle.as_raw_handle() as HANDLE, &mut code),
                0
            );
            (code, fs::read_to_string(&err_path).unwrap())
        }
    }
}

impl Drop for Station {
    fn drop(&mut self) {
        unsafe {
            CloseDesktop(self.desktop as _);
            CloseWindowStation(self.station as _);
        }
    }
}

/// The name of a window station.
fn object_name(station: windows_sys::Win32::System::StationsAndDesktops::HWINSTA) -> String {
    let mut buffer = [0u16; 256];
    let mut needed = 0;
    let done = unsafe {
        GetUserObjectInformationW(
            station as HANDLE,
            UOI_NAME,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 2) as u32,
            &mut needed,
        )
    };
    assert_ne!(done, 0, "GetUserObjectInformationW: {}", last_error());
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

/// `file`, its handle inherited by the processes this one starts.
fn inheritable(file: File) -> OwnedHandle {
    let handle = file.into_raw_handle();
    unsafe {
        assert_ne!(
            SetHandleInformation(handle as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT),
            0
        );
        OwnedHandle::from_raw_handle(handle)
    }
}

fn last_error() -> std::io::Error {
    std::io::Error::last_os_error()
}

#[test]
fn with_a_vault_and_nobody_to_type_it_is_refused_instead_of_waiting() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(
        workspace.join("_workspace.yaml"),
        "id: cli-no-desktop-test\n",
    )
    .unwrap();
    fs::write(
        workspace.join("r1.yaml"),
        "address: 192.0.2.1\ncredential_profile: lab\n",
    )
    .unwrap();
    let data = temp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let kdf = KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };
    drop(
        Vault::create_with(
            &vault_file(&data),
            "correct horse battery",
            kdf,
            StoreContents::default(),
        )
        .unwrap(),
    );
    let prefix = temp.path().join("netterm-test-no-desktop-never-used");

    let station = Station::new();
    for args in [
        ["credentials", workspace.to_str().unwrap(), "r1.yaml"],
        ["secret", workspace.to_str().unwrap(), "netbox-token"],
    ] {
        let (code, said) = station.run(
            &args,
            &[
                ("NETTERM_DATA_DIR", &data),
                (
                    "NETTERM_KEYCHAIN_PREFIX",
                    Path::new(prefix.file_name().unwrap()),
                ),
            ],
            temp.path(),
        );
        assert_eq!(code, 1, "{args:?}: {said}");
        assert!(
            said.contains("nobody can type the vault's master password here"),
            "{said}"
        );
        assert!(said.contains("start netterm and unlock it"), "{said}");
    }
}
