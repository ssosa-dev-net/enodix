//! The credential agent: a running app answers the headless binary's
//! requests for a host's profile and for named secrets over a pipe only the
//! current user can open. Served here from a memory store, an unlocked
//! vault and a locked one, and read back through `Profiles` and `Secrets`
//! over an `AgentStore`, as the binary does.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use netterm_core::credentials::Profiles;
use netterm_core::credentials::{
    vault_file, ActiveStore, AgentEndpoint, AgentError, AgentServer, AgentStore, CredentialProfile,
    CredentialStore, HostCredentials, KdfParams, MemoryStore, NamedSecret, ProfileAuth,
    ProfileError, ScopedProfile, ScopedSecret, SecretValue, Secrets, StoreContents, Vault,
    VaultStatus,
};
use netterm_core::workspace::{HostId, Workspace};
use tempfile::TempDir;

const SCOPE: &str = "agent-test";
const PASSWORD: &str = "correct horse battery";

/// A workspace (id [`SCOPE`]) with `r1` using the profile `lab`, `r2` using
/// `missing` (not stored) and `r3` using none.
fn workspace(temp: &TempDir) -> Workspace {
    let root = temp.path().join("ws");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("_workspace.yaml"), format!("id: {SCOPE}\n")).unwrap();
    fs::write(
        root.join("r1.yaml"),
        "address: 192.0.2.1\ncredential_profile: LAB\n",
    )
    .unwrap();
    fs::write(
        root.join("r2.yaml"),
        "address: 192.0.2.2\ncredential_profile: missing\n",
    )
    .unwrap();
    fs::write(root.join("r3.yaml"), "address: 192.0.2.3\n").unwrap();
    Workspace::open(&root).unwrap()
}

fn lab_profile() -> CredentialProfile {
    CredentialProfile {
        name: "lab".into(),
        username: "admin".into(),
        auth: ProfileAuth::Password {
            password: "s3cret-pass".into(),
        },
        enable_secret: Some("en4ble".into()),
    }
}

fn contents() -> StoreContents {
    StoreContents {
        profiles: vec![ScopedProfile {
            scope: SCOPE.into(),
            profile: lab_profile(),
        }],
        secrets: vec![ScopedSecret {
            scope: SCOPE.into(),
            name: "netbox-token".into(),
            value: SecretValue::new("t0ken"),
        }],
    }
}

fn memory_store() -> Arc<MemoryStore> {
    let store = MemoryStore::new();
    store.put(SCOPE, &lab_profile()).unwrap();
    store
        .put_secret(SCOPE, "netbox-token", &SecretValue::new("t0ken"))
        .unwrap();
    Arc::new(store)
}

fn cheap() -> KdfParams {
    KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    }
}

/// A data folder of its own, so that the agent has its own pipe.
fn data_folder(temp: &TempDir) -> PathBuf {
    let folder = temp.path().join("data");
    fs::create_dir_all(&folder).unwrap();
    folder
}

fn serve(folder: &Path, store: Arc<dyn CredentialStore>) -> AgentServer {
    AgentServer::start(AgentEndpoint::for_data_folder(folder), store).expect("the agent starts")
}

fn client(folder: &Path) -> AgentStore {
    AgentStore::connect(&AgentEndpoint::for_data_folder(folder))
        .unwrap()
        .expect("an agent is listening")
}

fn id(text: &str) -> HostId {
    HostId::new(text)
}

/// Reads `r1`'s profile and the secret through the agent, as the binary
/// does.
fn check_served(agent: &AgentStore, workspace: &Workspace) {
    let profiles = Profiles::new(agent, workspace);
    let profile = profiles.host_profile(&id("r1.yaml")).unwrap();
    assert_eq!(profile, lab_profile());

    let secrets = Secrets::new(agent, workspace);
    assert_eq!(
        secrets
            .get("NETBOX-token")
            .unwrap()
            .map(|v| v.expose().to_owned()),
        Some("t0ken".to_owned())
    );
    assert_eq!(secrets.get("nothing").unwrap(), None);

    assert_eq!(
        profiles.host_profile(&id("r2.yaml")).unwrap_err(),
        ProfileError::NotSetUp("missing".into())
    );
    let none = profiles.host_profile(&id("r3.yaml")).unwrap_err();
    assert!(
        none.to_string().contains("has no credential profile"),
        "{none}"
    );
    assert!(matches!(
        profiles.host_profile(&id("nope.yaml")),
        Err(ProfileError::NotFound(_))
    ));
}

#[test]
fn an_agent_serves_profiles_and_secrets_from_a_memory_store() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let folder = data_folder(&temp);
    let _server = serve(&folder, memory_store());

    let agent = client(&folder);
    assert_eq!(agent.status().unwrap(), VaultStatus::Off);
    check_served(&agent, &workspace);
}

#[test]
fn an_agent_serves_from_an_unlocked_vault() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let folder = data_folder(&temp);
    let vault = Vault::create_with(&vault_file(&folder), PASSWORD, cheap(), contents()).unwrap();
    let _server = serve(&folder, Arc::new(vault));

    let agent = client(&folder);
    assert_eq!(agent.status().unwrap(), VaultStatus::Unlocked);
    check_served(&agent, &workspace);
}

#[test]
fn an_agent_over_a_locked_vault_gives_nothing_until_it_is_unlocked() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let folder = data_folder(&temp);
    drop(Vault::create_with(&vault_file(&folder), PASSWORD, cheap(), contents()).unwrap());
    let store = Arc::new(ActiveStore::new(
        Box::new(MemoryStore::new()),
        vault_file(&folder),
    ));
    let _server = serve(&folder, store.clone());

    let agent = client(&folder);
    assert_eq!(agent.status().unwrap(), VaultStatus::Locked);
    let refused = Profiles::new(&agent, &workspace)
        .host_profile(&id("r1.yaml"))
        .unwrap_err();
    assert!(refused.to_string().contains("locked"), "{refused}");
    let refused = Secrets::new(&agent, &workspace)
        .get("netbox-token")
        .unwrap_err();
    assert!(refused.to_string().contains("locked"), "{refused}");

    // The app unlocks it: the same connection now gets answers.
    store.unlock(PASSWORD).unwrap();
    assert_eq!(agent.status().unwrap(), VaultStatus::Unlocked);
    check_served(&agent, &workspace);
}

#[test]
fn the_agent_only_gives_out_and_never_stores_or_lists() {
    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let store = memory_store();
    let _server = serve(&folder, store.clone());
    let agent = client(&folder);

    assert!(agent.list(SCOPE).is_err());
    assert!(agent.secret_names(SCOPE).is_err());
    assert!(agent.contents().is_err());
    let mut changed = lab_profile();
    changed.username = "intruder".into();
    assert!(agent.put(SCOPE, &changed).is_err());
    assert!(agent.delete(SCOPE, "lab").is_err());
    assert!(agent
        .put_secret(SCOPE, "netbox-token", &SecretValue::new("x"))
        .is_err());
    assert!(agent.delete_secret(SCOPE, "netbox-token").is_err());
    assert_eq!(store.get(SCOPE, "lab").unwrap(), Some(lab_profile()));
    assert!(store.get_secret(SCOPE, "netbox-token").unwrap().is_some());
}

#[test]
fn without_a_running_agent_there_is_nobody_to_ask() {
    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let endpoint = AgentEndpoint::for_data_folder(&folder);
    assert!(AgentStore::connect(&endpoint).unwrap().is_none());

    let server = serve(&folder, memory_store());
    assert!(AgentStore::connect(&endpoint).unwrap().is_some());
    drop(server);
    assert!(AgentStore::connect(&endpoint).unwrap().is_none());
}

#[test]
fn a_second_agent_for_the_same_data_folder_is_refused() {
    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let _first = serve(&folder, memory_store());
    let second = AgentServer::start(AgentEndpoint::for_data_folder(&folder), memory_store());
    assert!(matches!(second, Err(AgentError::InUse(_))), "{second:?}");
}

#[test]
fn each_data_folder_has_its_own_endpoint() {
    let temp = TempDir::new().unwrap();
    let one = AgentEndpoint::for_data_folder(&temp.path().join("one"));
    let other = AgentEndpoint::for_data_folder(&temp.path().join("other"));
    assert_ne!(one, other);
    assert_eq!(
        one,
        AgentEndpoint::for_data_folder(&temp.path().join("one"))
    );
}

#[test]
fn several_clients_are_answered_at_once() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let folder = data_folder(&temp);
    let _server = serve(&folder, memory_store());
    let first = client(&folder);
    let second = client(&folder);
    check_served(&second, &workspace);
    check_served(&first, &workspace);
}

#[test]
fn what_the_binary_hands_out_is_json_and_its_debug_hides_secrets() {
    let credentials = HostCredentials::new(id("r1.yaml"), lab_profile());
    let json = serde_json::to_value(&credentials).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "format": 1,
            "host": "r1.yaml",
            "profile": "lab",
            "username": "admin",
            "auth": { "kind": "password", "password": "s3cret-pass" },
            "enable_secret": "en4ble",
        })
    );
    let key = HostCredentials::new(
        id("r1.yaml"),
        CredentialProfile {
            name: "keys".into(),
            username: "netops".into(),
            auth: ProfileAuth::PrivateKey {
                key_file: "C:/keys/id_ed25519".into(),
                passphrase: Some("phr4se".into()),
            },
            enable_secret: None,
        },
    );
    assert_eq!(
        serde_json::to_value(&key).unwrap()["auth"],
        serde_json::json!({
            "kind": "private_key",
            "key_file": "C:/keys/id_ed25519",
            "passphrase": "phr4se",
        })
    );
    let shown = format!("{credentials:?} {key:?}");
    for secret in ["s3cret-pass", "en4ble", "phr4se"] {
        assert!(!shown.contains(secret), "{shown}");
    }

    let secret = NamedSecret::new("netbox-token", SecretValue::new("t0ken"));
    assert_eq!(
        serde_json::to_value(&secret).unwrap(),
        serde_json::json!({ "format": 1, "name": "netbox-token", "value": "t0ken" })
    );
    assert!(!format!("{secret:?}").contains("t0ken"));
}

/// The pipe's access list: only the current user, nobody else (not even
/// administrators or SYSTEM), and the entry is not inherited.
#[cfg(windows)]
#[test]
fn only_the_current_user_can_open_the_pipe() {
    use std::os::windows::io::AsRawHandle;

    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let server = serve(&folder, memory_store());
    let pipe = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(server.endpoint().path())
        .unwrap();
    let access = windows::dacl_of(pipe.as_raw_handle());
    let user = windows::current_user_sid();
    assert_eq!(access, format!("D:P(A;;FA;;;{user})"));
}

/// Refused: a client that is not the current user cannot open the pipe,
/// not even to read (a pipe's default access list lets everyone read). The
/// other user is played by this user's own token with the user's SID made
/// deny-only, so that only the groups it is in (Everyone, Users, the logon
/// session...) can grant it anything; the current user still gets in.
#[cfg(windows)]
#[test]
fn a_client_that_is_not_the_current_user_cannot_open_the_pipe() {
    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let server = serve(&folder, memory_store());
    let path = server.endpoint().path().to_owned();
    // The control: a pipe with the default access list, which that user
    // may read.
    let default = windows::DefaultPipe::new();
    let default_path = default.name.clone();
    let (refused, control) = std::thread::spawn(move || {
        windows::as_another_user(|| {
            let open = |path: &Path, read: bool, write: bool| {
                fs::OpenOptions::new()
                    .read(read)
                    .write(write)
                    .open(path)
                    .map(drop)
                    .map_err(|error| error.kind())
            };
            (
                [(true, true), (true, false)].map(|(read, write)| open(&path, read, write)),
                open(Path::new(&default_path), true, false),
            )
        })
    })
    .join()
    .unwrap();
    assert_eq!(
        control,
        Ok(()),
        "the stand-in user cannot even read a default pipe"
    );
    assert_eq!(
        refused,
        [Err(std::io::ErrorKind::PermissionDenied); 2],
        "the pipe let another user in"
    );
    drop(default);
    assert!(AgentStore::connect(server.endpoint()).unwrap().is_some());
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
        GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, GetTokenInformation, ImpersonateLoggedOnUser, RevertToSelf,
        TokenUser, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SID_AND_ATTRIBUTES,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// A named pipe made with the system's default access list.
    pub struct DefaultPipe {
        pub name: String,
        handle: HANDLE,
    }

    impl DefaultPipe {
        pub fn new() -> Self {
            use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
            use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
            use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE};

            let suffix: u64 = rand::random();
            let name = format!(r"\\.\pipe\netterm-test-default-{suffix:016x}");
            let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
            // SAFETY: a valid name; no security attributes (the default).
            let handle = unsafe {
                CreateNamedPipeW(
                    wide.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE,
                    4,
                    4096,
                    4096,
                    0,
                    ptr::null(),
                )
            };
            assert_ne!(handle, INVALID_HANDLE_VALUE, "CreateNamedPipeW");
            Self { name, handle }
        }
    }

    impl Drop for DefaultPipe {
        fn drop(&mut self) {
            // SAFETY: the pipe's own handle.
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Runs `work` on this thread as this user's token with the user's own
    /// SID deny-only: as a user who is in the same groups but is not this
    /// one.
    pub fn as_another_user<T>(work: impl FnOnce() -> T) -> T {
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            assert_ne!(
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_ASSIGN_PRIMARY,
                    &mut token,
                ),
                0
            );
            let mut buffer = vec![0u64; 128];
            let mut length = 0;
            assert_ne!(
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                    &mut length,
                ),
                0
            );
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let disabled = [SID_AND_ATTRIBUTES {
                Sid: user.User.Sid,
                Attributes: 0,
            }];
            let mut restricted: HANDLE = ptr::null_mut();
            assert_ne!(
                CreateRestrictedToken(
                    token,
                    0,
                    1,
                    disabled.as_ptr(),
                    0,
                    ptr::null(),
                    0,
                    ptr::null(),
                    &mut restricted,
                ),
                0,
                "CreateRestrictedToken"
            );
            assert_ne!(ImpersonateLoggedOnUser(restricted), 0, "impersonation");
            let result = work();
            assert_ne!(RevertToSelf(), 0);
            CloseHandle(restricted);
            CloseHandle(token);
            result
        }
    }

    unsafe fn take_string(text: *mut u16) -> String {
        let length = (0..).take_while(|&i| *text.add(i) != 0).count();
        let string = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        string
    }

    pub fn dacl_of(handle: *mut c_void) -> String {
        unsafe {
            let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
            let status = GetSecurityInfo(
                handle as HANDLE,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            );
            assert_eq!(status, 0, "GetSecurityInfo");
            let mut text = ptr::null_mut();
            let converted = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            );
            assert_ne!(converted, 0);
            LocalFree(descriptor);
            take_string(text)
        }
    }

    pub fn current_user_sid() -> String {
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            assert_ne!(
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token),
                0
            );
            let mut buffer = vec![0u64; 128];
            let mut length = 0;
            assert_ne!(
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                    &mut length,
                ),
                0
            );
            CloseHandle(token);
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text = ptr::null_mut();
            assert_ne!(ConvertSidToStringSidW(user.User.Sid, &mut text), 0);
            take_string(text)
        }
    }
}

/// The socket is the owner's alone (mode 0600).
#[cfg(unix)]
#[test]
fn only_the_current_user_can_open_the_socket() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let server = serve(&folder, memory_store());
    let mode = fs::metadata(server.endpoint().path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// Refused: the client asks nothing of a socket others can open, of a path
/// that is not a socket (a file, a link to an agent's socket), as a socket
/// of another user would be refused (which a test cannot play).
#[cfg(unix)]
#[test]
fn a_socket_others_can_open_or_a_path_that_is_no_socket_is_not_asked() {
    use std::os::unix::fs::PermissionsExt;

    fn not_used(endpoint: &AgentEndpoint, expected: &str) {
        match AgentStore::connect(endpoint) {
            Err(AgentError::NotAgent(why)) => assert!(why.contains(expected), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    let temp = TempDir::new().unwrap();
    let folder = data_folder(&temp);
    let server = serve(&folder, memory_store());
    let endpoint = server.endpoint().clone();
    fs::set_permissions(endpoint.path(), fs::Permissions::from_mode(0o666)).unwrap();
    not_used(&endpoint, "can be opened by other users (mode 666)");
    fs::set_permissions(endpoint.path(), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(AgentStore::connect(&endpoint).unwrap().is_some());
    drop(server);

    fs::write(endpoint.path(), "not a socket").unwrap();
    not_used(&endpoint, "is not a socket");
    fs::remove_file(endpoint.path()).unwrap();

    let elsewhere = temp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let other = serve(&elsewhere, memory_store());
    std::os::unix::fs::symlink(other.endpoint().path(), endpoint.path()).unwrap();
    not_used(&endpoint, "is not a socket");
}
