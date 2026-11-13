//! App-level state: what the app remembers between launches, stored per user
//! outside any workspace.

use std::fs;
use std::path::PathBuf;

use netterm_core::app_state::AppState;
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
