//! App-level state: what the app remembers between launches, stored per user
//! outside any workspace.

use std::fs;
use std::path::{Path, PathBuf};

use enodix_core::app_state::{resolved_path, AppState, ForgetError, KnownWorkspace};
use tempfile::TempDir;

#[test]
fn the_last_opened_workspace_is_remembered_across_launches() {
    let data = TempDir::new().unwrap();
    let workspace = PathBuf::from("D:/customers/acme");

    let mut state = AppState::load(data.path());
    assert_eq!(state.last_workspace, None);
    state.last_workspace = Some(workspace.clone());
    state.save(data.path()).unwrap();

    assert_eq!(AppState::load(data.path()).last_workspace, Some(workspace));
}

#[test]
fn the_data_folder_is_created_when_saving() {
    let data = TempDir::new().unwrap();
    let folder = data.path().join("not/yet/there");

    AppState {
        last_workspace: Some("/srv/net".into()),
        ..AppState::default()
    }
    .save(&folder)
    .unwrap();

    assert_eq!(
        AppState::load(&folder).last_workspace,
        Some(PathBuf::from("/srv/net"))
    );
}

#[test]
fn an_unreadable_state_file_starts_the_app_afresh() {
    let data = TempDir::new().unwrap();
    AppState::default().save(data.path()).unwrap();
    for entry in fs::read_dir(data.path()).unwrap() {
        fs::write(entry.unwrap().path(), "last_workspace: [").unwrap();
    }

    assert_eq!(AppState::load(data.path()), AppState::default());
}

/// Folders standing in for workspaces, each created under `parent`.
fn folders(parent: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|name| {
            let folder = parent.join(name);
            fs::create_dir_all(&folder).unwrap();
            folder
        })
        .collect()
}

fn paths(known: &[KnownWorkspace]) -> Vec<PathBuf> {
    known.iter().map(|known| known.path.clone()).collect()
}

/// Folders as the list keeps them (resolved, so that each folder has one
/// spelling).
fn resolved(folders: &[&PathBuf]) -> Vec<PathBuf> {
    folders.iter().map(|folder| resolved_path(folder)).collect()
}

#[test]
fn opened_workspaces_are_listed_most_recent_first_and_kept_across_launches() {
    let data = TempDir::new().unwrap();
    let parent = TempDir::new().unwrap();
    let [acme, globex] = folders(parent.path(), &["acme", "globex"])
        .try_into()
        .unwrap();

    let mut state = AppState::load(data.path());
    state.remember(&acme);
    state.remember(&globex);
    state.save(data.path()).unwrap();

    let state = AppState::load(data.path());
    assert_eq!(
        paths(&state.known_workspaces(None)),
        resolved(&[&globex, &acme])
    );
    assert_eq!(state.last_workspace, Some(resolved_path(&globex)));
}

#[test]
fn opening_a_known_workspace_again_moves_it_up_without_listing_it_twice() {
    let parent = TempDir::new().unwrap();
    let [acme, globex] = folders(parent.path(), &["acme", "globex"])
        .try_into()
        .unwrap();
    let mut state = AppState::default();
    state.remember(&acme);
    state.remember(&globex);

    // The same folder, written another way.
    state.remember(&globex.join("..").join("acme"));

    let known = state.known_workspaces(None);
    assert_eq!(known.len(), 2);
    assert_eq!(known[0].name, "acme");
    assert_eq!(known[1].path, resolved_path(&globex));
}

#[test]
fn a_relative_path_is_remembered_in_full() {
    let mut state = AppState::default();

    state.remember(Path::new("relative-workspace"));

    let known = state.known_workspaces(None);
    assert!(known[0].path.is_absolute(), "{:?}", known[0].path);
    assert_eq!(known[0].name, "relative-workspace");
}

#[test]
fn the_list_shows_names_which_one_is_open_and_which_folders_are_gone() {
    let parent = TempDir::new().unwrap();
    let [acme, globex] = folders(parent.path(), &["acme", "globex"])
        .try_into()
        .unwrap();
    let gone = parent.path().join("initech");
    let mut state = AppState::default();
    state.remember(&gone);
    state.remember(&acme);
    state.remember(&globex);

    let known = state.known_workspaces(Some(&acme));

    let summary: Vec<_> = known
        .iter()
        .map(|known| (known.name.as_str(), known.open, known.available))
        .collect();
    assert_eq!(
        summary,
        [
            ("globex", false, true),
            ("acme", true, true),
            ("initech", false, false)
        ]
    );
}

#[test]
fn forgetting_a_workspace_takes_it_off_the_list_and_leaves_its_folder() {
    let parent = TempDir::new().unwrap();
    let [acme, globex] = folders(parent.path(), &["acme", "globex"])
        .try_into()
        .unwrap();
    fs::write(acme.join("r1.yaml"), "address: 192.0.2.1\n").unwrap();
    let mut state = AppState::default();
    state.remember(&globex);
    state.remember(&acme);

    assert_eq!(state.forget(&acme, Some(&globex)), Ok(true));

    assert_eq!(paths(&state.known_workspaces(None)), resolved(&[&globex]));
    // It was the last one opened: it is not reopened on the next launch.
    assert_eq!(state.last_workspace, None);
    assert_eq!(
        fs::read_to_string(acme.join("r1.yaml")).unwrap(),
        "address: 192.0.2.1\n"
    );
    assert_eq!(state.forget(&acme, None), Ok(false));
}

#[test]
fn the_open_workspace_cannot_be_forgotten() {
    let parent = TempDir::new().unwrap();
    let [acme] = folders(parent.path(), &["acme"]).try_into().unwrap();
    let mut state = AppState::default();
    state.remember(&acme);

    // However its path is written.
    let refused = state.forget(&acme.join("..").join("acme"), Some(&acme));

    assert_eq!(refused, Err(ForgetError::Open));
    assert_eq!(paths(&state.known_workspaces(None)), resolved(&[&acme]));
}

#[test]
fn a_folder_has_one_resolved_path_however_it_is_written() {
    let parent = TempDir::new().unwrap();
    let [acme] = folders(parent.path(), &["acme"]).try_into().unwrap();

    let other = resolved_path(&acme.join(".").join("..").join("acme"));

    assert_eq!(other, resolved_path(&acme));
    assert!(other.is_absolute());
    // Windows' `\\?\` form stays out of what users see.
    assert!(!other.display().to_string().starts_with(r"\\?\"));
}

#[test]
fn a_damaged_state_file_is_kept_aside_rather_than_written_over() {
    let data = TempDir::new().unwrap();
    let file = data.path().join("state.yaml");
    fs::write(
        &file,
        "workspaces: [
",
    )
    .unwrap();

    let mut state = AppState::load(data.path());
    state.remember(Path::new("/srv/acme"));
    state.save(data.path()).unwrap();

    assert_eq!(
        fs::read_to_string(data.path().join("state.yaml.damaged")).unwrap(),
        "workspaces: [\n"
    );
}

#[test]
fn forgetting_another_workspace_keeps_the_last_one_to_reopen() {
    let parent = TempDir::new().unwrap();
    let [acme, globex] = folders(parent.path(), &["acme", "globex"])
        .try_into()
        .unwrap();
    let mut state = AppState::default();
    state.remember(&acme);
    state.remember(&globex);

    state.forget(&acme, Some(&globex)).unwrap();

    assert_eq!(state.last_workspace, Some(resolved_path(&globex)));
}

#[test]
fn a_state_file_from_before_the_list_lists_its_last_workspace() {
    let data = TempDir::new().unwrap();
    fs::write(
        data.path().join("state.yaml"),
        "last_workspace: /srv/customers/acme\n",
    )
    .unwrap();

    let state = AppState::load(data.path());

    assert_eq!(
        paths(&state.known_workspaces(None)),
        [PathBuf::from("/srv/customers/acme")]
    );
}
