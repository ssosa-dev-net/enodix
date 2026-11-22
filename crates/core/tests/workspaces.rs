//! Several workspaces side by side: creating an empty one, adopting an
//! existing folder, and keeping each one's credential profiles apart under
//! a stable workspace identifier (#25).

use std::fs;

use enodix_core::credentials::{
    prepare_workspace, AuthDraft, CredentialProfile, CredentialStore, MemoryStore, ProfileAuth,
    ProfileDraft, Profiles,
};
use enodix_core::workspace::{Workspace, WORKSPACE_FILE};
use tempfile::TempDir;

fn draft(name: &str, username: &str) -> ProfileDraft {
    ProfileDraft {
        name: name.into(),
        username: username.into(),
        auth: AuthDraft::Password {
            password: Some(format!("{username}-secret")),
        },
        enable_secret: None,
    }
}

fn usernames(store: &MemoryStore, workspace: &Workspace) -> Vec<String> {
    Profiles::new(store, workspace)
        .list()
        .unwrap()
        .into_iter()
        .map(|profile| format!("{}={}", profile.name, profile.username))
        .collect()
}

#[test]
fn a_new_workspace_is_created_in_a_folder_that_does_not_exist_yet() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("customers").join("acme");

    let workspace = Workspace::create(&root).unwrap();

    assert!(root.is_dir());
    assert!(root.join(WORKSPACE_FILE).is_file());
    assert!(workspace.id().is_some());
    assert!(workspace.tree().hosts.is_empty());
    assert!(workspace.tree().folders.is_empty());
    assert!(workspace.problems().is_empty());
    assert_eq!(workspace.tree().name, "acme");
}

#[test]
fn a_new_workspace_can_go_in_an_empty_folder_even_one_holding_a_git_repository() {
    let root = TempDir::new().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();

    let workspace = Workspace::create(root.path()).unwrap();

    assert!(workspace.id().is_some());
    assert!(root.path().join(".git").is_dir());
}

#[test]
fn a_new_workspace_is_refused_in_a_folder_that_has_files() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("notes.txt"), "mine").unwrap();

    let error = Workspace::create(root.path()).unwrap_err();

    // It says what to do instead, in the words of the welcome view (#116).
    assert_eq!(
        error.reason,
        "the folder is not empty; open it as a workspace instead, or create one in an empty folder"
    );
    assert!(!root.path().join(WORKSPACE_FILE).exists());
    assert_eq!(
        fs::read_to_string(root.path().join("notes.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn opening_a_folder_that_is_not_there_says_so_and_what_to_do() {
    let parent = TempDir::new().unwrap();
    let gone = parent.path().join("acme");

    let error = Workspace::open(&gone).unwrap_err();

    // Not the system's own words ("os error 2"), and a way on (#116).
    assert_eq!(
        error.reason,
        "there is no such folder; check the path, or create a new workspace there"
    );
}

#[test]
fn opening_a_file_as_a_workspace_says_to_choose_its_folder() {
    let parent = TempDir::new().unwrap();
    let file = parent.path().join("hosts.csv");
    fs::write(&file, "name,address\n").unwrap();

    let error = Workspace::open(&file).unwrap_err();

    assert_eq!(
        error.reason,
        "it is a file, not a folder; choose the folder that holds the host files"
    );
}

#[test]
fn a_new_workspace_is_refused_where_a_file_is() {
    let parent = TempDir::new().unwrap();
    let file = parent.path().join("acme");
    fs::write(&file, "").unwrap();

    assert!(Workspace::create(&file).is_err());
}

#[test]
fn every_new_workspace_has_its_own_identifier_and_keeps_it_when_reopened() {
    let parent = TempDir::new().unwrap();
    let first = Workspace::create(parent.path().join("acme")).unwrap();
    let second = Workspace::create(parent.path().join("globex")).unwrap();

    assert_ne!(first.id(), second.id());
    let reopened = Workspace::open(parent.path().join("acme")).unwrap();
    assert_eq!(reopened.id(), first.id());
}

#[test]
fn the_identifier_file_is_not_a_host() {
    let parent = TempDir::new().unwrap();
    let workspace = Workspace::create(parent.path().join("acme")).unwrap();

    let reopened = Workspace::open(workspace.root()).unwrap();

    assert!(reopened.tree().hosts.is_empty());
    assert!(reopened.problems().is_empty());
}

#[test]
fn an_existing_folder_gets_an_identifier_when_prepared_and_keeps_its_files() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join("r1.yaml"), "address: 192.0.2.1\n").unwrap();
    let store = MemoryStore::new();
    let mut workspace = Workspace::open(root.path()).unwrap();
    assert_eq!(workspace.id(), None);

    prepare_workspace(&store, &mut workspace).unwrap();

    let id = workspace.id().expect("an identifier").to_owned();
    assert_eq!(workspace.tree().hosts.len(), 1);
    assert_eq!(
        fs::read_to_string(root.path().join("r1.yaml")).unwrap(),
        "address: 192.0.2.1\n"
    );
    // Preparing again changes nothing.
    prepare_workspace(&store, &mut workspace).unwrap();
    assert_eq!(workspace.id(), Some(id.as_str()));
    assert_eq!(
        Workspace::open(root.path()).unwrap().id(),
        Some(id.as_str())
    );
}

#[test]
fn two_workspaces_with_a_profile_of_the_same_name_keep_their_own() {
    let parent = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let acme = Workspace::create(parent.path().join("acme")).unwrap();
    let globex = Workspace::create(parent.path().join("globex")).unwrap();

    Profiles::new(&store, &acme)
        .save(None, &draft("core", "acme-admin"))
        .unwrap();
    Profiles::new(&store, &globex)
        .save(None, &draft("core", "globex-admin"))
        .unwrap();

    assert_eq!(usernames(&store, &acme), ["core=acme-admin"]);
    assert_eq!(usernames(&store, &globex), ["core=globex-admin"]);
    Profiles::new(&store, &acme).delete("core").unwrap();
    assert_eq!(usernames(&store, &acme), Vec::<String>::new());
    assert_eq!(usernames(&store, &globex), ["core=globex-admin"]);
}

#[test]
fn two_workspaces_in_folders_of_the_same_name_keep_their_own_profiles() {
    let parent = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let first = Workspace::create(parent.path().join("a").join("lab")).unwrap();
    let second = Workspace::create(parent.path().join("b").join("lab")).unwrap();

    Profiles::new(&store, &first)
        .save(None, &draft("core", "first"))
        .unwrap();

    assert_eq!(usernames(&store, &second), Vec::<String>::new());
}

#[test]
fn a_workspace_keeps_its_profiles_when_its_folder_moves() {
    let parent = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let before = parent.path().join("acme");
    let workspace = Workspace::create(&before).unwrap();
    Profiles::new(&store, &workspace)
        .save(None, &draft("core", "admin"))
        .unwrap();
    drop(workspace);

    let after = parent.path().join("archive").join("acme-2025");
    fs::create_dir(parent.path().join("archive")).unwrap();
    fs::rename(&before, &after).unwrap();
    let moved = Workspace::open(&after).unwrap();

    assert_eq!(usernames(&store, &moved), ["core=admin"]);
}

#[test]
fn profiles_kept_under_the_folder_path_move_to_the_identifier() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let mut workspace = Workspace::open(root.path()).unwrap();
    // As earlier versions kept them: under the folder's path.
    let by_path = workspace.credential_scope();
    Profiles::new(&store, &workspace)
        .save(None, &draft("core", "admin"))
        .unwrap();
    Profiles::new(&store, &workspace)
        .save(None, &draft("edge", "operator"))
        .unwrap();

    prepare_workspace(&store, &mut workspace).unwrap();

    assert_ne!(workspace.credential_scope(), by_path);
    assert_eq!(
        usernames(&store, &workspace),
        ["core=admin", "edge=operator"]
    );
    assert!(store.list(&by_path).unwrap().is_empty());
}

#[test]
fn a_profile_already_kept_under_the_identifier_is_not_replaced_by_an_old_one() {
    let root = TempDir::new().unwrap();
    let store = MemoryStore::new();
    let mut workspace = Workspace::open(root.path()).unwrap();
    let by_path = workspace.credential_scope();
    Profiles::new(&store, &workspace)
        .save(None, &draft("core", "old"))
        .unwrap();
    prepare_workspace(&store, &mut workspace).unwrap();
    Profiles::new(&store, &workspace)
        .save(Some("core"), &draft("core", "new"))
        .unwrap();
    // An old entry under the path again (written by an older version of
    // the app still in use, say).
    store
        .put(
            &by_path,
            &CredentialProfile {
                name: "Core".into(),
                username: "stale".into(),
                auth: ProfileAuth::Agent,
                enable_secret: None,
            },
        )
        .unwrap();

    prepare_workspace(&store, &mut workspace).unwrap();

    assert_eq!(usernames(&store, &workspace), ["core=new"]);
    // The old one is left where it was rather than lost.
    assert_eq!(store.list(&by_path).unwrap().len(), 1);
}

#[test]
fn a_damaged_identifier_file_is_reported_and_never_overwritten() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(WORKSPACE_FILE), "id: [\n").unwrap();
    let store = MemoryStore::new();
    let mut workspace = Workspace::open(root.path()).unwrap();

    assert_eq!(workspace.id(), None);
    assert_eq!(workspace.problems().len(), 1);
    assert_eq!(workspace.problems()[0].file, WORKSPACE_FILE);
    assert!(prepare_workspace(&store, &mut workspace).is_err());
    assert_eq!(
        fs::read_to_string(root.path().join(WORKSPACE_FILE)).unwrap(),
        "id: [\n"
    );
}

#[test]
fn an_identifier_that_is_not_plain_text_is_reported() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(WORKSPACE_FILE), "id: \"a:b c\"\n").unwrap();

    let workspace = Workspace::open(root.path()).unwrap();

    assert_eq!(workspace.id(), None);
    assert_eq!(workspace.problems()[0].file, WORKSPACE_FILE);
}

#[test]
fn an_identifier_file_without_an_identifier_gets_one_and_keeps_its_other_keys() {
    let root = TempDir::new().unwrap();
    fs::write(root.path().join(WORKSPACE_FILE), "owner: neteng\n").unwrap();
    let store = MemoryStore::new();
    let mut workspace = Workspace::open(root.path()).unwrap();
    assert!(workspace.problems().is_empty());

    prepare_workspace(&store, &mut workspace).unwrap();

    let text = fs::read_to_string(root.path().join(WORKSPACE_FILE)).unwrap();
    assert!(text.contains("owner: neteng"), "{text}");
    assert!(text.contains(workspace.id().unwrap()), "{text}");
}

#[test]
fn an_identifier_written_outside_the_app_is_picked_up_on_refresh() {
    let root = TempDir::new().unwrap();
    let mut workspace = Workspace::open(root.path()).unwrap();

    fs::write(root.path().join(WORKSPACE_FILE), "id: from-git\n").unwrap();

    assert!(workspace.refresh().unwrap());
    assert_eq!(workspace.id(), Some("from-git"));
}
