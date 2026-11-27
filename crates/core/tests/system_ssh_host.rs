//! A workspace host for the system's ssh client (#27): what it is told, and
//! how the session is marked. No session is opened here (the client's
//! arguments are in `system_ssh.rs`).

use std::fs;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::session::{
    system_ssh_target, SessionRecord, SystemSshHost, Transport, DEFAULT_SSH_PORT,
    SYSTEM_SSH_WARNING,
};
use enodix_core::workspace::{HostId, Workspace};
use tempfile::TempDir;

fn workspace(files: &[(&str, &str)]) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("ws");
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

#[test]
fn a_host_with_no_profile_leaves_the_user_to_the_client() {
    let (_temp, workspace) = workspace(&[("old.yaml", "address: 10.0.0.9\nport: 2222\n")]);
    let store = MemoryStore::new();
    let host = Profiles::new(&store, &workspace)
        .system_ssh_host(&HostId::new("old.yaml"))
        .unwrap();
    assert_eq!(
        host,
        SystemSshHost {
            host: "10.0.0.9".into(),
            port: 2222,
            username: None,
        }
    );
}

#[test]
fn the_profiles_username_is_passed_but_never_its_secret() {
    let (_temp, workspace) = workspace(&[
        ("_folder.yaml", "credential_profile: noc\n"),
        ("old.yaml", "address: 10.0.0.9\n"),
    ]);
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "noc".into(),
                username: "admin".into(),
                auth: ProfileAuth::Password {
                    password: "s3cret-pw".into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    let host = Profiles::new(&store, &workspace)
        .system_ssh_host(&HostId::new("old.yaml"))
        .unwrap();
    assert_eq!(host.username.as_deref(), Some("admin"));
    assert_eq!(host.port, DEFAULT_SSH_PORT);
    let target = system_ssh_target(None, &host, None, Vec::new(), &Default::default());
    assert!(
        !format!("{target:?}").contains("s3cret-pw"),
        "the password reached the command line"
    );
}

#[test]
fn a_host_behind_a_jump_host_is_refused_rather_than_reached_around_it() {
    let (_temp, workspace) = workspace(&[
        ("bastion.yaml", "address: 10.0.0.1\n"),
        ("Site/_folder.yaml", "jump_host: bastion.yaml\n"),
        ("Site/switch.yaml", "address: 10.1.0.9\n"),
    ]);
    let store = MemoryStore::new();
    let error = Profiles::new(&store, &workspace)
        .system_ssh_host(&HostId::new("Site/switch.yaml"))
        .unwrap_err();
    assert!(
        error.to_string().contains("jump host bastion.yaml"),
        "{error}"
    );
}

#[test]
fn the_record_of_such_a_session_carries_the_warning_and_the_host() {
    let (_temp, workspace) = workspace(&[("old.yaml", "address: 10.0.0.9\n")]);
    let id = HostId::new("old.yaml");
    let record = SessionRecord::for_host(&workspace, &id, Transport::Local, "old", "old (ssh)")
        .unwrap()
        .insecure(SYSTEM_SSH_WARNING);
    assert_eq!(record.insecure.as_deref(), Some(SYSTEM_SSH_WARNING));
    assert_eq!(record.host, Some(id));
    // An ordinary session has no such warning.
    assert_eq!(SessionRecord::new("a", "a").insecure, None);
}
