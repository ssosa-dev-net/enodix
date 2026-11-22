//! The file job (#107): one file sent to many hosts through the transfer
//! engine, each host over its own unattended file connection to an
//! in-process SSH server serving a folder over SFTP (or SCP only).
//! Pre-flight, the destination (the platform's file folder, or a typed
//! path with host variables), "already there", a device that cannot tell,
//! refused questions, the parallel limit, cancel, and the kept run.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::job::results::RunStore;
use enodix_core::job::{
    self, HostState, Job, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget, PlanError,
    SkipReason,
};
use enodix_core::known_hosts::KnownHosts;
use enodix_core::session::SessionManager;
use enodix_core::workspace::{HostId, Protocol, SavedJob, Workspace};
use sha2::Digest;
use support::scp::ScpQuirk;
use support::{TestSshServer, CODE_QUESTION, OTP_USERNAME, PASSWORD, USERNAME, WAIT};
use tempfile::TempDir;

/// A platform whose file folder is `/images`, and one with none.
const PLATFORMS: &str = "imaging:\n  prompt: '#'\n  file_folder: /images\nbare:\n  prompt: '#'\n";

/// A workspace with an id, made of `files` and [`PLATFORMS`].
fn workspace(temp: &TempDir, files: &[(&str, String)]) -> Workspace {
    let root = temp.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("_platforms.yaml"), PLATFORMS).unwrap();
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let mut workspace = Workspace::open(&root).unwrap();
    workspace.ensure_id().unwrap();
    assert!(
        workspace.problems().is_empty(),
        "{:?}",
        workspace.problems()
    );
    workspace
}

/// A host file for `server` with the profile `lab`, on `platform`.
fn host(server: &TestSshServer, platform: &str, extra: &str) -> String {
    format!(
        "address: 127.0.0.1\nport: {}\nplatform: {platform}\ncredential_profile: lab\n{extra}",
        server.port()
    )
}

fn profile(name: &str, username: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: username.into(),
        auth: ProfileAuth::Password {
            password: PASSWORD.into(),
        },
        enable_secret: None,
    }
}

fn store(workspace: &Workspace) -> MemoryStore {
    let store = MemoryStore::new();
    for profile in [profile("lab", USERNAME), profile("otp", OTP_USERNAME)] {
        store.put(&workspace.credential_scope(), &profile).unwrap();
    }
    store
}

/// A server serving a new folder (with an `images` folder) over SFTP.
async fn sftp_server() -> (TestSshServer, TempDir) {
    let server = TestSshServer::start().await;
    let folder = TempDir::new().unwrap();
    fs::create_dir(folder.path().join("images")).unwrap();
    server.serve_sftp(folder.path());
    (server, folder)
}

/// Bytes that do not repeat in short runs.
fn content(size: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_u32;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

/// A local file `name` holding `bytes`.
fn local_file(folder: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = folder.path().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn send(file: &Path, destination: &str, overwrite: bool) -> JobStep {
    JobStep::SendFile {
        file: file.to_string_lossy().into_owned(),
        destination: destination.into(),
        overwrite,
    }
}

fn folder(path: &str) -> JobTarget {
    JobTarget::Folder { path: path.into() }
}

fn plan(workspace: &Workspace, store: &MemoryStore, target: &JobTarget, step: &JobStep) -> JobPlan {
    job::plan(workspace, &Profiles::new(store, workspace), target, step).unwrap()
}

fn ids(plan: &JobPlan) -> Vec<&str> {
    plan.hosts.iter().map(|host| host.id.as_str()).collect()
}

/// The options a run gets by default: the plan's own parallel number.
fn options(plan: &JobPlan) -> JobOptions {
    JobOptions {
        parallel: plan.default_parallel,
        ..JobOptions::default()
    }
}

/// What a run showed: each host's states and progress, and the report.
struct Ran {
    report: JobReport,
    states: BTreeMap<HostId, Vec<HostState>>,
    progress: BTreeMap<HostId, Vec<(u64, Option<u64>)>>,
    events: Vec<JobEvent>,
}

/// Follows `job` to its end.
async fn follow(mut job: Job) -> Ran {
    let mut states: BTreeMap<HostId, Vec<HostState>> = BTreeMap::new();
    let mut progress: BTreeMap<HostId, Vec<(u64, Option<u64>)>> = BTreeMap::new();
    let mut events = Vec::new();
    let report = tokio::time::timeout(WAIT * 3, async {
        loop {
            let event = job.next().await.expect("the job ended without a report");
            events.push(event.clone());
            match event {
                JobEvent::State { host, state } => states.entry(host).or_default().push(state),
                JobEvent::Progress { host, done, total } => {
                    progress.entry(host).or_default().push((done, total))
                }
                JobEvent::Output { .. } => {}
                JobEvent::Backup { host, .. } => panic!("a file job backed up {host}"),
                JobEvent::Finished(report) => return report,
            }
        }
    })
    .await
    .expect("the job did not finish in time");
    assert!(job.next().await.is_none(), "events after the report");
    Ran {
        report,
        states,
        progress,
        events,
    }
}

async fn run(manager: &SessionManager, plan: JobPlan) -> Ran {
    let options = options(&plan);
    follow(job::start(manager, plan, options).unwrap()).await
}

fn result<'a>(report: &'a JobReport, id: &str) -> &'a job::HostResult {
    report
        .hosts
        .iter()
        .find(|host| host.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is not in the report"))
}

fn state(report: &JobReport, id: &str) -> HostState {
    result(report, id).state.clone()
}

fn failure(report: &JobReport, id: &str) -> String {
    match state(report, id) {
        HostState::Failed { reason } => reason,
        other => panic!("{id} should have failed, but is {other:?}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Waits until `server` has no connection open.
async fn closes(server: &TestSshServer) {
    tokio::time::timeout(WAIT, async {
        while server.connections_open() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a file connection stayed open");
}

#[tokio::test]
async fn a_file_goes_to_every_host_of_a_folder_with_progress_and_its_hashes() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let bytes = content(3 * 1024 * 1024);
    let file = local_file(&local, "image.bin", &bytes);
    let servers = [
        sftp_server().await,
        sftp_server().await,
        sftp_server().await,
    ];
    let workspace = workspace(
        &temp,
        &[
            ("Site/r1.yaml", host(&servers[0].0, "imaging", "")),
            ("Site/r2.yaml", host(&servers[1].0, "imaging", "")),
            ("Site/r3.yaml", host(&servers[2].0, "imaging", "")),
        ],
    );
    let store = store(&workspace);
    let planned = plan(&workspace, &store, &folder("Site"), &send(&file, "", false));
    assert!(planned.ready());
    assert_eq!(
        ids(&planned),
        ["Site/r1.yaml", "Site/r2.yaml", "Site/r3.yaml"]
    );
    let planned_file = planned.file.clone().expect("the plan names the file");
    assert_eq!(planned_file.name, "image.bin");
    assert_eq!(planned_file.size, bytes.len() as u64);
    assert!(planned
        .hosts
        .iter()
        .all(|host| host.destination.as_deref() == Some("/images/image.bin")));

    let ran = run(&support::manager(), planned).await;
    for (index, id) in ["Site/r1.yaml", "Site/r2.yaml", "Site/r3.yaml"]
        .into_iter()
        .enumerate()
    {
        let host = HostId::new(id);
        assert_eq!(state(&ran.report, id), HostState::Done, "{id}");
        assert_eq!(result(&ran.report, id).bytes, Some(bytes.len() as u64));
        assert_eq!(
            ran.states[&host],
            [
                HostState::Waiting,
                HostState::Connecting,
                HostState::Running,
                HostState::Done
            ]
        );
        let progress = &ran.progress[&host];
        assert!(!progress.is_empty(), "{id} showed no progress");
        assert!(progress
            .windows(2)
            .all(|pair| pair[0].0 <= pair[1].0 && pair[1].1 == Some(bytes.len() as u64)));
        let sent = fs::read(servers[index].1.path().join("images/image.bin")).unwrap();
        assert!(sent == bytes, "{id} got the file byte for byte");
        let output = &result(&ran.report, id).output;
        assert!(output.contains("/images/image.bin"), "{output}");
    }
    let sent = ran.report.file.expect("the report names the file");
    assert_eq!(sent.name, "image.bin");
    assert_eq!(sent.size, bytes.len() as u64);
    let hashes = sent.hashes.expect("the file's hashes");
    assert_eq!(hashes.md5, hex(&md5::compute(&bytes).0));
    assert_eq!(hashes.sha256, hex(&sha2::Sha256::digest(&bytes)));
    for (server, _) in &servers {
        closes(server).await;
    }
}

#[tokio::test]
async fn pre_flight_skips_each_kind_of_host_with_its_reason_and_connects_to_nothing() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let file = local_file(&local, "image.bin", b"twelve bytes");
    let (server, _remote) = sftp_server().await;
    let workspace = workspace(
        &temp,
        &[
            ("prod.yaml", host(&server, "imaging", "environment: production\n")),
            ("ok.yaml", host(&server, "imaging", "")),
            ("telnet.yaml", host(&server, "imaging", "protocol: telnet\n")),
            (
                "no-profile.yaml",
                format!("address: 127.0.0.1\nport: {}\nplatform: imaging\n", server.port()),
            ),
            (
                "not-set-up.yaml",
                format!(
                    "address: 127.0.0.1\nport: {}\nplatform: imaging\ncredential_profile: elsewhere\n",
                    server.port()
                ),
            ),
            (
                "bastion.yaml",
                format!(
                    "address: 127.0.0.1\nport: {}\nplatform: imaging\n",
                    server.port()
                ),
            ),
            ("behind.yaml", host(&server, "imaging", "jump_host: bastion.yaml\n")),
            ("no-folder.yaml", host(&server, "bare", "")),
            (
                "no-platform.yaml",
                format!(
                    "address: 127.0.0.1\nport: {}\ncredential_profile: lab\n",
                    server.port()
                ),
            ),
        ],
    );
    let store = store(&workspace);
    let order = [
        "prod.yaml",
        "telnet.yaml",
        "no-profile.yaml",
        "not-set-up.yaml",
        "behind.yaml",
        "no-folder.yaml",
        "no-platform.yaml",
        "ok.yaml",
        "gone.yaml",
    ];
    let target = JobTarget::Hosts {
        ids: order.into_iter().map(HostId::new).collect(),
    };
    let planned = plan(&workspace, &store, &target, &send(&file, "", false));
    assert_eq!(ids(&planned), ["prod.yaml", "ok.yaml"]);
    assert_eq!(planned.production, [HostId::new("prod.yaml")]);
    assert_eq!(planned.file.as_ref().unwrap().size, 12);
    assert_eq!(planned.default_parallel, 3);
    let skipped: Vec<(&str, &SkipReason)> = planned
        .skipped
        .iter()
        .map(|host| (host.id.as_str(), &host.reason))
        .collect();
    assert_eq!(
        skipped,
        [
            (
                "telnet.yaml",
                &SkipReason::NotSsh {
                    protocol: Protocol::Telnet
                }
            ),
            ("no-profile.yaml", &SkipReason::NoProfile),
            (
                "not-set-up.yaml",
                &SkipReason::ProfileNotSetUp {
                    profile: "elsewhere".into()
                }
            ),
            (
                "behind.yaml",
                &SkipReason::JumpHostNeedsLogin {
                    jump_host: "bastion.yaml".into()
                }
            ),
            (
                "no-folder.yaml",
                &SkipReason::NoDestination {
                    platform: Some("bare".into())
                }
            ),
            (
                "no-platform.yaml",
                &SkipReason::NoDestination { platform: None }
            ),
            ("gone.yaml", &SkipReason::Gone),
        ]
    );
    let message = |id: &str| {
        planned
            .skipped
            .iter()
            .find(|host| host.id.as_str() == id)
            .unwrap()
            .message
            .clone()
    };
    assert_eq!(
        message("no-folder.yaml"),
        "its platform bare has no file folder, and no destination was typed"
    );
    assert_eq!(
        message("no-platform.yaml"),
        "it has no platform to give a file folder, and no destination was typed"
    );

    // A typed destination reaches the hosts without a file folder.
    let typed = plan(&workspace, &store, &target, &send(&file, "/images", false));
    assert_eq!(
        ids(&typed),
        ["prod.yaml", "no-folder.yaml", "no-platform.yaml", "ok.yaml"]
    );
    assert_eq!(server.connections(), 0, "pre-flight connects to nothing");

    // A file that is not there cannot be planned.
    let missing = local.path().join("missing.bin");
    let refused = job::plan(
        &workspace,
        &Profiles::new(&store, &workspace),
        &target,
        &send(&missing, "", false),
    );
    assert!(
        matches!(&refused, Err(PlanError::LocalFile { .. })),
        "{refused:?}"
    );
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("missing.bin"), "{message}");
}

#[tokio::test]
async fn a_typed_destination_with_a_host_variable_resolves_for_each_host() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let file = local_file(&local, "notes.txt", b"hello\n");
    let (server, remote) = sftp_server().await;
    for site in ["north", "south"] {
        fs::create_dir_all(remote.path().join("sites").join(site)).unwrap();
    }
    let workspace = workspace(
        &temp,
        &[
            (
                "a.yaml",
                host(&server, "bare", "custom:\n  region: north\n"),
            ),
            (
                "b.yaml",
                host(&server, "imaging", "custom:\n  region: south\n"),
            ),
            ("c.yaml", host(&server, "imaging", "")),
        ],
    );
    let store = store(&workspace);
    let planned = plan(
        &workspace,
        &store,
        &folder(""),
        &send(&file, "/sites/{{region}}", false),
    );
    assert_eq!(ids(&planned), ["a.yaml", "b.yaml"]);
    let destinations: Vec<Option<&str>> = planned
        .hosts
        .iter()
        .map(|host| host.destination.as_deref())
        .collect();
    assert_eq!(
        destinations,
        [
            Some("/sites/north/notes.txt"),
            Some("/sites/south/notes.txt")
        ]
    );
    assert_eq!(
        planned.skipped[0].reason,
        SkipReason::DestinationVariables {
            names: vec!["region".into()]
        }
    );
    assert_eq!(
        planned.skipped[0].message,
        "the destination uses {{region}}, which it has no value for"
    );

    let ran = run(&support::manager(), planned).await;
    assert_eq!(state(&ran.report, "a.yaml"), HostState::Done);
    assert_eq!(state(&ran.report, "b.yaml"), HostState::Done);
    for site in ["north", "south"] {
        let path = remote.path().join("sites").join(site).join("notes.txt");
        assert_eq!(fs::read(path).unwrap(), b"hello\n");
    }
}

#[tokio::test]
async fn without_overwrite_a_file_of_that_size_is_already_there_and_another_size_fails() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let file = local_file(&local, "image.bin", &content(100_000));
    let servers = [
        sftp_server().await,
        sftp_server().await,
        sftp_server().await,
    ];
    let workspace = workspace(
        &temp,
        &[
            ("r1.yaml", host(&servers[0].0, "imaging", "")),
            ("r2.yaml", host(&servers[1].0, "imaging", "")),
            ("r3.yaml", host(&servers[2].0, "imaging", "")),
        ],
    );
    let store = store(&workspace);
    let manager = support::manager();
    let first = run(
        &manager,
        plan(&workspace, &store, &folder(""), &send(&file, "", false)),
    )
    .await;
    assert!(first
        .report
        .hosts
        .iter()
        .all(|host| host.state == HostState::Done));

    // r2 now holds another file of that name.
    let other = servers[1].1.path().join("images/image.bin");
    fs::write(&other, b"something else").unwrap();
    let again = run(
        &manager,
        plan(&workspace, &store, &folder(""), &send(&file, "", false)),
    )
    .await;
    assert_eq!(state(&again.report, "r1.yaml"), HostState::AlreadyThere);
    assert_eq!(state(&again.report, "r3.yaml"), HostState::AlreadyThere);
    assert_eq!(result(&again.report, "r1.yaml").bytes, None);
    assert_eq!(
        failure(&again.report, "r2.yaml"),
        "a file of that name exists with another size (14 bytes on the device, 100000 to send); tick overwrite to replace it"
    );
    assert_eq!(fs::read(&other).unwrap(), b"something else");
    assert!(again.progress.is_empty(), "nothing was sent");

    let overwritten = run(
        &manager,
        plan(&workspace, &store, &folder(""), &send(&file, "", true)),
    )
    .await;
    for id in ["r1.yaml", "r2.yaml", "r3.yaml"] {
        assert_eq!(state(&overwritten.report, id), HostState::Done, "{id}");
        assert_eq!(result(&overwritten.report, id).bytes, Some(100_000));
    }
    assert_eq!(fs::read(&other).unwrap().len(), 100_000);
}

#[tokio::test]
async fn a_device_that_offers_only_scp_fails_without_overwrite_and_is_sent_to_with_it() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let file = local_file(&local, "image.bin", &content(200_000));
    let server = TestSshServer::start().await;
    let remote = TempDir::new().unwrap();
    fs::create_dir(remote.path().join("images")).unwrap();
    server.serve_scp(remote.path());
    // As IOS-XE on the lab's CSR 1000v (#108): the SFTP channel is closed
    // and the whole connection with it, and again after each SCP transfer.
    server.close_sftp_and_hang_up();
    server.scp_quirk(ScpQuirk::CloseConnectionAfter);
    let workspace = workspace(&temp, &[("scp.yaml", host(&server, "imaging", ""))]);
    let store = store(&workspace);
    let manager = support::manager();

    let refused = run(
        &manager,
        plan(&workspace, &store, &folder(""), &send(&file, "", false)),
    )
    .await;
    assert_eq!(
        failure(&refused.report, "scp.yaml"),
        "this device cannot say whether the file is there; tick overwrite to send it anyway"
    );
    assert!(!remote.path().join("images/image.bin").exists());

    let sent = run(
        &manager,
        plan(&workspace, &store, &folder(""), &send(&file, "", true)),
    )
    .await;
    assert_eq!(state(&sent.report, "scp.yaml"), HostState::Done);
    assert_eq!(result(&sent.report, "scp.yaml").bytes, Some(200_000));
    assert_eq!(
        fs::read(remote.path().join("images/image.bin")).unwrap(),
        content(200_000)
    );
}

#[tokio::test]
async fn an_untrusted_key_and_a_login_question_fail_their_hosts_and_the_others_finish() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let file = local_file(&local, "image.bin", b"data");
    let (trusted, trusted_folder) = sftp_server().await;
    let (unknown, _u) = sftp_server().await;
    let (otp, _o) = sftp_server().await;
    let known_hosts = KnownHosts::in_memory();
    for server in [&trusted, &otp] {
        known_hosts
            .remember("127.0.0.1", server.port(), server.host_key())
            .unwrap();
    }
    let workspace = workspace(
        &temp,
        &[
            ("a-trusted.yaml", host(&trusted, "imaging", "")),
            ("b-unknown.yaml", host(&unknown, "imaging", "")),
            (
                "c-otp.yaml",
                format!(
                    "address: 127.0.0.1\nport: {}\nplatform: imaging\ncredential_profile: otp\n",
                    otp.port()
                ),
            ),
        ],
    );
    let store = store(&workspace);
    let planned = plan(&workspace, &store, &folder(""), &send(&file, "", false));
    assert_eq!(planned.hosts.len(), 3);

    let ran = run(&SessionManager::new(known_hosts.clone()), planned).await;
    assert_eq!(state(&ran.report, "a-trusted.yaml"), HostState::Done);
    assert_eq!(
        fs::read(trusted_folder.path().join("images/image.bin")).unwrap(),
        b"data"
    );
    let reason = failure(&ran.report, "b-unknown.yaml");
    assert!(
        reason.contains(&format!("127.0.0.1 port {}", unknown.port()))
            && reason.contains("not trusted yet"),
        "{reason}"
    );
    let reason = failure(&ran.report, "c-otp.yaml");
    assert!(
        reason.contains(CODE_QUESTION.trim()) && reason.contains("never answers"),
        "{reason}"
    );
}

#[tokio::test]
async fn three_hosts_send_at_once_by_default_and_a_run_can_change_it() {
    for parallel in [None, Some(1)] {
        let temp = TempDir::new().unwrap();
        let local = TempDir::new().unwrap();
        let file = local_file(&local, "image.bin", &content(1024 * 1024));
        let (server, _remote) = sftp_server().await;
        // Every host is the same server, each in its own folder.
        let names: Vec<String> = (1..=5).map(|n| format!("r{n}")).collect();
        let files: Vec<(String, String)> = names
            .iter()
            .map(|name| {
                fs::create_dir_all(_remote.path().join(name)).unwrap();
                (
                    format!("{name}.yaml"),
                    host(&server, "imaging", &format!("custom:\n  folder: /{name}\n")),
                )
            })
            .collect();
        let files: Vec<(&str, String)> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.clone()))
            .collect();
        let workspace = workspace(&temp, &files);
        let store = store(&workspace);
        let planned = plan(
            &workspace,
            &store,
            &folder(""),
            &send(&file, "{{folder}}", false),
        );
        assert_eq!(planned.hosts.len(), 5);
        let mut options = options(&planned);
        if let Some(parallel) = parallel {
            options.parallel = parallel;
        }
        // Writes wait, so that every host that may send at once does.
        server.sftp_quirks().hold_from(0);
        let job = job::start(&support::manager(), planned, options).unwrap();
        let expected = parallel.unwrap_or(3);
        tokio::time::timeout(WAIT, async {
            while server.connections_open() < expected {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the hosts connected");
        tokio::time::sleep(Duration::from_millis(300)).await;
        server.sftp_quirks().release();
        let ran = follow(job).await;
        assert!(
            ran.report
                .hosts
                .iter()
                .all(|host| host.state == HostState::Done),
            "{:?}",
            ran.report
        );
        assert_eq!(server.most_connections_open(), expected);
        closes(&server).await;
    }
    assert_eq!(job::DEFAULT_FILE_PARALLEL, 3);
}

#[tokio::test]
async fn cancel_stops_the_run_removes_the_partial_file_and_marks_hosts_not_started() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let file = local_file(&local, "image.bin", &content(8 * 1024 * 1024));
    let (first, first_folder) = sftp_server().await;
    let (second, _s) = sftp_server().await;
    let (third, _t) = sftp_server().await;
    let workspace = workspace(
        &temp,
        &[
            ("r1.yaml", host(&first, "imaging", "")),
            ("r2.yaml", host(&second, "imaging", "")),
            ("r3.yaml", host(&third, "imaging", "")),
        ],
    );
    let store = store(&workspace);
    let planned = plan(&workspace, &store, &folder(""), &send(&file, "", false));
    first.sftp_quirks().hold_from(1024 * 1024);
    let options = JobOptions {
        parallel: 1,
        ..JobOptions::default()
    };
    let mut job = job::start(&support::manager(), planned, options).unwrap();
    let mut events = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let event = job.next().await.expect("an event");
            let moving = matches!(&event, JobEvent::Progress { done, .. } if *done > 0);
            events.push(event);
            if moving {
                return;
            }
        }
    })
    .await
    .expect("the first host sends");
    job.cancel();
    first.sftp_quirks().release();
    let ran = follow(job).await;

    assert_eq!(state(&ran.report, "r1.yaml"), HostState::Cancelled);
    let output = &result(&ran.report, "r1.yaml").output;
    assert!(
        output.contains("the partial file was removed from the device"),
        "{output}"
    );
    assert!(!first_folder.path().join("images/image.bin").exists());
    for id in ["r2.yaml", "r3.yaml"] {
        assert_eq!(state(&ran.report, id), HostState::Cancelled);
        assert_eq!(ran.states[&HostId::new(id)], [HostState::Cancelled]);
    }
    assert_eq!(second.connections(), 0, "a waiting host never connects");
    assert_eq!(third.connections(), 0);
    let _ = events;
}

#[tokio::test]
async fn the_kept_run_holds_the_file_its_hashes_and_each_host_and_cannot_be_saved() {
    let temp = TempDir::new().unwrap();
    let local = TempDir::new().unwrap();
    let bytes = content(50_000);
    let file = local_file(&local, "image.bin", &bytes);
    let (server, remote) = sftp_server().await;
    fs::write(remote.path().join("images/image.bin"), &bytes).unwrap();
    let (fresh, _f) = sftp_server().await;
    let mut workspace = workspace(
        &temp,
        &[
            ("Site/there.yaml", host(&server, "imaging", "")),
            ("Site/fresh.yaml", host(&fresh, "imaging", "")),
            (
                "Site/telnet.yaml",
                host(&fresh, "imaging", "protocol: telnet\n"),
            ),
        ],
    );
    let store = store(&workspace);
    let step = send(&file, "", false);
    let planned = plan(&workspace, &store, &folder("Site"), &step);
    let runs = RunStore::new(temp.path().join("jobs"));
    let workspace_id = workspace.id().unwrap().to_string();
    let mut recorder = runs
        .begin(&workspace_id, &folder("Site"), &planned)
        .unwrap();
    let run_id = recorder.id().to_string();
    let begun = runs.run(&workspace_id, &run_id).unwrap();
    let kept = begun.file.as_ref().expect("the run names its file");
    assert_eq!((kept.name.as_str(), kept.size), ("image.bin", 50_000));

    let ran = run(&support::manager(), planned).await;
    for event in &ran.events {
        recorder.record(event).unwrap();
    }

    let stored = runs.run(&workspace_id, &run_id).unwrap();
    assert!(stored.ended());
    assert_eq!(stored.step, step);
    let kept = stored.file.as_ref().unwrap();
    let hashes = kept.hashes.as_ref().expect("the hashes are kept");
    assert_eq!(hashes.md5, hex(&md5::compute(&bytes).0));
    assert_eq!(hashes.sha256, hex(&sha2::Sha256::digest(&bytes)));
    let there = stored.host(&HostId::new("Site/there.yaml")).unwrap();
    assert_eq!(there.state, HostState::AlreadyThere);
    assert_eq!(there.bytes, None);
    assert_eq!(there.destination.as_deref(), Some("/images/image.bin"));
    let sent = stored.host(&HostId::new("Site/fresh.yaml")).unwrap();
    assert_eq!(sent.state, HostState::Done);
    assert_eq!(sent.bytes, Some(50_000));
    assert_eq!(stored.skipped.len(), 1);

    // Run again plans the same step.
    let again = job::plan_again(&workspace, &Profiles::new(&store, &workspace), &stored).unwrap();
    assert_eq!(again.step, step);
    assert_eq!(again.hosts.len(), 2);

    let refused = workspace.save_job(
        None,
        &SavedJob {
            name: "Stage image".into(),
            step,
            target: folder("Site"),
            ignored_answers: Vec::new(),
        },
    );
    let message = refused.unwrap_err().to_string();
    assert!(
        message.contains("sending a file cannot be saved as a job"),
        "{message}"
    );
}
