//! Credential profiles: named logins kept in a credential store (here the
//! in-memory one), assigned to hosts through inheritance, and used to log
//! in without a prompt. Workspace files only ever name a profile.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use netterm_core::credentials::{
    AuthDraft, AuthSummary, CredentialProfile, CredentialStore, HostLogin, MemoryStore,
    ProfileAuth, ProfileDraft, ProfileError, ProfileSummary, Profiles,
};
use netterm_core::session::{
    SessionManager, SessionState, SessionTarget, SshAgent, SshAuth, SshTarget, TerminalSize,
};
use netterm_core::workspace::{HostId, Settings, Workspace};
use support::agent::TestAgent;
use support::workspaces::files;
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

/// A workspace whose `Lab` folder uses the profile `lab-admin`; `Lab/r1`
/// inherits it, `Lab/r2` names `Key-Login` itself, `adhoc` has none. Every
/// host points at `port` on the loopback address.
fn lab_workspace(port: u16) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("customer-a");
    fs::create_dir_all(root.join("Lab")).unwrap();
    fs::write(
        root.join("Lab/_folder.yaml"),
        "credential_profile: lab-admin\n",
    )
    .unwrap();
    let host = |extra: &str| format!("address: 127.0.0.1\nport: {port}\n{extra}");
    fs::write(root.join("Lab/r1.yaml"), host("")).unwrap();
    fs::write(
        root.join("Lab/r2.yaml"),
        host("credential_profile: Key-Login\n"),
    )
    .unwrap();
    fs::write(root.join("adhoc.yaml"), host("")).unwrap();
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    (temp, workspace)
}

fn password_draft(name: &str, username: &str, password: &str) -> ProfileDraft {
    ProfileDraft {
        name: name.into(),
        username: username.into(),
        auth: AuthDraft::Password {
            password: Some(password.into()),
        },
        enable_secret: None,
    }
}

fn login(profiles: &Profiles<'_>, id: &str) -> HostLogin {
    profiles.login(&HostId::new(id)).unwrap()
}

/// The target a host logs in with through its profile; panics if it would
/// prompt instead.
fn profile_target(profiles: &Profiles<'_>, id: &str) -> SshTarget {
    match login(profiles, id) {
        HostLogin::Profile { target, .. } => target,
        other => panic!("{id}: expected a profile login, got {other:?}"),
    }
}

async fn connects(target: SshTarget) {
    let manager = SessionManager::new();
    let (_id, mut events) = manager.open(
        SessionTarget::Ssh(target),
        TerminalSize { cols: 80, rows: 24 },
    );
    assert_eq!(next_state(&mut events).await, SessionState::Connecting);
    assert_eq!(next_state(&mut events).await, SessionState::Connected);
    output_ending_with(&mut events, PROMPT).await;
}

/// Every byte of every file under `root`, for checking no secret got there.
fn all_file_contents(root: &Path) -> String {
    files(root)
        .iter()
        .filter(|path| !path.ends_with('/'))
        .map(|path| fs::read_to_string(root.join(path)).unwrap())
        .collect()
}

#[tokio::test]
async fn a_host_whose_folder_names_a_stored_profile_logs_in_with_it() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = lab_workspace(server.port());
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    profiles
        .save(None, &password_draft("lab-admin", USERNAME, PASSWORD))
        .unwrap();

    match login(&profiles, "Lab/r1.yaml") {
        HostLogin::Profile { profile, target } => {
            assert_eq!(profile, "lab-admin");
            assert_eq!(target.username, USERNAME);
            assert_eq!(target.auth, SshAuth::Password(PASSWORD.into()));
            assert_eq!(target.port, server.port());
            connects(target).await;
        }
        other => panic!("expected a profile login, got {other:?}"),
    }
}

#[tokio::test]
async fn a_private_key_profile_logs_in_with_the_key_and_its_passphrase() {
    let server = TestSshServer::start().await;
    let (temp, workspace) = lab_workspace(server.port());
    let key = temp.path().join("id_ed25519");
    server.write_client_key(&key, Some("open sesame"));
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    profiles
        .save(
            None,
            &ProfileDraft {
                name: "Key-Login".into(),
                username: USERNAME.into(),
                auth: AuthDraft::PrivateKey {
                    key_file: key.clone(),
                    passphrase: Some("open sesame".into()),
                },
                enable_secret: None,
            },
        )
        .unwrap();

    let target = profile_target(&profiles, "Lab/r2.yaml");

    assert_eq!(
        target.auth,
        SshAuth::PrivateKey {
            key_file: key,
            passphrase: Some("open sesame".into())
        }
    );
    connects(target).await;
}

#[tokio::test]
async fn an_agent_profile_logs_in_with_the_agents_keys_and_stores_no_secret() {
    let server = TestSshServer::start().await;
    let (_temp, workspace) = lab_workspace(server.port());
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let summary = profiles
        .save(
            None,
            &ProfileDraft {
                name: "Key-Login".into(),
                username: USERNAME.into(),
                auth: AuthDraft::Agent,
                enable_secret: None,
            },
        )
        .unwrap();
    assert_eq!(summary.auth, AuthSummary::Agent);
    assert_eq!(
        store
            .get(&workspace.credential_scope(), "key-login")
            .unwrap()
            .unwrap()
            .auth,
        ProfileAuth::Agent
    );

    let target = profile_target(&profiles, "Lab/r2.yaml");

    // The computer's own agent, whichever runs.
    assert_eq!(target.auth, SshAuth::Agent(SshAgent::System));
    // That is the user's agent, which tests leave alone: one inside the
    // test stands in for it.
    let agent = TestAgent::start(&[server.client_key().clone()]).await;
    connects(SshTarget {
        auth: SshAuth::Agent(SshAgent::At(agent.path().to_path_buf())),
        ..target
    })
    .await;
}

#[test]
fn a_host_without_a_profile_prompts() {
    let (_temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    profiles
        .save(None, &password_draft("lab-admin", "a", "b"))
        .unwrap();

    assert_eq!(login(&profiles, "adhoc.yaml"), HostLogin::Prompt);
}

#[test]
fn a_named_profile_with_nothing_stored_says_which_profile_to_set_up() {
    let (_temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);

    assert_eq!(
        login(&profiles, "Lab/r1.yaml"),
        HostLogin::NotSetUp {
            profile: "lab-admin".into()
        }
    );
    assert_eq!(profiles.missing().unwrap(), ["Key-Login", "lab-admin"]);

    // Setting it up there and then lets the host log in.
    profiles
        .save(None, &password_draft("lab-admin", "noc", "pw"))
        .unwrap();
    assert_eq!(profile_target(&profiles, "Lab/r1.yaml").username, "noc");
    assert_eq!(profiles.missing().unwrap(), ["Key-Login"]);
}

#[test]
fn profile_names_match_ignoring_case() {
    let (_temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    profiles
        .save(None, &password_draft("key-login", "noc", "pw"))
        .unwrap();

    match login(&profiles, "Lab/r2.yaml") {
        HostLogin::Profile { profile, .. } => assert_eq!(profile, "key-login"),
        other => panic!("got {other:?}"),
    }
    let clash = profiles.save(None, &password_draft("KEY-LOGIN", "x", "y"));
    assert!(
        matches!(clash, Err(ProfileError::AlreadyExists(_))),
        "{clash:?}"
    );
}

#[test]
fn connecting_without_a_prompt_says_why_it_cannot() {
    let (_temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);

    assert_eq!(
        profiles.profile_target(&HostId::new("Lab/r1.yaml")),
        Err(ProfileError::NotSetUp("lab-admin".into()))
    );
    let none = profiles.profile_target(&HostId::new("adhoc.yaml"));
    assert!(matches!(none, Err(ProfileError::Invalid(_))), "{none:?}");

    profiles
        .save(None, &password_draft("lab-admin", "noc", "pw"))
        .unwrap();
    assert_eq!(
        profiles
            .profile_target(&HostId::new("Lab/r1.yaml"))
            .unwrap()
            .username,
        "noc"
    );
}

#[test]
fn an_unknown_host_is_an_error() {
    let (_temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);

    let result = profiles.login(&HostId::new("nope.yaml"));
    assert!(
        matches!(result, Err(ProfileError::NotFound(_))),
        "{result:?}"
    );
}

#[test]
fn profiles_are_created_listed_edited_and_deleted() {
    let (temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let key = temp.path().join("id_rsa");

    profiles
        .save(
            None,
            &ProfileDraft {
                enable_secret: Some("en4ble".into()),
                ..password_draft("lab-admin", "noc", "pw1")
            },
        )
        .unwrap();
    profiles
        .save(
            None,
            &ProfileDraft {
                name: "  backbone  ".into(),
                username: "  ops ".into(),
                auth: AuthDraft::PrivateKey {
                    key_file: key.clone(),
                    passphrase: None,
                },
                enable_secret: None,
            },
        )
        .unwrap();

    // Listed by name ignoring case, without secrets; names and usernames
    // are trimmed.
    assert_eq!(
        profiles.list().unwrap(),
        [
            ProfileSummary {
                name: "backbone".into(),
                username: "ops".into(),
                auth: AuthSummary::PrivateKey {
                    key_file: key.clone(),
                    has_passphrase: false
                },
                has_enable_secret: false,
            },
            ProfileSummary {
                name: "lab-admin".into(),
                username: "noc".into(),
                auth: AuthSummary::Password,
                has_enable_secret: true,
            },
        ]
    );

    // Editing with the secrets left out keeps them.
    profiles
        .save(
            Some("lab-admin"),
            &ProfileDraft {
                name: "lab-admin".into(),
                username: "noc2".into(),
                auth: AuthDraft::Password { password: None },
                enable_secret: None,
            },
        )
        .unwrap();
    assert_eq!(
        profiles.profile("lab-admin").unwrap(),
        Some(CredentialProfile {
            name: "lab-admin".into(),
            username: "noc2".into(),
            auth: ProfileAuth::Password {
                password: "pw1".into()
            },
            enable_secret: Some("en4ble".into()),
        })
    );

    // Renaming moves it; an empty enable secret removes it.
    profiles
        .save(
            Some("lab-admin"),
            &ProfileDraft {
                enable_secret: Some(String::new()),
                ..password_draft("lab-ops", "noc2", "pw2")
            },
        )
        .unwrap();
    assert_eq!(profiles.profile("lab-admin").unwrap(), None);
    assert_eq!(
        profiles.profile("lab-ops").unwrap().unwrap(),
        CredentialProfile {
            name: "lab-ops".into(),
            username: "noc2".into(),
            auth: ProfileAuth::Password {
                password: "pw2".into()
            },
            enable_secret: None,
        }
    );

    profiles.delete("LAB-OPS").unwrap();
    profiles.delete("backbone").unwrap();
    assert_eq!(profiles.list().unwrap(), []);
    let again = profiles.delete("backbone");
    assert!(matches!(again, Err(ProfileError::NotFound(_))), "{again:?}");
}

#[test]
fn switching_a_profile_between_password_and_key_needs_the_new_secret() {
    let (temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let key = temp.path().join("id_ed25519");
    profiles
        .save(
            None,
            &ProfileDraft {
                name: "p".into(),
                username: "u".into(),
                auth: AuthDraft::PrivateKey {
                    key_file: key,
                    passphrase: Some("phrase".into()),
                },
                enable_secret: None,
            },
        )
        .unwrap();

    // No password to keep from a key profile.
    let result = profiles.save(
        Some("p"),
        &ProfileDraft {
            auth: AuthDraft::Password { password: None },
            ..password_draft("p", "u", "")
        },
    );
    assert!(
        matches!(result, Err(ProfileError::Invalid(_))),
        "{result:?}"
    );

    // A passphrase left out is kept while the key file stays a key.
    let other_key = temp.path().join("other");
    profiles
        .save(
            Some("p"),
            &ProfileDraft {
                name: "p".into(),
                username: "u".into(),
                auth: AuthDraft::PrivateKey {
                    key_file: other_key.clone(),
                    passphrase: None,
                },
                enable_secret: None,
            },
        )
        .unwrap();
    assert_eq!(
        profiles.profile("p").unwrap().unwrap().auth,
        ProfileAuth::PrivateKey {
            key_file: other_key,
            passphrase: Some("phrase".into())
        }
    );
}

#[test]
fn bad_profiles_are_refused_and_nothing_is_stored() {
    let (_temp, workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    profiles
        .save(None, &password_draft("taken", "u", "p"))
        .unwrap();

    let refused = [
        (None, password_draft("  ", "u", "p")),
        (None, password_draft("line\nbreak", "u", "p")),
        (None, password_draft("ok", " ", "p")),
        (
            None,
            ProfileDraft {
                auth: AuthDraft::Password { password: None },
                ..password_draft("new", "u", "")
            },
        ),
        (
            None,
            ProfileDraft {
                auth: AuthDraft::PrivateKey {
                    key_file: PathBuf::new(),
                    passphrase: None,
                },
                ..password_draft("new", "u", "")
            },
        ),
        // Editing a profile that is not there, or renaming onto another.
        (Some("ghost"), password_draft("ghost", "u", "p")),
        (Some("taken"), password_draft("other", "u", "p")),
    ];
    profiles
        .save(None, &password_draft("other", "u", "p"))
        .unwrap();
    for (previous, draft) in refused {
        let result = profiles.save(previous, &draft);
        assert!(
            result.is_err(),
            "{previous:?} {:?} was accepted",
            draft.name
        );
    }
    assert_eq!(
        profiles
            .list()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect::<Vec<_>>(),
        ["other", "taken"]
    );
}

#[test]
fn each_workspace_has_its_own_profiles() {
    let (_a, customer_a) = lab_workspace(22);
    let (_b, customer_b) = lab_workspace(22);
    let store = MemoryStore::new();
    Profiles::new(&store, &customer_a)
        .save(None, &password_draft("lab-admin", "alice", "a-secret"))
        .unwrap();

    let b = Profiles::new(&store, &customer_b);
    assert_eq!(b.list().unwrap(), []);
    assert!(matches!(
        login(&b, "Lab/r1.yaml"),
        HostLogin::NotSetUp { .. }
    ));

    // The same folder opened again is the same workspace.
    let reopened = Workspace::open(customer_a.root()).unwrap();
    assert_eq!(
        profile_target(&Profiles::new(&store, &reopened), "Lab/r1.yaml").username,
        "alice"
    );
}

#[test]
fn secrets_never_reach_the_workspace_files() {
    let (_temp, mut workspace) = lab_workspace(22);
    let store = MemoryStore::new();
    let draft = ProfileDraft {
        enable_secret: Some("enable-s3cret".into()),
        ..password_draft("vault", "noc", "pa55w0rd-s3cret")
    };
    Profiles::new(&store, &workspace)
        .save(None, &draft)
        .unwrap();

    // Assigning the profile writes its name, and only its name.
    workspace
        .write_folder_settings(
            "",
            &Settings {
                credential_profile: Some("vault".into()),
                ..Settings::default()
            },
        )
        .unwrap();

    let contents = all_file_contents(workspace.root());
    assert!(contents.contains("credential_profile: vault"));
    for secret in ["pa55w0rd-s3cret", "enable-s3cret"] {
        assert!(!contents.contains(secret), "{secret} was written");
    }
    // Nor can a profile's Debug output show them.
    let stored = Profiles::new(&store, &workspace)
        .profile("vault")
        .unwrap()
        .unwrap();
    let debug = format!(
        "{stored:?} {:?}",
        profile_target(&Profiles::new(&store, &workspace), "adhoc.yaml")
    );
    for secret in ["pa55w0rd-s3cret", "enable-s3cret"] {
        assert!(!debug.contains(secret), "{debug}");
    }
}

#[test]
fn the_store_is_used_through_its_interface() {
    // Profiles work with any credential store, the keychain included.
    let store: Box<dyn CredentialStore> = Box::new(MemoryStore::new());
    let (_temp, workspace) = lab_workspace(22);
    let profiles = Profiles::new(store.as_ref(), &workspace);
    profiles.save(None, &password_draft("p", "u", "s")).unwrap();
    assert_eq!(store.list(&workspace.credential_scope()).unwrap().len(), 1);
}
