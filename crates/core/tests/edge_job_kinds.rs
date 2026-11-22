//! Edge testing of the other kinds of job (#119): the config backup job
//! (two at once on one device, cancelled or cut by the app closing while
//! the configuration is shown, a configuration larger than a job keeps),
//! the file job (a full disk, a refused folder, the local file changed or
//! gone, the app closing while it sends) and a kept run run again after
//! its folder was removed.

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use enodix_core::configs::{ConfigStore, SaveOutcome};
use enodix_core::credentials::Profiles;
use enodix_core::job::results::RunStore;
use enodix_core::job::{
    self, HostState, JobEvent, JobOptions, JobStep, PlanError, RunningJobs, APP_CLOSED,
    APP_CLOSE_WAIT,
};
use enodix_core::workspace::HostId;
use support::jobs::{folder, follow, host, run, Lab};
use support::{TestSshServer, WAIT};

fn backup_options(configs: &ConfigStore) -> JobOptions {
    JobOptions {
        configs: Some(configs.clone()),
        ..JobOptions::default()
    }
}

/// Two config backups at once on the same device: one version is kept, the
/// other run finds it unchanged; neither writes over the other.
#[tokio::test]
async fn two_backups_at_once_of_one_device_keep_one_version() {
    let device = TestSshServer::start_device("r1").await;
    let lab = Lab::new(&[("r1.yaml", host(&device, "eos", ""))]);
    let configs = ConfigStore::new(lab.folder("data/configs"));
    let manager = support::manager();
    let first = job::start(
        &manager,
        lab.plan(&folder(""), &JobStep::ConfigBackup),
        backup_options(&configs),
    )
    .unwrap();
    let second = job::start(
        &manager,
        lab.plan(&folder(""), &JobStep::ConfigBackup),
        backup_options(&configs),
    )
    .unwrap();
    let (first, second) = tokio::join!(
        follow(first, WAIT * 3, |_, _| {}),
        follow(second, WAIT * 3, |_, _| {})
    );
    let mut outcomes: Vec<SaveOutcome> = [&first, &second]
        .iter()
        .map(|ran| {
            let result = ran.result("r1.yaml");
            assert_eq!(result.state, HostState::Done, "{}", result.output);
            result.backup.clone().expect("a backup")
        })
        .collect();
    outcomes.sort_by_key(|outcome| matches!(outcome, SaveOutcome::Unchanged { .. }));
    let versions = configs
        .versions(&lab.id(), &HostId::new("r1.yaml"))
        .unwrap();
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert_eq!(
        outcomes,
        [
            SaveOutcome::Saved {
                version: versions[0].id.clone()
            },
            SaveOutcome::Unchanged {
                version: versions[0].id.clone()
            }
        ]
    );
}

/// Whatever remains of a host's configuration history folder: its files.
fn history(configs: &ConfigStore, ws: &str) -> Vec<String> {
    let mut found = Vec::new();
    let root = configs.root().join(ws);
    if root.is_dir() {
        for entry in walk(&root) {
            found.push(entry);
        }
    }
    found
}

fn walk(folder: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path.display().to_string());
        }
    }
    files
}

/// A backup cancelled while the device shows its configuration, and one
/// cut by the app closing at that moment, keep nothing: no version, no
/// half-written file. The app's close says why in the kept run.
#[tokio::test]
async fn a_backup_cut_while_the_configuration_is_shown_keeps_nothing() {
    for closing in [false, true] {
        let device = TestSshServer::start_device("r1").await;
        for n in 0..40 {
            device.configure(&format!("interface Ethernet{n}"));
        }
        device.set_config_pace(Duration::from_millis(50));
        let lab = Lab::new(&[("r1.yaml", host(&device, "eos", ""))]);
        let ws = lab.id();
        let configs = ConfigStore::new(lab.folder("data/configs"));
        let runs = RunStore::new(lab.folder("data/jobs"));
        let plan = lab.plan(&folder(""), &JobStep::ConfigBackup);
        let recorder = runs.begin(&ws, &folder(""), &plan).unwrap();
        let run_id = recorder.id().to_string();
        let jobs = RunningJobs::new();
        let manager = support::manager();
        let job = job::start(&manager, plan, backup_options(&configs)).unwrap();
        let mut followed = jobs.follow(job.canceller(), recorder);
        let id = followed.id();
        let mut cut = false;
        let ending = jobs.clone();
        let ran = follow(job, WAIT * 3, |_, event| {
            followed.record(event).unwrap();
            if let JobEvent::Output { text, .. } = event {
                if text.contains("interface Ethernet3\n") && !cut {
                    cut = true;
                    if closing {
                        let ending = ending.clone();
                        std::thread::spawn(move || ending.end_all(APP_CLOSED, APP_CLOSE_WAIT));
                    } else {
                        assert!(ending.cancel(id));
                    }
                }
            }
        })
        .await;
        drop(followed);
        assert!(cut, "the configuration was being shown");
        assert_eq!(*ran.state("r1.yaml"), HostState::Cancelled);
        assert_eq!(ran.result("r1.yaml").backup, None);
        assert_eq!(history(&configs, &ws), Vec::<String>::new());
        let kept = runs.run(&ws, &run_id).unwrap();
        assert!(kept.ended());
        assert_eq!(
            kept.cancel_reason.as_deref(),
            closing.then_some(APP_CLOSED),
            "closing: {closing}"
        );
    }
}

/// A configuration larger than a job keeps of a host's output is never kept
/// cut short: the backup fails saying so and no version is written.
#[tokio::test]
async fn a_configuration_larger_than_a_job_keeps_is_not_kept_cut_short() {
    let device = TestSshServer::start_device("r1").await;
    for n in 0..400 {
        device.configure(&format!("interface Ethernet{n}\n description uplink {n}"));
    }
    let lab = Lab::new(&[("r1.yaml", host(&device, "eos", ""))]);
    let configs = ConfigStore::new(lab.folder("data/configs"));
    let plan = lab.plan(&folder(""), &JobStep::ConfigBackup);
    let options = JobOptions {
        output_limit: 4 * 1024,
        ..backup_options(&configs)
    };
    let ran = run(&support::manager(), plan, options, WAIT * 3).await;
    assert_eq!(
        ran.failure("r1.yaml"),
        "\"show running-config\" showed more than the 4 KiB a job keeps of a host's output; nothing was backed up"
    );
    let output = &ran.result("r1.yaml").output;
    assert!(output.len() < 4 * 1024 + 200, "{}", output.len());
    assert!(output.ends_with(&job::output_cut(4 * 1024)), "{output}");
    assert_eq!(history(&configs, &lab.id()), Vec::<String>::new());
}

// --- The file job ---

/// A workspace whose platform `imaging` puts files in `/images`.
const PLATFORMS: &str = "imaging:\n  prompt: '#'\n  file_folder: /images\n";

/// A server serving a new folder (with `images` and a locked folder) over
/// SFTP, and that folder.
async fn sftp_server() -> (TestSshServer, tempfile::TempDir) {
    let server = TestSshServer::start().await;
    let folder = tempfile::TempDir::new().unwrap();
    fs::create_dir(folder.path().join("images")).unwrap();
    fs::create_dir(folder.path().join(support::sftp::SFTP_LOCKED)).unwrap();
    server.serve_sftp(folder.path());
    (server, folder)
}

fn send(file: &Path, destination: &str) -> JobStep {
    JobStep::SendFile {
        file: file.to_string_lossy().into_owned(),
        destination: destination.into(),
        overwrite: true,
    }
}

fn bytes(size: usize) -> Vec<u8> {
    (0..size).map(|n| (n * 7 % 251) as u8).collect()
}

/// A device whose disk fills up part way, and a folder the login may not
/// write to: each host fails with its own reason, and the others are
/// sent.
#[tokio::test]
async fn a_full_disk_and_a_refused_folder_fail_only_their_hosts() {
    let (full, full_folder) = sftp_server().await;
    full.sftp_quirks().full_after(64 * 1024);
    let (locked, _locked_folder) = sftp_server().await;
    let (fine, fine_folder) = sftp_server().await;
    let lab = Lab::new(&[
        ("_platforms.yaml", PLATFORMS.into()),
        ("full.yaml", host(&full, "imaging", "")),
        ("fine.yaml", host(&fine, "imaging", "")),
    ]);
    let local = lab.folder("local").join("image.bin");
    fs::write(&local, bytes(300 * 1024)).unwrap();

    let plan = lab.plan(&folder(""), &send(&local, ""));
    let ran = run(&support::manager(), plan, JobOptions::default(), WAIT * 3).await;
    let reason = ran.failure("full.yaml");
    assert!(!reason.is_empty(), "{reason}");
    assert_eq!(*ran.state("fine.yaml"), HostState::Done);
    assert_eq!(
        fs::read(fine_folder.path().join("images/image.bin")).unwrap(),
        bytes(300 * 1024)
    );
    // What the full disk left is said, and nothing whole is claimed.
    let left = full_folder.path().join("images/image.bin");
    let output = &ran.result("full.yaml").output;
    assert!(
        !left.exists() || output.contains("partial") || reason.contains("partial"),
        "{reason}\n{output}"
    );
    println!("full disk: {reason}");

    let lab = Lab::new(&[
        ("_platforms.yaml", PLATFORMS.into()),
        ("locked.yaml", host(&locked, "imaging", "")),
    ]);
    let plan = lab.plan(
        &folder(""),
        &send(&local, &format!("/{}", support::sftp::SFTP_LOCKED)),
    );
    let ran = run(&support::manager(), plan, JobOptions::default(), WAIT * 3).await;
    let reason = ran.failure("locked.yaml");
    assert!(reason.to_lowercase().contains("permission"), "{reason}");
}

/// The local file gone before the run starts: every host fails naming it,
/// nothing is left on a device, and the run has no hashes.
#[tokio::test]
async fn a_local_file_gone_before_the_run_fails_every_host() {
    let (server, served) = sftp_server().await;
    let lab = Lab::new(&[
        ("_platforms.yaml", PLATFORMS.into()),
        ("a.yaml", host(&server, "imaging", "")),
        ("b.yaml", host(&server, "imaging", "")),
    ]);
    let local = lab.folder("local").join("image.bin");
    fs::write(&local, bytes(10 * 1024)).unwrap();
    let plan = lab.plan(&folder(""), &send(&local, ""));
    fs::remove_file(&local).unwrap();
    let ran = run(&support::manager(), plan, JobOptions::default(), WAIT * 3).await;
    for id in ["a.yaml", "b.yaml"] {
        let reason = ran.failure(id);
        assert!(reason.contains("image.bin"), "{id}: {reason}");
    }
    assert_eq!(
        ran.report
            .file
            .as_ref()
            .and_then(|file| file.hashes.clone()),
        None
    );
    assert!(!served.path().join("images/image.bin").exists());
}

/// Bytes of `size` unlike [`bytes`]'s.
fn other_bytes(size: usize) -> Vec<u8> {
    (0..size).map(|n| (n * 13 % 241) as u8 + 1).collect()
}

/// The MD5 and SHA-256 of `bytes`, as the run gives them.
fn hashes_of(bytes: &[u8]) -> (String, String) {
    use sha2::Digest;
    let hex = |digest: &[u8]| {
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    (
        hex(&md5::compute(bytes).0),
        hex(&sha2::Sha256::digest(bytes)),
    )
}

/// The local file changed after pre-flight, between two hosts (to another
/// size, and to other bytes of the same size): the first host has the
/// file pre-flight showed, the second fails saying the file changed and is
/// sent nothing; the run's size and hashes are those of the planned file
/// (#137; it used to send the second host the new file).
#[tokio::test]
async fn a_local_file_changed_during_the_run_is_not_sent_as_the_planned_one() {
    for changed_size in [150 * 1024, 100 * 1024] {
        let (a, a_folder) = sftp_server().await;
        let (b, b_folder) = sftp_server().await;
        let lab = Lab::new(&[
            ("_platforms.yaml", PLATFORMS.into()),
            ("a.yaml", host(&a, "imaging", "")),
            ("b.yaml", host(&b, "imaging", "")),
        ]);
        let local = lab.folder("local").join("image.bin");
        let planned = bytes(100 * 1024);
        fs::write(&local, &planned).unwrap();
        let plan = lab.plan(&folder(""), &send(&local, ""));
        let job = job::start(
            &support::manager(),
            plan,
            JobOptions {
                parallel: 1,
                ..JobOptions::default()
            },
        )
        .unwrap();
        let changing = local.clone();
        let ran = follow(job, WAIT * 3, move |_, event| {
            if let JobEvent::State { host, state } = event {
                if host.as_str() == "a.yaml" && *state == HostState::Done {
                    fs::write(&changing, other_bytes(changed_size)).unwrap();
                }
            }
        })
        .await;
        assert_eq!(*ran.state("a.yaml"), HostState::Done);
        assert_eq!(
            fs::read(a_folder.path().join("images/image.bin")).unwrap(),
            planned
        );
        let reason = ran.failure("b.yaml");
        assert!(
            reason.starts_with("the file changed since pre-flight (")
                && reason.ends_with(
                    "): nothing was sent; plan the job again to send the file as it is now"
                ),
            "{changed_size}: {reason}"
        );
        if changed_size != planned.len() {
            assert!(
                reason.contains("it was 102400 bytes at pre-flight and is 153600 bytes now"),
                "{reason}"
            );
        }
        assert!(!b_folder.path().join("images/image.bin").exists());
        let file = ran.report.file.as_ref().unwrap();
        assert_eq!(file.size, planned.len() as u64);
        let hashes = file.hashes.as_ref().expect("the planned file's hashes");
        assert_eq!(
            (hashes.md5.clone(), hashes.sha256.clone()),
            hashes_of(&planned)
        );
    }
}

/// The local file changed while a host is sent it (the same size, other
/// bytes, written while the device held all but the first piece): the host fails
/// saying the file changed while it was sent, what was sent is left on the
/// device and nothing claims it is the planned file (#137).
#[tokio::test]
async fn a_local_file_changed_while_it_is_sent_fails_the_host() {
    let (server, served) = sftp_server().await;
    // Larger than the SSH window, so most of it is read after the change.
    let size = 8 * 1024 * 1024;
    server.sftp_quirks().hold_from(256 * 1024);
    let lab = Lab::new(&[
        ("_platforms.yaml", PLATFORMS.into()),
        ("r1.yaml", host(&server, "imaging", "")),
    ]);
    let local = lab.folder("local").join("image.bin");
    let planned = bytes(size);
    fs::write(&local, &planned).unwrap();
    let plan = lab.plan(&folder(""), &send(&local, ""));
    let job = job::start(&support::manager(), plan, JobOptions::default()).unwrap();
    let changing = local.clone();
    let mut changed = false;
    let ran = follow(job, WAIT * 3, |_, event| {
        if let JobEvent::Progress { done, .. } = event {
            if *done > 0 && !changed {
                changed = true;
                fs::write(&changing, other_bytes(size)).unwrap();
                server.sftp_quirks().release();
            }
        }
    })
    .await;
    assert!(changed, "the file was being sent");
    let reason = ran.failure("r1.yaml");
    assert!(reason.starts_with("the file changed "), "{reason}");
    assert!(reason.ends_with("left on the device"), "{reason}");
    let sent = fs::read(served.path().join("images/image.bin")).unwrap();
    assert_ne!(sent, planned);
    assert_eq!(ran.result("r1.yaml").bytes, None);
    let hashes = ran.report.file.as_ref().unwrap().hashes.clone();
    if let Some(hashes) = hashes {
        // Read before the change: the planned file's.
        assert_eq!((hashes.md5, hashes.sha256), hashes_of(&planned));
    }
    println!("{reason}");
}

/// The app closing while a file is being sent: the host ends cancelled
/// with the reason, within the time the app waits; its partial file is
/// removed from the device, and the kept run says the app was closed. A
/// device that has stopped answering is given up on at the transfer's
/// stall limit (three connect timeouts: here 1 s each), its partial file
/// not said to be removed.
#[tokio::test]
async fn closing_the_app_while_a_file_is_sent_ends_it_and_says_what_was_left() {
    for answering in [true, false] {
        let (server, served) = sftp_server().await;
        server.sftp_quirks().hold_from(32 * 1024);
        let lab = Lab::new(&[
            ("_platforms.yaml", PLATFORMS.into()),
            ("r1.yaml", host(&server, "imaging", "connect_timeout: 1\n")),
        ]);
        let ws = lab.id();
        let local = lab.folder("local").join("image.bin");
        fs::write(&local, bytes(256 * 1024)).unwrap();
        let plan = lab.plan(&folder(""), &send(&local, ""));
        let runs = RunStore::new(lab.folder("data/jobs"));
        let recorder = runs.begin(&ws, &folder(""), &plan).unwrap();
        let run_id = recorder.id().to_string();
        let jobs = RunningJobs::new();
        let job = job::start(&support::manager(), plan, JobOptions::default()).unwrap();
        let mut followed = jobs.follow(job.canceller(), recorder);
        let ending = jobs.clone();
        let mut closing = None;
        let ran = follow(job, WAIT * 3, |_, event| {
            followed.record(event).unwrap();
            if let JobEvent::Progress { done, .. } = event {
                if *done > 0 && closing.is_none() {
                    let ending = ending.clone();
                    closing = Some(std::thread::spawn(move || {
                        ending.end_all(APP_CLOSED, APP_CLOSE_WAIT)
                    }));
                    if answering {
                        server.sftp_quirks().release();
                    }
                }
            }
        })
        .await;
        drop(followed);
        let left = closing.expect("the file was being sent").join().unwrap();
        assert_eq!(
            left, 0,
            "answering {answering}: the job ended within the wait"
        );
        assert_eq!(*ran.state("r1.yaml"), HostState::Cancelled);
        let output = ran.result("r1.yaml").output.clone();
        let partial = served.path().join("images/image.bin");
        if answering {
            assert!(
                output.contains("the partial file was removed from the device"),
                "{output}"
            );
            assert!(!partial.exists(), "the partial file stayed on the device");
        } else {
            assert!(output.contains("Cancelled:"), "{output}");
            assert!(!output.contains("was removed"), "{output}");
        }
        server.sftp_quirks().release();
        let kept = runs.run(&ws, &run_id).unwrap();
        assert_eq!(kept.cancel_reason.as_deref(), Some(APP_CLOSED));
        assert_eq!(kept.hosts[0].state, HostState::Cancelled);
        println!("answering {answering}: {output}");
    }
}

/// A file job sending to a device that stopped answering, with the
/// default connect timeout (15 s, so a stall limit of 45 s): the user's
/// Cancel, and the app closing, end it within the time the app waits for
/// its jobs, the host saying the partial file may remain on the device
/// (#137; both waited for the stall limit, as the remove went
/// unanswered).
#[tokio::test]
async fn cancelling_a_file_sent_to_a_device_that_stopped_answering_ends_it_at_once() {
    for closing in [false, true] {
        let (server, served) = sftp_server().await;
        server.sftp_quirks().hold_from(32 * 1024);
        let lab = Lab::new(&[
            ("_platforms.yaml", PLATFORMS.into()),
            ("r1.yaml", host(&server, "imaging", "")),
        ]);
        let ws = lab.id();
        let local = lab.folder("local").join("image.bin");
        fs::write(&local, bytes(256 * 1024)).unwrap();
        let plan = lab.plan(&folder(""), &send(&local, ""));
        let runs = RunStore::new(lab.folder("data/jobs"));
        let recorder = runs.begin(&ws, &folder(""), &plan).unwrap();
        let run_id = recorder.id().to_string();
        let jobs = RunningJobs::new();
        let job = job::start(&support::manager(), plan, JobOptions::default()).unwrap();
        let mut followed = jobs.follow(job.canceller(), recorder);
        let id = followed.id();
        let ending = jobs.clone();
        let mut cancelled_at = None;
        let mut closing_thread = None;
        let ran = follow(job, WAIT * 2, |_, event| {
            followed.record(event).unwrap();
            if let JobEvent::Progress { done, .. } = event {
                if *done > 0 && cancelled_at.is_none() {
                    cancelled_at = Some(std::time::Instant::now());
                    if closing {
                        let ending = ending.clone();
                        closing_thread = Some(std::thread::spawn(move || {
                            ending.end_all(APP_CLOSED, APP_CLOSE_WAIT)
                        }));
                    } else {
                        assert!(ending.cancel(id));
                    }
                }
            }
        })
        .await;
        let took = cancelled_at.expect("the file was being sent").elapsed();
        drop(followed);
        assert!(took < APP_CLOSE_WAIT, "closing {closing}: {took:?}");
        if let Some(thread) = closing_thread {
            assert_eq!(thread.join().unwrap(), 0, "the job ended within the wait");
        }
        assert_eq!(*ran.state("r1.yaml"), HostState::Cancelled);
        let output = ran.result("r1.yaml").output.clone();
        assert!(
            output.contains(&format!(
                "Cancelled: a partial file may remain on the device: it did not answer the request to remove it within {} s.",
                enodix_core::files::CANCEL_REMOVE_WAIT.as_secs()
            )),
            "{output}"
        );
        server.sftp_quirks().release();
        let kept = runs.run(&ws, &run_id).unwrap();
        assert!(kept.ended());
        assert_eq!(kept.cancel_reason.as_deref(), closing.then_some(APP_CLOSED));
        let _ = served;
        println!("closing {closing}: ended {took:?} after the cancel");
    }
}

/// A run whose folder was removed since: running it again is refused,
/// naming the folder, rather than run on nothing; a run already started
/// when the folder goes goes on with the hosts it planned.
#[tokio::test]
async fn a_run_again_after_its_folder_was_removed_is_refused() {
    let device = TestSshServer::start_device("r1").await;
    let mut lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        ("Site/r1.yaml", host(&device, "frr", "")),
    ]);
    let ws = lab.id();
    let runs = RunStore::new(lab.folder("data/jobs"));
    let plan = lab.plan(&folder("Site"), &support::jobs::snippet("Show"));
    let mut recorder = runs.begin(&ws, &folder("Site"), &plan).unwrap();
    let id = recorder.id().to_string();
    let job = job::start(&support::manager(), plan, JobOptions::default()).unwrap();
    fs::remove_dir_all(lab.root().join("Site")).unwrap();
    lab.reload();
    let ran = follow(job, WAIT * 3, |_, event| recorder.record(event).unwrap()).await;
    assert_eq!(*ran.state("Site/r1.yaml"), HostState::Done);
    let kept = runs.run(&ws, &id).unwrap();
    let again = job::plan_again(
        &lab.workspace,
        &Profiles::new(&lab.store, &lab.workspace),
        &kept,
    );
    assert!(
        matches!(again, Err(PlanError::NoSuchFolder(ref name)) if name == "Site"),
        "{again:?}"
    );
}
