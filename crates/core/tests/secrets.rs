//! Named secrets: a name and a value per workspace, kept in a credential
//! store (here the in-memory one) apart from the credential profiles.
//! Listing shows names only; the value is read back by name for later
//! consumers.

use std::fs;

use netterm_core::credentials::{
    prepare_workspace, AuthDraft, CredentialStore, MemoryStore, ProfileDraft, Profiles,
    SecretError, SecretValue, Secrets, MAX_SECRET_NAME,
};
use netterm_core::workspace::Workspace;
use tempfile::TempDir;

fn workspace(name: &str) -> (TempDir, Workspace) {
    let temp = TempDir::new().unwrap();
    let workspace = Workspace::create(temp.path().join(name)).unwrap();
    (temp, workspace)
}

fn value(secrets: &Secrets<'_>, name: &str) -> Option<String> {
    secrets
        .get(name)
        .unwrap()
        .map(|value| value.expose().to_owned())
}

#[test]
fn secrets_are_added_listed_replaced_and_deleted() {
    let (_temp, workspace) = workspace("acme");
    let store = MemoryStore::new();
    let secrets = Secrets::new(&store, &workspace);
    assert!(secrets.list().unwrap().is_empty());

    assert_eq!(
        secrets.add(" netbox-token ", "t0ken").unwrap(),
        "netbox-token"
    );
    secrets.add("Backup.Key_2", "k3y").unwrap();
    secrets.add("api", "a").unwrap();

    // Names only, by name ignoring case.
    assert_eq!(
        secrets.list().unwrap(),
        ["api", "Backup.Key_2", "netbox-token"]
    );
    assert_eq!(value(&secrets, "netbox-token").as_deref(), Some("t0ken"));

    secrets.replace("netbox-token", "r0tated").unwrap();
    assert_eq!(value(&secrets, "netbox-token").as_deref(), Some("r0tated"));
    assert_eq!(secrets.list().unwrap().len(), 3);

    secrets.delete("netbox-token").unwrap();
    assert_eq!(value(&secrets, "netbox-token"), None);
    assert_eq!(secrets.list().unwrap(), ["api", "Backup.Key_2"]);
    assert_eq!(
        secrets.delete("netbox-token"),
        Err(SecretError::NotFound("netbox-token".into()))
    );
}

#[test]
fn names_match_ignoring_case() {
    let (_temp, workspace) = workspace("acme");
    let store = MemoryStore::new();
    let secrets = Secrets::new(&store, &workspace);
    secrets.add("NetBox-Token", "t0ken").unwrap();

    assert_eq!(value(&secrets, "netbox-token").as_deref(), Some("t0ken"));
    assert_eq!(
        secrets.add("NETBOX-TOKEN", "other"),
        Err(SecretError::AlreadyExists("NETBOX-TOKEN".into()))
    );
    secrets.replace("netbox-TOKEN", "new").unwrap();
    // Replacing keeps the name as it was first written.
    assert_eq!(secrets.list().unwrap(), ["NetBox-Token"]);
    assert_eq!(value(&secrets, "NetBox-Token").as_deref(), Some("new"));
    secrets.delete("NETBOX-token").unwrap();
    assert!(secrets.list().unwrap().is_empty());
}

#[test]
fn bad_names_and_values_are_refused_and_nothing_is_stored() {
    let (_temp, workspace) = workspace("acme");
    let store = MemoryStore::new();
    let secrets = Secrets::new(&store, &workspace);
    let too_long = "x".repeat(MAX_SECRET_NAME + 1);

    for name in [
        "",
        "  ",
        "my token",
        "token/1",
        "töken",
        "a:b",
        too_long.as_str(),
    ] {
        assert!(
            matches!(secrets.add(name, "v"), Err(SecretError::Invalid(_))),
            "{name:?} was accepted"
        );
    }
    assert!(matches!(
        secrets.add("token", ""),
        Err(SecretError::Invalid(_))
    ));
    assert!(secrets.list().unwrap().is_empty());

    secrets
        .add("x".repeat(MAX_SECRET_NAME).as_str(), "v")
        .unwrap();
    assert_eq!(
        secrets.replace("missing", "v"),
        Err(SecretError::NotFound("missing".into()))
    );
    secrets.add("token", "v").unwrap();
    assert!(matches!(
        secrets.replace("token", ""),
        Err(SecretError::Invalid(_))
    ));
    assert_eq!(value(&secrets, "token").as_deref(), Some("v"));
}

#[test]
fn a_profile_and_a_secret_with_the_same_name_coexist() {
    let (_temp, workspace) = workspace("acme");
    let store = MemoryStore::new();
    let profiles = Profiles::new(&store, &workspace);
    let secrets = Secrets::new(&store, &workspace);
    let draft = ProfileDraft {
        name: "core".into(),
        username: "noc".into(),
        auth: AuthDraft::Password {
            password: Some("pw".into()),
        },
        enable_secret: None,
    };

    profiles.save(None, &draft).unwrap();
    secrets.add("CORE", "t0ken").unwrap();

    assert_eq!(profiles.list().unwrap().len(), 1);
    assert_eq!(secrets.list().unwrap(), ["CORE"]);

    secrets.delete("core").unwrap();
    assert!(profiles.profile("core").unwrap().is_some());

    secrets.add("core", "t0ken").unwrap();
    profiles.delete("core").unwrap();
    assert_eq!(value(&secrets, "core").as_deref(), Some("t0ken"));
    assert!(profiles.list().unwrap().is_empty());
    assert!(store
        .list(&workspace.credential_scope())
        .unwrap()
        .is_empty());
}

#[test]
fn each_workspace_has_its_own_secrets() {
    let (_a, acme) = workspace("acme");
    let (_b, globex) = workspace("globex");
    let store = MemoryStore::new();

    Secrets::new(&store, &acme).add("token", "acme").unwrap();
    Secrets::new(&store, &globex)
        .add("token", "globex")
        .unwrap();

    assert_eq!(
        value(&Secrets::new(&store, &acme), "token").as_deref(),
        Some("acme")
    );
    assert_eq!(
        value(&Secrets::new(&store, &globex), "token").as_deref(),
        Some("globex")
    );
    assert_eq!(
        store.secret_names(&acme.credential_scope()).unwrap(),
        ["token"]
    );
}

#[test]
fn secrets_kept_under_the_folder_path_move_to_the_workspace_id() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("acme");
    fs::create_dir_all(&root).unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    let store = MemoryStore::new();
    let by_path = workspace.path_credential_scope();
    store
        .put_secret(&by_path, "token", &SecretValue::new("old"))
        .unwrap();

    prepare_workspace(&store, &mut workspace).unwrap();

    assert_ne!(workspace.credential_scope(), by_path);
    let secrets = Secrets::new(&store, &workspace);
    assert_eq!(value(&secrets, "token").as_deref(), Some("old"));
    assert!(store.secret_names(&by_path).unwrap().is_empty());
}

#[test]
fn debug_output_hides_values() {
    let secret = SecretValue::new("hunter2-very-secret");
    let shown = format!("{secret:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert_eq!(secret.expose(), "hunter2-very-secret");
}
