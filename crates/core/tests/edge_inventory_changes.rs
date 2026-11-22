//! Edge tests (#117): the inventory changing under the app. A git pull that
//! rewrites hundreds of files, a branch switch that replaces the tree, an
//! editor saving by rename, a folder renamed outside, and the workspace
//! folder itself going away and coming back (what a mapped network drive
//! that disconnects does, simulated with a folder moved away and back, and
//! removed and made again).
//!
//! What must hold: once the changes stop, the app shows what is on disk;
//! no edit of a path that went away writes anything; nothing is lost
//! silently. Windows and Linux alike unless a test says otherwise.

mod support;

use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use enodix_core::workspace::{
    Changes, Condition, EditError, HostId, SavedFilter, Unavailable, WatchOptions, Workspace,
    WorkspaceWatcher,
};
use support::workspaces::files;
use tempfile::TempDir;

/// How long the folder must stay quiet for the changes to count as over.
const QUIET: Duration = Duration::from_secs(2);
/// The longest the whole of a test's changes may take to be reported.
const WITHIN: Duration = Duration::from_secs(30);

fn host(index: usize, version: &str) -> String {
    format!(
        "address: 10.0.{}.{}\ndescription: {version}\ntags: [gen]\n",
        index / 256,
        index % 256
    )
}

/// A workspace of `count` hosts in ten folders.
fn generated(count: usize) -> TempDir {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    fs::write(root.join("_workspace.yaml"), "id: edge-changes\n").unwrap();
    for index in 0..count {
        let folder = root.join(format!("site{}", index % 10));
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join(format!("host-{index:04}.yaml")),
            host(index, "one"),
        )
        .unwrap();
    }
    temp
}

struct Watched {
    workspace: Workspace,
    changes: Receiver<Changes>,
    _watcher: WorkspaceWatcher,
}

fn watch(root: &Path) -> Watched {
    let workspace = Workspace::open(root).unwrap();
    let (sender, changes) = mpsc::channel();
    let watcher = WorkspaceWatcher::start(root, WatchOptions::default(), move |change| {
        let _ = sender.send(change);
    })
    .unwrap();
    Watched {
        workspace,
        changes,
        _watcher: watcher,
    }
}

impl Watched {
    /// Refreshes on every notification, as the app does, until the folder
    /// has been quiet for [`QUIET`] after at least one; how many there were.
    fn follow(&mut self) -> usize {
        let started = Instant::now();
        let mut notifications = 0;
        loop {
            match self.changes.recv_timeout(QUIET) {
                Ok(_) => {
                    notifications += 1;
                    self.workspace.refresh().unwrap();
                }
                Err(_) if notifications > 0 => return notifications,
                Err(_) => assert!(started.elapsed() < WITHIN, "no notification at all"),
            }
            assert!(
                started.elapsed() < WITHIN,
                "changes still reported after {WITHIN:?}"
            );
        }
    }

    /// What the app shows is what is on disk.
    fn shows_the_disk(&self, root: &Path) {
        let fresh = Workspace::open(root).unwrap();
        assert!(
            self.workspace.tree() == fresh.tree(),
            "the tree shown differs from the one on disk"
        );
        assert_eq!(self.workspace.problems(), fresh.problems());
    }
}

#[test]
fn a_pull_rewriting_hundreds_of_files_ends_with_the_tree_on_disk() {
    let temp = generated(400);
    let root = temp.path();
    let mut watched = watch(root);

    // The pull: every host rewritten, fifty added in a new folder, fifty
    // removed, one folder renamed, as fast as git writes.
    for index in 0..400 {
        let path = root.join(format!("site{}/host-{index:04}.yaml", index % 10));
        if index % 8 == 0 {
            fs::remove_file(path).unwrap();
        } else {
            fs::write(path, host(index, "two")).unwrap();
        }
    }
    fs::create_dir(root.join("site-new")).unwrap();
    for index in 400..450 {
        fs::write(
            root.join(format!("site-new/host-{index:04}.yaml")),
            host(index, "two"),
        )
        .unwrap();
    }
    fs::rename(root.join("site3"), root.join("site3-renamed")).unwrap();

    let notifications = watched.follow();
    watched.shows_the_disk(root);
    let shown = watched.workspace.search("", 1000).total;
    assert_eq!(shown, 400 - 50 + 50);
    println!("{notifications} notifications");
}

#[test]
fn a_branch_switch_that_replaces_the_whole_tree_ends_with_the_new_tree() {
    let temp = generated(200);
    let root = temp.path();
    let mut watched = watch(root);

    for site in 0..10 {
        fs::remove_dir_all(root.join(format!("site{site}"))).unwrap();
    }
    for index in 0..150 {
        let folder = root.join(format!("branch-b/rack{}", index % 5));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("b-{index:04}.yaml")), host(index, "b")).unwrap();
    }
    fs::write(root.join("_filters.yaml"), "Gen:\n  tags: gen\n").unwrap();

    watched.follow();
    watched.shows_the_disk(root);
    assert_eq!(watched.workspace.search("", 1000).total, 150);
    assert_eq!(watched.workspace.filters().len(), 1);
}

#[test]
fn an_editor_saving_by_rename_shows_as_the_new_file_and_its_temporary_files_never_as_hosts() {
    let temp = generated(10);
    let root = temp.path();
    let mut watched = watch(root);
    let id = HostId::new("site1/host-0001.yaml");
    let path = root.join(id.as_str());

    // Write beside, rename over (most editors' safe save).
    fs::write(
        root.join("site1/host-0001.yaml.tmp"),
        host(1, "saved by rename"),
    )
    .unwrap();
    fs::rename(root.join("site1/host-0001.yaml.tmp"), &path).unwrap();
    // A hidden swap file, as vim keeps one.
    fs::write(root.join("site1/.host-0001.yaml.swp"), "binary\0").unwrap();
    // Keep a backup, then write anew (another way editors save).
    fs::rename(
        root.join("site2/host-0002.yaml"),
        root.join("site2/host-0002.yaml~"),
    )
    .unwrap();
    fs::write(
        root.join("site2/host-0002.yaml"),
        host(2, "saved after a backup"),
    )
    .unwrap();

    watched.follow();
    watched.shows_the_disk(root);
    let workspace = &watched.workspace;
    assert_eq!(
        workspace.host(&id).unwrap().description.as_deref(),
        Some("saved by rename")
    );
    assert_eq!(
        workspace
            .host(&HostId::new("site2/host-0002.yaml"))
            .unwrap()
            .description
            .as_deref(),
        Some("saved after a backup")
    );
    assert_eq!(
        workspace.search("", 100).total,
        10,
        "no temporary file is a host"
    );
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
}

#[test]
fn a_folder_renamed_outside_is_followed_and_no_edit_of_its_old_path_recreates_it() {
    let temp = generated(20);
    let root = temp.path();
    let mut workspace = Workspace::open(root).unwrap();
    let id = HostId::new("site4/host-0004.yaml");
    let mut host = workspace.host(&id).unwrap().clone();
    host.description = Some("edited in the app".into());

    fs::rename(root.join("site4"), root.join("site4-moved")).unwrap();
    // Before the reload: every edit of the old path is refused.
    assert!(workspace.write_host(&host).is_err());
    assert!(workspace
        .set_folder_description("site4", Some("x"))
        .is_err());
    assert!(workspace.create_host("site4", "new", "10.9.9.9").is_err());
    assert!(workspace.create_folder("site4", "below").is_err());
    assert!(workspace.rename_host(&id, "renamed").is_err());
    assert!(!root.join("site4").exists(), "the old folder came back");

    assert!(workspace.refresh().unwrap());
    assert!(workspace.host(&id).is_none());
    let moved = HostId::new("site4-moved/host-0004.yaml");
    let mut host = workspace.host(&moved).unwrap().clone();
    host.description = Some("edited in the app".into());
    workspace.write_host(&host).unwrap();
}

/// Every edit the app can make, each refused while the workspace folder is
/// gone.
fn every_edit_is_refused(workspace: &mut Workspace, host: &enodix_core::workspace::Host) {
    let mut edited = host.clone();
    edited.description = Some("edited while gone".into());
    let results: Vec<(&str, Result<(), EditError>)> = vec![
        ("write_host", workspace.write_host(&edited)),
        (
            "create_host",
            workspace.create_host("", "new", "10.9.9.9").map(drop),
        ),
        (
            "create_folder",
            workspace.create_folder("", "new").map(drop),
        ),
        (
            "set_folder_description",
            workspace.set_folder_description("site1", Some("gone")),
        ),
        (
            "save_filter",
            workspace.save_filter(&SavedFilter {
                name: "Gone".into(),
                conditions: vec![Condition::new("tag", "gen")],
            }),
        ),
        ("delete_host", workspace.delete_host(&host.id)),
    ];
    for (edit, result) in results {
        assert!(result.is_err(), "{edit} went through");
    }
}

#[test]
fn a_workspace_folder_that_goes_away_keeps_what_was_shown_writes_nothing_and_recovers() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    let away = parent.path().join("ws-away");
    let temp = generated(30);
    fs::rename(temp.path(), &root).unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    let host = workspace
        .host(&HostId::new("site1/host-0001.yaml"))
        .unwrap()
        .clone();

    // The drive disconnects.
    fs::rename(&root, &away).unwrap();
    assert!(
        workspace.refresh().is_err(),
        "a gone folder is an error, not an empty workspace"
    );
    assert_eq!(
        workspace.search("", 100).total,
        30,
        "what was shown is kept"
    );
    every_edit_is_refused(&mut workspace, &host);
    assert!(!root.exists(), "an edit made the folder again");

    // It comes back.
    fs::rename(&away, &root).unwrap();
    workspace.refresh().unwrap();
    assert_eq!(workspace.search("", 100).total, 30);
    let mut edited = host.clone();
    edited.description = Some("after it came back".into());
    workspace.write_host(&edited).unwrap();
    assert_eq!(files(&root).len(), 30 + 10 + 1);
}

/// How the folder comes back.
#[derive(Debug, Clone, Copy)]
enum Return {
    /// Moved away and back: the same folder.
    MovedBack,
    /// Removed and made again with the same files: another folder at the
    /// same path, as a network drive that reconnects is.
    MadeAgain,
}

fn watcher_after_return(how: Return) {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    let copy = parent.path().join("copy");
    let temp = generated(5);
    fs::rename(temp.path(), &root).unwrap();
    let mut watched = watch(&root);

    match how {
        Return::MovedBack => {
            let away = parent.path().join("away");
            fs::rename(&root, &away).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            fs::rename(&away, &root).unwrap();
        }
        Return::MadeAgain => {
            fs::create_dir(&copy).unwrap();
            for entry in walk(&root) {
                let to = copy.join(entry.strip_prefix(&root).unwrap());
                fs::create_dir_all(to.parent().unwrap()).unwrap();
                fs::copy(&entry, to).unwrap();
            }
            fs::remove_dir_all(&root).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            fs::rename(&copy, &root).unwrap();
        }
    }
    // Let what the going and coming gave be reported first.
    while watched.changes.recv_timeout(QUIET).is_ok() {
        let _ = watched.workspace.refresh();
    }

    // A change once it is back must be reported.
    fs::write(
        root.join("site1/host-0001.yaml"),
        host(1, "after the return"),
    )
    .unwrap();
    let reported = watched.changes.recv_timeout(Duration::from_secs(10));
    assert!(
        reported.is_ok(),
        "{how:?}: a change after the folder came back was not reported"
    );
    watched.workspace.refresh().unwrap();
    assert_eq!(
        watched
            .workspace
            .host(&HostId::new("site1/host-0001.yaml"))
            .unwrap()
            .description
            .as_deref(),
        Some("after the return")
    );
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[test]
fn live_reload_goes_on_after_the_workspace_folder_is_moved_away_and_back() {
    watcher_after_return(Return::MovedBack);
}

/// Windows and Linux: the system's watch ends with the folder it watched;
/// the watcher looks for the folder and watches the new one (#132).
#[test]
fn live_reload_goes_on_after_the_workspace_folder_is_removed_and_made_again() {
    watcher_after_return(Return::MadeAgain);
}

/// Watches `root` checking the folder every 200 ms.
fn watch_checking_often(root: &Path) -> Watched {
    let workspace = Workspace::open(root).unwrap();
    let (sender, changes) = mpsc::channel();
    let options = WatchOptions {
        check_every: Duration::from_millis(200),
        ..WatchOptions::default()
    };
    let watcher = WorkspaceWatcher::start(root, options, move |change| {
        let _ = sender.send(change);
    })
    .unwrap();
    Watched {
        workspace,
        changes,
        _watcher: watcher,
    }
}

impl Watched {
    /// Refreshes on each notification until `until` holds after one, as
    /// the app does; fails after [`WITHIN`].
    fn refresh_until(&mut self, what: &str, until: impl Fn(&Workspace) -> bool) {
        let started = Instant::now();
        loop {
            let left = WITHIN.saturating_sub(started.elapsed());
            match self.changes.recv_timeout(left) {
                Ok(_) => {
                    let _ = self.workspace.refresh();
                    if until(&self.workspace) {
                        return;
                    }
                }
                Err(_) => panic!("{what}: not within {WITHIN:?}"),
            }
        }
    }
}

/// Windows and Linux: the folder going is reported by itself (no file need
/// change after it), the workspace then says it cannot be reached and why,
/// every edit is refused with that reason; once the folder is back the
/// workspace loads it again by itself and the edits work.
#[test]
fn a_workspace_folder_that_cannot_be_reached_says_so_until_it_is_back() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    let away = parent.path().join("ws-away");
    fs::rename(generated(5).keep(), &root).unwrap();
    let mut watched = watch_checking_often(&root);
    let host = watched
        .workspace
        .host(&HostId::new("site1/host-0001.yaml"))
        .unwrap()
        .clone();
    let revision = watched.workspace.revision();

    fs::rename(&root, &away).unwrap();
    watched.refresh_until("the folder going was not seen", |workspace| {
        workspace.unavailable().is_some()
    });
    let workspace = &mut watched.workspace;
    let unavailable = workspace.unavailable().unwrap().clone();
    assert!(
        matches!(unavailable, Unavailable::Unreachable { .. }),
        "{unavailable:?}"
    );
    assert!(
        unavailable.to_string().contains("cannot be reached"),
        "{unavailable}"
    );
    assert!(
        workspace.revision() > revision,
        "a newer revision, so that the window shows it"
    );
    assert_eq!(workspace.search("", 100).total, 5, "what was shown is kept");
    let mut edited = host.clone();
    edited.description = Some("while gone".into());
    let error = workspace.write_host(&edited).unwrap_err();
    assert!(matches!(error, EditError::Unavailable(_)), "{error}");
    assert!(error.to_string().contains("cannot be reached"), "{error}");
    let error = workspace.create_folder("", "new").unwrap_err();
    assert!(matches!(error, EditError::Unavailable(_)), "{error}");
    assert!(!root.exists(), "an edit made the folder again");

    fs::rename(&away, &root).unwrap();
    watched.refresh_until("the folder coming back was not seen", |workspace| {
        workspace.unavailable().is_none()
    });
    watched.workspace.write_host(&edited).unwrap();
    fs::remove_dir_all(&root).unwrap();
}

/// Windows and Linux: the refresh that finds the folder gone says so even
/// before the watcher notices (the app refreshes on any change), and the
/// edits refuse with the reason without a refresh first.
#[test]
fn an_edit_while_the_folder_is_gone_is_refused_with_that_reason() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    fs::rename(generated(3).keep(), &root).unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    fs::rename(&root, parent.path().join("away")).unwrap();
    let error = workspace.create_host("", "new", "10.9.9.9").unwrap_err();
    assert!(matches!(error, EditError::Unavailable(_)), "{error}");
    assert!(workspace.refresh().is_err());
    assert!(workspace.unavailable().is_some());
}

/// Windows and Linux: a folder that comes back holding another workspace
/// (its `_workspace.yaml` names another id) is not taken for the one that
/// was open: what was shown is kept, edits are refused, and it says why.
#[test]
fn a_folder_that_comes_back_as_another_workspace_is_not_taken_for_the_open_one() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    let away = parent.path().join("away");
    fs::rename(generated(4).keep(), &root).unwrap();
    let mut workspace = Workspace::open(&root).unwrap();
    assert_eq!(workspace.id(), Some("edge-changes"));

    fs::rename(&root, &away).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("_workspace.yaml"), "id: someone-else\n").unwrap();
    fs::write(root.join("theirs.yaml"), "address: 10.7.7.7\n").unwrap();
    assert!(workspace.refresh().is_err());
    let unavailable = workspace.unavailable().unwrap().clone();
    assert_eq!(
        unavailable,
        Unavailable::OtherWorkspace {
            id: Some("someone-else".into())
        }
    );
    let text = format!("{unavailable} {}", unavailable.explanation());
    for word in [
        "another workspace",
        "someone-else",
        "Credential",
        "open it again",
    ] {
        assert!(text.contains(word), "{word:?} not in {text}");
    }
    assert_eq!(workspace.search("", 100).total, 4, "what was shown is kept");
    let error = workspace.create_host("", "new", "10.9.9.9").unwrap_err();
    assert!(matches!(error, EditError::Unavailable(_)), "{error}");
    assert!(!root.join("new.yaml").exists());

    // Without an identity file at all: not taken either.
    fs::remove_file(root.join("_workspace.yaml")).unwrap();
    assert!(workspace.refresh().is_err());
    assert_eq!(
        workspace.unavailable(),
        Some(&Unavailable::OtherWorkspace { id: None })
    );

    // The right folder back: shown again.
    fs::remove_dir_all(&root).unwrap();
    fs::rename(&away, &root).unwrap();
    assert!(workspace.refresh().unwrap());
    assert_eq!(workspace.unavailable(), None);
    workspace.create_host("", "new", "10.9.9.9").unwrap();
}

/// Windows and Linux: a system watch can end without a word (a network
/// share's connection drops while the folder stays the same): asked to
/// check again, the watcher watches the folder anew and reports.
#[test]
fn asked_to_check_again_the_watcher_watches_the_folder_anew() {
    let temp = generated(3);
    let root = temp.path();
    let watched = watch_checking_often(root);
    watched._watcher.check_again();
    let started = Instant::now();
    let change = watched
        .changes
        .recv_timeout(Duration::from_secs(10))
        .expect("no notification after asking to check again");
    assert!(change.files.is_empty(), "{change:?}");
    println!("reported after {:?}", started.elapsed());

    fs::write(root.join("site1/host-0001.yaml"), host(1, "after")).unwrap();
    let change = watched
        .changes
        .recv_timeout(Duration::from_secs(10))
        .expect("a change after watching anew was not reported");
    assert!(
        change
            .files
            .iter()
            .any(|file| file == "site1/host-0001.yaml"),
        "{change:?}"
    );
}
