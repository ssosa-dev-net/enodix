//! Renaming or moving a host or a folder (#99): what names the hosts
//! concerned follows them. The `jump_host` settings and saved jobs that
//! named an old path are rewritten (only those files, only that value), and
//! the hosts' session logs and config history move to the new path
//! (`history::carry`), an open session's log included. History that cannot
//! move is reported where it stays and is never mixed with another host's.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use netterm_core::configs::{ConfigStore, SaveOutcome};
use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::history::{self, HistoryKind, LeftHistory, Sessions};
use netterm_core::job::{self, JobPlan};
use netterm_core::logging::LogFolder;
use netterm_core::session::{
    SessionEvents, SessionRecord, SessionRecords, SessionState, SessionTarget, TerminalSize,
    Transport,
};
use netterm_core::workspace::{HostId, HostMove, Workspace};
use support::{next_state, output_ending_with, TestSshServer, PASSWORD, PROMPT, USERNAME};
use tempfile::TempDir;

const SIZE: TerminalSize = TerminalSize { cols: 80, rows: 24 };

const SITES_FOLDER: &str = "\
# Every site goes through the bastion.
jump_host: bastion.yaml   # see the runbook
credential_profile: lab
";
const R2: &str = "\
# r2 names the bastion itself, in other letters.
address: 10.1.0.2
jump_host: 'Bastion.yaml'
";
const OTHER: &str = "# Nothing to do with the bastion.\naddress: 10.3.0.1\n";
const JOBS: &str = "\
# The team's jobs.
Version r1:
  snippet: Show version
  host: Sites/Madrid/r1.yaml   # the one
Madrid:
  snippet: Show version
  folder: Sites/Madrid
Far:
  snippet: Show version
  host: Lab/far.yaml
";

/// The files of the `lab` workspace: a bastion every site goes through, a
/// chain of two jump hosts (`Lab/far` through `Lab/mid` through `edge`),
/// saved jobs naming hosts and a folder.
fn lab_files() -> Vec<(&'static str, String)> {
    vec![
        ("_workspace.yaml", "id: lab-1\n".into()),
        ("_snippets.yaml", "Show version: show version\n".into()),
        ("_jobs.yaml", JOBS.into()),
        ("bastion.yaml", "address: 10.0.0.1\n".into()),
        ("edge.yaml", "address: 10.0.0.9\n".into()),
        ("other.yaml", OTHER.into()),
        ("Sites/_folder.yaml", SITES_FOLDER.into()),
        ("Sites/Madrid/r1.yaml", "address: 10.1.0.1\n".into()),
        ("Sites/Madrid/r2.yaml", R2.into()),
        (
            "Lab/mid.yaml",
            "address: 10.2.0.1\njump_host: edge.yaml\n".into(),
        ),
        (
            "Lab/far.yaml",
            "address: 10.2.0.2\njump_host: Lab/mid.yaml\n".into(),
        ),
    ]
}

/// A workspace folder called `lab` made of `files`, opened.
fn workspace(files: &[(&str, String)]) -> (TempDir, PathBuf, Workspace) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("lab");
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
    (temp, root, workspace)
}

fn id(text: &str) -> HostId {
    HostId::new(text)
}

fn read(root: &Path, file: &str) -> String {
    fs::read_to_string(root.join(file)).unwrap_or_else(|error| panic!("{file}: {error}"))
}

/// The ids of the jump hosts `host` is reached through.
fn chain(workspace: &Workspace, host: &str) -> Vec<String> {
    let resolved = workspace.resolve_host(&id(host)).unwrap();
    workspace
        .jump_chain(&resolved)
        .unwrap()
        .iter()
        .map(|hop| hop.host.id.to_string())
        .collect()
}

/// Every file of the lab that is not in `except`, still as it was written,
/// at its path or at the path `moved` gives it.
fn untouched_but(root: &Path, except: &[&str], moved: impl Fn(&str) -> String) {
    for (file, text) in lab_files() {
        let now = moved(file);
        if except.contains(&now.as_str()) {
            continue;
        }
        assert_eq!(read(root, &now), text, "{now} should not be rewritten");
    }
}

/// The saved job `name` planned: the hosts it runs on.
fn plan_saved(workspace: &Workspace, name: &str) -> Vec<String> {
    let store = MemoryStore::new();
    store
        .put(
            &workspace.credential_scope(),
            &CredentialProfile {
                name: "lab".into(),
                username: "admin".into(),
                auth: ProfileAuth::Password {
                    password: "secret".into(),
                },
                enable_secret: None,
            },
        )
        .unwrap();
    let (_, plan): (_, JobPlan) =
        job::plan_saved(workspace, &Profiles::new(&store, workspace), name).unwrap();
    let mut ids: Vec<String> = plan.hosts.iter().map(|h| h.id.to_string()).collect();
    ids.extend(plan.skipped.iter().map(|h| h.id.to_string()));
    ids
}

// --- References ---

#[test]
fn renaming_a_jump_host_rewrites_the_settings_that_name_it_and_nothing_else() {
    let (_temp, root, mut workspace) = workspace(&lab_files());

    let moved = workspace.rename_host(&id("bastion.yaml"), "gw").unwrap();

    assert_eq!(moved.to, id("gw.yaml"));
    assert_eq!(
        moved.hosts,
        [HostMove {
            from: id("bastion.yaml"),
            to: id("gw.yaml"),
        }]
    );
    assert_eq!(
        moved.rewritten,
        ["Sites/_folder.yaml", "Sites/Madrid/r2.yaml"]
    );
    assert!(moved.not_rewritten.is_empty(), "{:?}", moved.not_rewritten);
    // Only the value changed: the comments, the other keys and the layout
    // stay.
    assert_eq!(
        read(&root, "Sites/_folder.yaml"),
        SITES_FOLDER.replace("bastion.yaml", "gw.yaml")
    );
    assert_eq!(
        read(&root, "Sites/Madrid/r2.yaml"),
        R2.replace("'Bastion.yaml'", "gw.yaml")
    );
    untouched_but(
        &root,
        &["Sites/_folder.yaml", "Sites/Madrid/r2.yaml"],
        |file| file.replace("bastion.yaml", "gw.yaml"),
    );
    // The hosts behind it connect through it.
    assert_eq!(chain(&workspace, "Sites/Madrid/r1.yaml"), ["gw.yaml"]);
    assert_eq!(chain(&workspace, "Sites/Madrid/r2.yaml"), ["gw.yaml"]);
}

#[test]
fn a_chain_of_two_jump_hosts_resolves_after_the_middle_one_is_renamed() {
    let (_temp, root, mut workspace) = workspace(&lab_files());

    let moved = workspace.rename_host(&id("Lab/mid.yaml"), "hop").unwrap();

    assert_eq!(moved.rewritten, ["Lab/far.yaml"]);
    assert_eq!(
        read(&root, "Lab/far.yaml"),
        "address: 10.2.0.2\njump_host: Lab/hop.yaml\n"
    );
    assert_eq!(
        chain(&workspace, "Lab/far.yaml"),
        ["edge.yaml", "Lab/hop.yaml"]
    );
}

#[test]
fn renaming_a_folder_rewrites_references_to_every_host_under_it() {
    let (_temp, root, mut workspace) = workspace(&lab_files());

    let moved = workspace.rename_folder("Lab", "Hops").unwrap();

    assert_eq!(moved.to, "Hops");
    assert_eq!(
        moved.hosts,
        [
            HostMove {
                from: id("Lab/far.yaml"),
                to: id("Hops/far.yaml"),
            },
            HostMove {
                from: id("Lab/mid.yaml"),
                to: id("Hops/mid.yaml"),
            },
        ]
    );
    assert_eq!(moved.rewritten, ["Hops/far.yaml", "_jobs.yaml"]);
    assert_eq!(
        read(&root, "Hops/far.yaml"),
        "address: 10.2.0.2\njump_host: Hops/mid.yaml\n"
    );
    assert_eq!(
        read(&root, "_jobs.yaml"),
        JOBS.replace("host: Lab/far.yaml", "host: Hops/far.yaml")
    );
    assert_eq!(
        chain(&workspace, "Hops/far.yaml"),
        ["edge.yaml", "Hops/mid.yaml"]
    );
    assert_eq!(plan_saved(&workspace, "Far"), ["Hops/far.yaml"]);
}

#[test]
fn moving_a_folder_rewrites_the_saved_jobs_naming_its_hosts_or_itself() {
    let (_temp, root, mut workspace) = workspace(&lab_files());

    let moved = workspace.move_folder("Sites", "Lab").unwrap();

    assert_eq!(moved.to, "Lab/Sites");
    assert_eq!(moved.hosts.len(), 2);
    // The folder's own jump host did not move: its settings stay as they
    // are.
    assert_eq!(moved.rewritten, ["_jobs.yaml"]);
    assert_eq!(
        read(&root, "_jobs.yaml"),
        JOBS.replace(
            "host: Sites/Madrid/r1.yaml",
            "host: Lab/Sites/Madrid/r1.yaml"
        )
        .replace("folder: Sites/Madrid", "folder: Lab/Sites/Madrid")
    );
    assert_eq!(read(&root, "Lab/Sites/_folder.yaml"), SITES_FOLDER);
    assert_eq!(
        plan_saved(&workspace, "Version r1"),
        ["Lab/Sites/Madrid/r1.yaml"]
    );
    assert_eq!(
        plan_saved(&workspace, "Madrid"),
        ["Lab/Sites/Madrid/r1.yaml", "Lab/Sites/Madrid/r2.yaml"]
    );
    assert_eq!(
        chain(&workspace, "Lab/Sites/Madrid/r1.yaml"),
        ["bastion.yaml"]
    );
}

#[test]
fn moving_a_host_rewrites_the_saved_job_naming_it() {
    let (_temp, root, mut workspace) = workspace(&lab_files());

    let moved = workspace
        .move_host(&id("Sites/Madrid/r1.yaml"), "")
        .unwrap();

    assert_eq!(moved.to, id("r1.yaml"));
    assert_eq!(moved.rewritten, ["_jobs.yaml"]);
    assert_eq!(
        read(&root, "_jobs.yaml"),
        JOBS.replace("host: Sites/Madrid/r1.yaml", "host: r1.yaml")
    );
    assert_eq!(plan_saved(&workspace, "Version r1"), ["r1.yaml"]);
    // Moved out from under its folder's jump host.
    assert!(chain(&workspace, "r1.yaml").is_empty());
}

#[test]
fn a_saved_job_follows_a_folder_with_no_hosts_and_a_host_named_with_backslashes() {
    let mut files = lab_files();
    files.retain(|(file, _)| *file != "_jobs.yaml");
    files.push(("Spare/.gitkeep", String::new()));
    let jobs = "\
Spare:
  snippet: Show version
  folder: Spare
Mid:
  snippet: Show version
  host: Lab\\mid.yaml
";
    files.push(("_jobs.yaml", jobs.into()));
    let (_temp, root, mut workspace) = workspace(&files);

    let moved = workspace.rename_folder("Spare", "Reserve").unwrap();
    assert!(moved.hosts.is_empty());
    assert_eq!(moved.rewritten, ["_jobs.yaml"]);
    let moved = workspace.rename_host(&id("Lab/mid.yaml"), "hop").unwrap();
    assert_eq!(moved.rewritten, ["Lab/far.yaml", "_jobs.yaml"]);

    assert_eq!(
        read(&root, "_jobs.yaml"),
        jobs.replace("folder: Spare", "folder: Reserve")
            .replace("host: Lab\\mid.yaml", "host: Lab/hop.yaml")
    );
    assert_eq!(plan_saved(&workspace, "Mid"), ["Lab/hop.yaml"]);
}

#[test]
fn a_reference_on_no_line_of_its_own_is_rewritten_with_the_whole_file() {
    let mut files = lab_files();
    files.push((
        "Lab/flow.yaml",
        "{address: 10.2.0.3, jump_host: bastion.yaml}\n".into(),
    ));
    let (_temp, root, mut workspace) = workspace(&files);

    let moved = workspace.rename_host(&id("bastion.yaml"), "gw").unwrap();

    assert!(moved.rewritten.contains(&"Lab/flow.yaml".to_string()));
    assert_eq!(
        read(&root, "Lab/flow.yaml"),
        "address: 10.2.0.3\njump_host: gw.yaml\n"
    );
    assert_eq!(chain(&workspace, "Lab/flow.yaml"), ["gw.yaml"]);
}

#[test]
fn nothing_is_rewritten_when_nothing_moves() {
    let (_temp, root, mut workspace) = workspace(&lab_files());

    let moved = workspace
        .rename_host(&id("bastion.yaml"), "bastion")
        .unwrap();
    assert!(moved.hosts.is_empty() && moved.rewritten.is_empty());
    let moved = workspace.move_folder("Sites", "").unwrap();
    assert!(moved.hosts.is_empty() && moved.rewritten.is_empty());
    untouched_but(&root, &[], str::to_string);
}

// --- History ---

/// Session logs and config history, in temp folders.
struct Kept {
    _temp: TempDir,
    logs: LogFolder,
    configs: ConfigStore,
}

fn kept() -> Kept {
    let temp = TempDir::new().unwrap();
    Kept {
        logs: LogFolder::new(temp.path().join("logs")),
        configs: ConfigStore::new(temp.path().join("configs")),
        _temp: temp,
    }
}

impl Kept {
    /// Gives `host` a session log (written as a session would) and a config
    /// version; returns the version.
    fn give_history(&self, workspace: &Workspace, host: &str, config: &str) -> String {
        let folder = self.logs.create_host_folder(workspace, &id(host)).unwrap();
        fs::write(
            folder.join("2026-11-18_10-00-00.log"),
            format!("{host} log\n"),
        )
        .unwrap();
        let saved = self
            .configs
            .save(workspace.id().unwrap(), &id(host), config, None)
            .unwrap();
        saved.version().to_string()
    }

    fn log_texts(&self, workspace: &Workspace, host: &str) -> Vec<String> {
        let Some(folder) = self.logs.host_folder(workspace, &id(host)) else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(folder) else {
            return Vec::new();
        };
        let mut texts: Vec<String> = entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .map(|path| fs::read_to_string(path).unwrap())
            .collect();
        texts.sort();
        texts
    }

    fn versions(&self, workspace: &Workspace, host: &str) -> Vec<String> {
        self.configs
            .versions(workspace.id().unwrap(), &id(host))
            .unwrap()
            .into_iter()
            .map(|version| version.id)
            .collect()
    }

    fn carry(&self, workspace: &Workspace, moves: &[HostMove]) -> Vec<LeftHistory> {
        history::carry(workspace, moves, &self.logs, &self.configs, None).left
    }
}

#[test]
fn a_renamed_host_keeps_its_logs_and_its_config_history_carries_on() {
    let (_temp, _root, mut workspace) = workspace(&lab_files());
    let kept = kept();
    let version = kept.give_history(&workspace, "Sites/Madrid/r1.yaml", "hostname r1\n");
    let old_logs = kept
        .logs
        .host_folder(&workspace, &id("Sites/Madrid/r1.yaml"))
        .unwrap();
    let old_configs = kept
        .configs
        .host_folder("lab-1", &id("Sites/Madrid/r1.yaml"))
        .unwrap();

    let moved = workspace
        .rename_host(&id("Sites/Madrid/r1.yaml"), "core-1")
        .unwrap();
    let left = kept.carry(&workspace, &moved.hosts);

    assert!(left.is_empty(), "{left:?}");
    let new = "Sites/Madrid/core-1.yaml";
    assert_eq!(
        kept.log_texts(&workspace, new),
        ["Sites/Madrid/r1.yaml log\n"]
    );
    assert_eq!(
        kept.versions(&workspace, new),
        std::slice::from_ref(&version)
    );
    // The old places are gone, the folders above them too when emptied.
    assert!(!old_logs.exists());
    assert!(!old_configs.exists());
    assert!(old_logs.parent().unwrap().exists(), "Madrid holds r2's");
    // A backup (on login or not) compares with the versions kept.
    assert_eq!(
        kept.configs
            .save("lab-1", &id(new), "hostname r1\n", None)
            .unwrap(),
        SaveOutcome::Unchanged { version }
    );
}

#[test]
fn a_moved_host_keeps_its_logs_and_config_history() {
    let (_temp, _root, mut workspace) = workspace(&lab_files());
    let kept = kept();
    let version = kept.give_history(&workspace, "Sites/Madrid/r2.yaml", "hostname r2\n");

    let moved = workspace
        .move_host(&id("Sites/Madrid/r2.yaml"), "Lab")
        .unwrap();
    let left = kept.carry(&workspace, &moved.hosts);

    assert!(left.is_empty(), "{left:?}");
    assert_eq!(
        kept.log_texts(&workspace, "Lab/r2.yaml"),
        ["Sites/Madrid/r2.yaml log\n"]
    );
    assert_eq!(kept.versions(&workspace, "Lab/r2.yaml"), [version]);
    // Nothing is left under the old tree path, folders included.
    let old = kept.logs.root().join("workspaces/lab/Sites");
    assert!(!old.exists(), "{}", old.display());
}

#[test]
fn every_host_under_a_renamed_or_moved_folder_keeps_its_history() {
    let (_temp, _root, mut workspace) = workspace(&lab_files());
    let kept = kept();
    let v1 = kept.give_history(&workspace, "Sites/Madrid/r1.yaml", "hostname r1\n");
    let v2 = kept.give_history(&workspace, "Sites/Madrid/r2.yaml", "hostname r2\n");

    let moved = workspace.rename_folder("Sites", "Europe").unwrap();
    assert!(kept.carry(&workspace, &moved.hosts).is_empty());
    let moved = workspace.move_folder("Europe/Madrid", "Lab").unwrap();
    assert!(kept.carry(&workspace, &moved.hosts).is_empty());

    for (host, version) in [("Lab/Madrid/r1.yaml", &v1), ("Lab/Madrid/r2.yaml", &v2)] {
        let was = host.replace("Lab/", "Sites/");
        assert_eq!(kept.log_texts(&workspace, host), [format!("{was} log\n")]);
        assert_eq!(
            kept.versions(&workspace, host),
            std::slice::from_ref(version)
        );
    }
}

#[test]
fn history_is_never_mixed_with_an_earlier_hosts_of_the_new_name() {
    let (_temp, _root, mut workspace) = workspace(&lab_files());
    let kept = kept();
    kept.give_history(&workspace, "Sites/Madrid/r1.yaml", "hostname r1\n");
    // An earlier host called r9 left its history behind.
    let r9 = kept.logs.root().join("workspaces/lab/Sites/Madrid/r9");
    fs::create_dir_all(&r9).unwrap();
    fs::write(r9.join("2026-01-01_00-00-00.log"), "the old r9\n").unwrap();
    let r9_configs = kept
        .configs
        .host_folder("lab-1", &id("Sites/Madrid/r9.yaml"))
        .unwrap();
    fs::create_dir_all(&r9_configs).unwrap();
    fs::write(r9_configs.join("2026-01-01_00-00-00.txt"), "hostname r9\n").unwrap();
    let old_logs = kept
        .logs
        .host_folder(&workspace, &id("Sites/Madrid/r1.yaml"))
        .unwrap();
    let old_configs = kept
        .configs
        .host_folder("lab-1", &id("Sites/Madrid/r1.yaml"))
        .unwrap();

    let moved = workspace
        .rename_host(&id("Sites/Madrid/r1.yaml"), "r9")
        .unwrap();
    let left = kept.carry(&workspace, &moved.hosts);

    // The rename is done; the history stays where it was, and says so.
    assert_eq!(moved.to, id("Sites/Madrid/r9.yaml"));
    let kinds: Vec<_> = left
        .iter()
        .map(|left| (left.kind, left.folder.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            (HistoryKind::SessionLogs, old_logs.clone()),
            (HistoryKind::ConfigHistory, old_configs.clone()),
        ]
    );
    for left in &left {
        assert_eq!(left.host, id("Sites/Madrid/r9.yaml"));
        assert_eq!(left.from, id("Sites/Madrid/r1.yaml"));
        assert!(left.reason.contains("earlier host"), "{}", left.reason);
    }
    assert_eq!(
        fs::read_to_string(old_logs.join("2026-11-18_10-00-00.log")).unwrap(),
        "Sites/Madrid/r1.yaml log\n"
    );
    assert_eq!(
        kept.log_texts(&workspace, "Sites/Madrid/r9.yaml"),
        ["the old r9\n"]
    );
    assert_eq!(
        kept.versions(&workspace, "Sites/Madrid/r9.yaml"),
        ["2026-01-01_00-00-00"]
    );
}

#[test]
fn a_host_without_history_moves_with_nothing_to_report() {
    let (_temp, _root, mut workspace) = workspace(&lab_files());
    let kept = kept();
    let moved = workspace.rename_host(&id("other.yaml"), "spare").unwrap();
    assert!(kept.carry(&workspace, &moved.hosts).is_empty());
    assert!(!kept.logs.root().exists());
}

#[test]
fn session_records_name_the_host_after_it_moved() {
    let (_temp, root, mut workspace) = workspace(&lab_files());
    let records = SessionRecords::new();
    let session = netterm_core::session::SessionId(7);
    let record = SessionRecord::for_host(
        &workspace,
        &id("Lab/far.yaml"),
        Transport::Ssh,
        "far",
        "admin@10.2.0.2",
    )
    .unwrap();
    records.record(session, record);
    records.record(
        netterm_core::session::SessionId(8),
        SessionRecord::new("10.9.9.9", "10.9.9.9"),
    );

    let moved = workspace.rename_folder("Lab", "Hops").unwrap();
    assert_eq!(records.move_hosts(&root, &moved.hosts), [session]);
    // Another workspace's hosts are not touched.
    assert!(records
        .move_hosts(Path::new("elsewhere"), &moved.hosts)
        .is_empty());

    assert_eq!(
        records.get(session).unwrap().host_in(&workspace),
        Some(&id("Hops/far.yaml"))
    );
}

// --- An open session ---

async fn connected(events: &mut SessionEvents) {
    assert_eq!(next_state(events).await, SessionState::Connecting);
    assert_eq!(next_state(events).await, SessionState::Connected);
    output_ending_with(events, PROMPT).await;
}

async fn closed(events: &mut SessionEvents) {
    assert!(matches!(next_state(events).await, SessionState::Closed(_)));
}

/// A session open on the host while it is renamed goes on logging into the
/// same file, moved to the new place; its next connection logs there too.
#[tokio::test]
async fn a_session_open_during_the_rename_carries_on_logging_in_the_new_place() {
    let server = TestSshServer::start().await;
    let (_temp, _root, mut workspace) = workspace(&[(
        "Sites/r1.yaml",
        format!("address: 127.0.0.1\nport: {}\n", server.port()),
    )]);
    let kept = kept();
    let manager = support::manager();
    let old = id("Sites/r1.yaml");
    let target = workspace
        .resolve_host(&old)
        .unwrap()
        .ssh_target(USERNAME.into(), PASSWORD.into());
    let (session, mut events) = manager.open_logged(
        SessionTarget::Ssh(target),
        SIZE,
        kept.logs.host_log(&workspace, &old),
    );
    connected(&mut events).await;
    manager.write(session, b"lines 1\r".to_vec()).unwrap();
    output_ending_with(&mut events, PROMPT).await;
    let old_folder = kept.logs.host_folder(&workspace, &old).unwrap();
    let workspace_before = workspace.clone();

    let moved = workspace.rename_host(&old, "edge-1").unwrap();
    let records = SessionRecords::new();
    let record =
        SessionRecord::for_host(&workspace_before, &old, Transport::Ssh, "r1", "r1").unwrap();
    records.record(session, record);
    let carried = history::carry(
        &workspace,
        &moved.hosts,
        &kept.logs,
        &kept.configs,
        Some(Sessions {
            manager: &manager,
            records: &records,
        }),
    );
    assert!(carried.left.is_empty(), "{:?}", carried.left);
    // Its record names the host's new id.
    assert_eq!(carried.sessions, [session]);
    assert_eq!(
        records.get(session).unwrap().host_in(&workspace),
        Some(&id("Sites/edge-1.yaml"))
    );

    manager.write(session, b"lines 2\r".to_vec()).unwrap();
    output_ending_with(&mut events, PROMPT).await;
    manager.close(session).unwrap();
    closed(&mut events).await;

    let new = "Sites/edge-1.yaml";
    assert!(!old_folder.exists());
    assert_eq!(
        kept.log_texts(&workspace, new),
        ["test-ssh$ lines 1\nline 1\ntest-ssh$ lines 2\nline 1\nline 2\ntest-ssh$\n"]
    );

    // Its next connection logs in the new place too.
    let mut events = manager.reconnect(session, SIZE).unwrap();
    connected(&mut events).await;
    manager.close(session).unwrap();
    closed(&mut events).await;
    assert_eq!(kept.log_texts(&workspace, new).len(), 2);
    assert!(!old_folder.exists());
}
