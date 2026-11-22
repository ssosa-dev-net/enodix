//! Edge tests (#117): every feature that needs a login meets a locked vault,
//! and a keychain that refuses every read (as one denying access does). Each
//! must say why, in its own words carrying the store's reason, at once: none
//! may hang, fail with an unrelated message, or take the refusal for a
//! profile that is not set up. A keychain with no entry is a profile not set
//! up (covered by `credentials.rs`, `jobs.rs` and the cli's tests).
//!
//! The features, through the core's API: a session's login and target
//! (`Profiles::login`, `profile_target`, `prompted_target` behind a jump
//! host), the Files tab's target (the same `profile_target`), the headless
//! binary's host credentials (`host_profile`) and secrets, the Credentials
//! view's lists, jobs of every step (a snippet, a config backup, a script, host
//! enrichment, a file job), back up on login, the tunnels scripts use, and
//! the credential agent. Windows and Linux alike.

mod support;

use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use enodix_core::configs::ConfigStore;
use enodix_core::credentials::{
    vault_file, ActiveStore, AgentEndpoint, AgentServer, AgentStore, CredentialProfile,
    CredentialStore, KdfParams, MemoryStore, ProfileAuth, Profiles, ScopedProfile, Secrets,
    StoreContents, Vault, VaultStatus,
};
use enodix_core::job::{self, JobStep, JobTarget, LoginBackup, SessionLogin, SkipReason};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::tunnel::{self, Planned};
use enodix_core::workspace::{HostId, Workspace};
use support::credentials::{FailingStore, Failures, READ_REFUSED};
use tempfile::TempDir;

const SCOPE: &str = "edge-logins";
const MASTER: &str = "correct horse battery staple";

/// A workspace (id [`SCOPE`]) with `r1` (eos, profile `lab`, backed up on
/// login), the jump host `j1` (profile `lab`), `r2` behind it (eos, profile
/// `lab`) and `typed` behind it with no profile (logged in by hand); a
/// snippet, a script and a local file to send.
fn workspace(temp: &TempDir) -> Workspace {
    let root = temp.path().join("ws");
    fs::create_dir_all(root.join("scripts")).unwrap();
    let files = [
        ("_workspace.yaml", format!("id: {SCOPE}\n")),
        ("_snippets.yaml", "Show version: show version\n".into()),
        ("scripts/hello.py", "print('hello')\n".into()),
        (
            "r1.yaml",
            "address: 192.0.2.1\nplatform: eos\ncredential_profile: lab\nbackup_on_login: true\n"
                .into(),
        ),
        (
            "j1.yaml",
            "address: 192.0.2.9\ncredential_profile: lab\n".into(),
        ),
        (
            "r2.yaml",
            "address: 192.0.2.2\nplatform: eos\ncredential_profile: lab\njump_host: j1.yaml\n"
                .into(),
        ),
        (
            "typed.yaml",
            "address: 192.0.2.3\njump_host: j1.yaml\n".into(),
        ),
    ];
    for (file, text) in files {
        fs::write(root.join(file), text).unwrap();
    }
    fs::write(temp.path().join("image.bin"), b"firmware").unwrap();
    let workspace = Workspace::open(&root).unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    workspace
}

fn lab() -> CredentialProfile {
    CredentialProfile {
        name: "lab".into(),
        username: "admin".into(),
        auth: ProfileAuth::Password {
            password: "s3cret".into(),
        },
        enable_secret: None,
    }
}

/// The app's store over a vault holding `lab`, locked.
fn locked_vault(data: &Path) -> ActiveStore {
    let contents = StoreContents {
        profiles: vec![ScopedProfile {
            scope: SCOPE.into(),
            profile: lab(),
        }],
        secrets: Vec::new(),
    };
    let cheap = KdfParams {
        memory_kib: 8 * 1024,
        iterations: 1,
        parallelism: 1,
    };
    drop(Vault::create_with(&vault_file(data), MASTER, cheap, contents).unwrap());
    let store = ActiveStore::new(Box::new(MemoryStore::new()), vault_file(data));
    assert_eq!(store.status(), VaultStatus::Locked);
    store
}

/// A keychain holding `lab` that refuses every read.
fn refusing_keychain() -> FailingStore {
    let keychain = FailingStore::new(Failures {
        reads: true,
        ..Failures::default()
    });
    keychain.put(SCOPE, &lab()).unwrap();
    keychain
}

fn id(text: &str) -> HostId {
    HostId::new(text)
}

/// Every feature, over `store`, says `reason`.
fn every_feature_says(
    store: &dyn CredentialStore,
    workspace: &Workspace,
    reason: &str,
    temp: &TempDir,
) {
    let says = |what: &str, message: String| {
        assert!(
            message.contains(reason),
            "{what} does not say {reason:?}: {message}"
        );
    };
    let profiles = Profiles::new(store, workspace);

    // A session: how it logs in, its target, a typed login behind a jump
    // host (whose own profile cannot be read). The Files tab connects
    // through the same `profile_target`.
    says(
        "login",
        profiles.login(&id("r1.yaml")).unwrap_err().to_string(),
    );
    says(
        "profile_target",
        profiles
            .profile_target(&id("r1.yaml"))
            .unwrap_err()
            .to_string(),
    );
    says(
        "profile_target behind a jump host",
        profiles
            .profile_target(&id("r2.yaml"))
            .unwrap_err()
            .to_string(),
    );
    says(
        "prompted_target behind a jump host",
        profiles
            .prompted_target(&id("typed.yaml"), "me".into(), "pw".into())
            .unwrap_err()
            .to_string(),
    );
    // The headless binary's answers.
    says(
        "host_profile",
        profiles
            .host_profile(&id("r1.yaml"))
            .unwrap_err()
            .to_string(),
    );
    says(
        "a named secret",
        Secrets::new(store, workspace)
            .get("netbox")
            .unwrap_err()
            .to_string(),
    );
    // The Credentials view.
    says("the profile list", profiles.list().unwrap_err().to_string());
    says(
        "the missing list",
        profiles.missing().unwrap_err().to_string(),
    );

    // Jobs of every step: r1 and r2 are skipped saying why.
    let steps = [
        JobStep::Snippet {
            name: "Show version".into(),
            answers: Default::default(),
        },
        JobStep::ConfigBackup,
        JobStep::Script {
            path: "scripts/hello.py".into(),
        },
        JobStep::Enrich,
        JobStep::SendFile {
            file: temp.path().join("image.bin").to_string_lossy().into_owned(),
            destination: "/tmp".into(),
            overwrite: false,
        },
    ];
    for step in steps {
        let plan = job::plan(
            workspace,
            &profiles,
            &JobTarget::Folder {
                path: String::new(),
            },
            &step,
        )
        .unwrap_or_else(|error| panic!("{step:?}: {error}"));
        for host in ["r1.yaml", "r2.yaml"] {
            let skipped = plan
                .skipped
                .iter()
                .find(|skipped| skipped.id.as_str() == host)
                .unwrap_or_else(|| panic!("{step:?}: {host} not skipped: {plan:?}"));
            assert!(
                matches!(skipped.reason, SkipReason::Credentials { .. }),
                "{step:?}: {host}: {:?}",
                skipped.reason
            );
            says(&format!("{step:?} on {host}"), skipped.message.clone());
        }
        assert!(
            plan.hosts.iter().all(|host| host.id.as_str() != "r1.yaml"),
            "{step:?}"
        );
    }

    // Back up on login after a typed login: planned without the profile's
    // enable secret (there is none to read), never a panic or a wait.
    let configs = ConfigStore::new(temp.path().join("configs"));
    let backup = LoginBackup::plan(
        workspace,
        &profiles,
        &id("r1.yaml"),
        SessionLogin::Typed,
        configs.clone(),
    );
    assert!(backup.is_some(), "a typed login is still backed up");
    let backup = LoginBackup::plan(
        workspace,
        &profiles,
        &id("r1.yaml"),
        SessionLogin::Profile,
        configs,
    );
    assert!(backup.is_some());

    // The tunnels a script uses to reach r2.
    let plan = tunnel::plan(
        workspace,
        &profiles,
        &KnownHosts::in_memory(),
        &[id("r2.yaml")],
        &[],
    );
    match plan.hosts.as_slice() {
        [Planned::Refused {
            reason: message, ..
        }] => says("the tunnel", message.clone()),
        other => panic!("the tunnel to r2: {other:?}"),
    }
}

#[test]
fn a_locked_vault_is_named_by_every_feature_that_needs_a_login() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let data = temp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let store = locked_vault(&data);
    every_feature_says(&store, &workspace, "locked", &temp);

    // Unlocked, the same features have the profile.
    store.unlock(MASTER).unwrap();
    let profiles = Profiles::new(&store, &workspace);
    assert_eq!(
        profiles.profile_target(&id("r1.yaml")).unwrap().username,
        "admin"
    );
    assert_eq!(
        profiles
            .profile_target(&id("r2.yaml"))
            .unwrap()
            .jump_hosts
            .len(),
        1
    );
}

#[test]
fn a_keychain_refusing_every_read_is_named_by_every_feature_that_needs_a_login() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let keychain = refusing_keychain();
    every_feature_says(&keychain, &workspace, READ_REFUSED, &temp);

    // The app's store over that keychain (no vault) passes the reason on.
    let data = temp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let store = ActiveStore::new(Box::new(keychain), vault_file(&data));
    every_feature_says(&store, &workspace, READ_REFUSED, &temp);
}

/// Asks the agent for r1's credentials on a thread of its own: the answer,
/// or a failure if none comes within ten seconds (a hang).
fn ask_agent(folder: &Path, workspace: Workspace) -> String {
    let endpoint = AgentEndpoint::for_data_folder(folder);
    let (sender, answer) = mpsc::channel();
    thread::spawn(move || {
        let agent = AgentStore::connect(&endpoint)
            .unwrap()
            .expect("the agent listens");
        let answer = Profiles::new(&agent, &workspace).host_profile(&HostId::new("r1.yaml"));
        let _ = sender.send(
            answer
                .map(|profile| profile.username)
                .map_err(|e| e.to_string()),
        );
    });
    match answer.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(username)) => panic!("the agent gave {username}'s profile"),
        Ok(Err(message)) => message,
        Err(_) => panic!("the agent did not answer within ten seconds"),
    }
}

#[test]
fn the_credential_agent_passes_on_a_refusing_keychain_and_a_locked_vault_at_once() {
    let temp = TempDir::new().unwrap();
    let data = temp.path().join("data");
    fs::create_dir_all(&data).unwrap();

    let server = AgentServer::start(
        AgentEndpoint::for_data_folder(&data),
        Arc::new(refusing_keychain()),
    )
    .unwrap();
    let message = ask_agent(&data, workspace(&temp));
    assert!(message.contains(READ_REFUSED), "{message}");
    drop(server);

    let other = TempDir::new().unwrap();
    let data = other.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let store = Arc::new(locked_vault(&data));
    let _server = AgentServer::start(AgentEndpoint::for_data_folder(&data), store).unwrap();
    let message = ask_agent(&data, workspace(&other));
    assert!(message.contains("locked"), "{message}");
}
