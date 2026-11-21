//! Script jobs: a Python script of the workspace run once for a job's
//! hosts. Most tests run a stand-in interpreter (a batch file or shell
//! script, `support::python::FakePython::running`) that prints what the
//! job gave it; one runs a tiny real Python script and is SKIPPED without
//! Python 3.10+.

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use netterm_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use netterm_core::job::results::{OutputRef, RunStore, OUTPUT_FOLDER};
use netterm_core::job::script::{ScriptTargets, TARGETS_FORMAT};
use netterm_core::job::{
    self, HostState, Job, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget, PlanError,
    RunningJobs, ScriptOptions, SkipReason, StartError, APP_CLOSED, APP_CLOSE_WAIT, SCRIPT_OUTPUT,
};
use netterm_core::workspace::{HostId, ScriptError, Workspace};
use support::processes;
use support::python::{real_python, FakePython};
use tempfile::TempDir;

/// The workspace every test uses: three hosts that run (one telnet, one
/// production), one whose profile is not stored here, and some scripts
/// among files that are not listed.
fn workspace(temp: &TempDir) -> Workspace {
    workspace_in(temp.path())
}

/// [`workspace`], in `folder`.
fn workspace_in(folder: &Path) -> Workspace {
    let root = folder.join("workspace");
    let files = [
        (
            "Madrid/r1.yaml",
            "address: 10.0.0.1\ncredential_profile: lab\n",
        ),
        (
            "Madrid/p1.yaml",
            "address: 10.0.0.2\nenvironment: production\n",
        ),
        ("Madrid/t1.yaml", "address: 10.0.0.3\nprotocol: telnet\n"),
        (
            "Madrid/ghost.yaml",
            "address: 10.0.0.4\ncredential_profile: ghost\n",
        ),
        ("Paris/r9.yaml", "address: 10.0.9.9\n"),
        ("scripts/show.py", "print('hello')\n"),
        ("top.py", "print('top')\n"),
        ("Madrid/notes.txt", "not a script\n"),
        (".hidden/secret.py", "print('hidden')\n"),
        ("scripts/__pycache__/show.py", "\n"),
        ("venv/pyvenv.cfg", "home = x\n"),
        ("venv/lib/site.py", "\n"),
    ];
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

fn store(workspace: &Workspace) -> MemoryStore {
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
    store
}

fn script(path: &str) -> JobStep {
    JobStep::Script { path: path.into() }
}

fn madrid() -> JobTarget {
    JobTarget::Folder {
        path: "Madrid".into(),
    }
}

fn plan(workspace: &Workspace, step: &JobStep) -> JobPlan {
    let store = store(workspace);
    job::plan(
        workspace,
        &Profiles::new(&store, workspace),
        &madrid(),
        step,
    )
    .unwrap()
}

fn ids(plan: &JobPlan) -> Vec<&str> {
    plan.hosts.iter().map(|host| host.id.as_str()).collect()
}

fn options(interpreter: &Path) -> JobOptions {
    JobOptions {
        script: Some(ScriptOptions {
            interpreter: interpreter.to_path_buf(),
            cli: Some("C:/somewhere/netterm-cli.exe".into()),
            environment: vec![("NETTERM_TEST_EXTRA".into(), "extra-value".into())],
        }),
        ..JobOptions::default()
    }
}

/// Every event until the end, with the report.
async fn finish(job: &mut Job) -> (Vec<JobEvent>, JobReport) {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), job.next())
            .await
            .expect("the job ends")
            .expect("the job reports its end");
        if let JobEvent::Finished(report) = event {
            return (events, report);
        }
        events.push(event);
    }
}

fn states(events: &[JobEvent]) -> Vec<HostState> {
    events
        .iter()
        .filter_map(|event| match event {
            JobEvent::State { host, state } => {
                assert_eq!(host.as_str(), SCRIPT_OUTPUT);
                Some(state.clone())
            }
            _ => None,
        })
        .collect()
}

/// A stand-in that prints its environment, working folder, script and the
/// targets file, then exits 0.
fn printing(folder: &Path) -> FakePython {
    FakePython::running(
        folder,
        "printing",
        "echo workspace=%NETTERM_WORKSPACE%\r\necho cli=%NETTERM_CLI%\r\necho extra=%NETTERM_TEST_EXTRA%\r\necho args=%1 %2\r\necho cwd=%CD%\r\ntype \"%NETTERM_JOB_TARGETS%\"\r\necho.\r\necho targets=%NETTERM_JOB_TARGETS%",
        "echo \"workspace=$NETTERM_WORKSPACE\"\necho \"cli=$NETTERM_CLI\"\necho \"extra=$NETTERM_TEST_EXTRA\"\necho \"args=$1 $2\"\necho \"cwd=$(pwd)\"\ncat \"$NETTERM_JOB_TARGETS\"\necho\necho \"targets=$NETTERM_JOB_TARGETS\"",
    )
}

#[test]
fn the_workspace_lists_its_python_scripts() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    assert_eq!(workspace.scripts(), ["scripts/show.py", "top.py"]);
    assert_eq!(
        workspace.script_file("scripts\\show.py").unwrap(),
        workspace.root().join("scripts/show.py")
    );
    for bad in [
        "../top.py",
        "Madrid/notes.txt",
        ".hidden/secret.py",
        "",
        "/top.py",
        "scripts/__pycache__/show.py",
        "venv/lib/site.py",
    ] {
        assert_eq!(
            workspace.script_file(bad),
            Err(ScriptError::BadPath(bad.into())),
            "{bad}"
        );
    }
    assert_eq!(
        workspace.script_file("gone.py"),
        Err(ScriptError::NotFound("gone.py".into()))
    );
}

#[test]
fn pre_flight_lists_the_targets_and_skips_only_what_the_inventory_cannot_hand_over() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &script("scripts/show.py"));
    // Telnet and no profile are the script's business; a profile named but
    // not stored here would stop the inventory from loading.
    assert_eq!(
        ids(&plan),
        ["Madrid/p1.yaml", "Madrid/r1.yaml", "Madrid/t1.yaml"]
    );
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(plan.skipped[0].id.as_str(), "Madrid/ghost.yaml");
    assert_eq!(
        plan.skipped[0].reason,
        SkipReason::ProfileNotSetUp {
            profile: "ghost".into()
        }
    );
    assert_eq!(plan.production, [HostId::new("Madrid/p1.yaml")]);
    assert!(plan.ready());
}

/// Hosts behind jump hosts run (the plugin tunnels to them), unless their
/// chain cannot be tunnelled: broken, a jump host with no stored profile,
/// or telnet.
#[test]
fn pre_flight_skips_hosts_behind_jump_hosts_that_cannot_be_tunnelled() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let root = workspace.root().to_path_buf();
    let files = [
        (
            "bastion.yaml",
            "address: 10.9.0.1\ncredential_profile: lab\n",
        ),
        ("open.yaml", "address: 10.9.0.2\n"),
        (
            "Behind/ok.yaml",
            "address: 10.2.0.1\njump_host: bastion.yaml\n",
        ),
        (
            "Behind/no-login.yaml",
            "address: 10.2.0.2\njump_host: open.yaml\n",
        ),
        (
            "Behind/broken.yaml",
            "address: 10.2.0.3\njump_host: gone.yaml\n",
        ),
        (
            "Behind/telnet.yaml",
            "address: 10.2.0.4\nprotocol: telnet\njump_host: bastion.yaml\n",
        ),
        (
            "Behind/serial.yaml",
            "address: x\nprotocol: serial\njump_host: bastion.yaml\n",
        ),
    ];
    for (path, text) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    let workspace = Workspace::open(&root).unwrap();
    let store = store(&workspace);
    let plan = job::plan(
        &workspace,
        &Profiles::new(&store, &workspace),
        &JobTarget::Folder {
            path: "Behind".into(),
        },
        &script("scripts/show.py"),
    )
    .unwrap();
    assert_eq!(ids(&plan), ["Behind/ok.yaml", "Behind/serial.yaml"]);
    let skipped: Vec<(&str, &SkipReason)> = plan
        .skipped
        .iter()
        .map(|host| (host.id.as_str(), &host.reason))
        .collect();
    assert_eq!(skipped.len(), 3, "{skipped:?}");
    assert_eq!(skipped[0].0, "Behind/broken.yaml");
    assert!(matches!(skipped[0].1, SkipReason::JumpChain { .. }));
    assert_eq!(
        skipped[1],
        (
            "Behind/no-login.yaml",
            &SkipReason::BehindJumpHost {
                jump_host: "open.yaml".into()
            }
        )
    );
    assert_eq!(
        skipped[2],
        (
            "Behind/telnet.yaml",
            &SkipReason::TelnetBehindJumpHost {
                jump_host: "bastion.yaml".into()
            }
        )
    );
    assert!(plan.skipped[2].message.contains("only over SSH"));
}

#[test]
fn a_script_the_workspace_does_not_have_cannot_be_planned() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let store = store(&workspace);
    let profiles = Profiles::new(&store, &workspace);
    assert_eq!(
        job::plan(&workspace, &profiles, &madrid(), &script("missing.py")),
        Err(PlanError::NoSuchScript(ScriptError::NotFound(
            "missing.py".into()
        )))
    );
    assert_eq!(
        job::plan(&workspace, &profiles, &madrid(), &script("../top.py")),
        Err(PlanError::NoSuchScript(ScriptError::BadPath(
            "../top.py".into()
        )))
    );
}

#[tokio::test]
async fn without_an_interpreter_a_script_job_does_not_start() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &script("top.py"));
    assert_eq!(
        job::check(&plan, &JobOptions::default()),
        Err(StartError::NoPython)
    );
    let manager = support::manager();
    assert!(matches!(
        job::start(&manager, plan, JobOptions::default()),
        Err(StartError::NoPython)
    ));
}

#[tokio::test]
async fn the_script_gets_exactly_the_jobs_hosts_and_succeeds_by_exit_status() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let python = printing(temp.path());
    let plan = plan(&workspace, &script("scripts/show.py"));
    let manager = support::manager();
    let mut job = job::start(&manager, plan, options(&python.path)).unwrap();
    let (events, report) = finish(&mut job).await;

    assert_eq!(
        states(&events),
        [HostState::Waiting, HostState::Running, HostState::Done]
    );
    let [result] = report.hosts.as_slice() else {
        panic!("one output: {report:?}")
    };
    assert_eq!(result.id.as_str(), SCRIPT_OUTPUT);
    assert_eq!(result.name, "scripts/show.py");
    assert_eq!(result.exit_status, Some(0));
    let output = &result.output;
    let streamed: String = events
        .iter()
        .filter_map(|event| match event {
            JobEvent::Output { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(&streamed, output);

    let line = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name}= in {output}"))
            .to_string()
    };
    let root = workspace.root();
    assert_eq!(Path::new(&line("workspace")), root);
    assert_eq!(Path::new(&line("cwd")), root);
    assert_eq!(line("cli"), "C:/somewhere/netterm-cli.exe");
    assert_eq!(line("extra"), "extra-value");
    let args = line("args");
    assert!(args.starts_with("-u "), "{args}");
    assert!(args.ends_with("show.py"), "{args}");

    // The targets file it was given, and gone once the script ended.
    let json: String = output
        .lines()
        .skip_while(|line| !line.starts_with('{'))
        .take_while(|line| !line.starts_with("targets="))
        .collect();
    let targets: ScriptTargets = serde_json::from_str(&json).unwrap();
    assert_eq!(targets.format, TARGETS_FORMAT);
    assert_eq!(targets.workspace, root);
    assert_eq!(
        targets.hosts,
        ["Madrid/p1.yaml", "Madrid/r1.yaml", "Madrid/t1.yaml"].map(HostId::new)
    );
    assert!(!Path::new(&line("targets")).exists());
}

#[tokio::test]
async fn a_script_that_exits_with_another_status_fails_naming_it() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let python = FakePython::running(
        temp.path(),
        "failing",
        "echo to stdout\r\necho to stderr 1>&2\r\nexit /b 3",
        "echo to stdout\necho to stderr >&2\nexit 3",
    );
    let plan = plan(&workspace, &script("top.py"));
    let manager = support::manager();
    let mut job = job::start(&manager, plan, options(&python.path)).unwrap();
    let (_, report) = finish(&mut job).await;
    let result = &report.hosts[0];
    assert_eq!(
        result.state,
        HostState::Failed {
            reason: "the script exited with status 3".into()
        }
    );
    assert_eq!(result.exit_status, Some(3));
    assert!(result.output.contains("to stdout\n"), "{}", result.output);
    assert!(result.output.contains("to stderr\n"), "{}", result.output);
}

#[tokio::test]
async fn an_interpreter_that_cannot_start_fails_the_job() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let plan = plan(&workspace, &script("top.py"));
    let manager = support::manager();
    let missing = temp.path().join("no-python.exe");
    let mut job = job::start(&manager, plan, options(&missing)).unwrap();
    let (_, report) = finish(&mut job).await;
    let HostState::Failed { reason } = &report.hosts[0].state else {
        panic!("{report:?}")
    };
    assert!(
        reason.starts_with("the script could not be started"),
        "{reason}"
    );
    assert_eq!(report.hosts[0].exit_status, None);
}

#[tokio::test]
async fn output_streams_while_the_script_runs_and_cancel_stops_it_keeping_the_output() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let python = FakePython::running(
        temp.path(),
        "slow",
        "echo started\r\nping -n 60 127.0.0.1 > nul\r\necho never",
        "echo started\nsleep 60\necho never",
    );
    let plan = plan(&workspace, &script("top.py"));
    let manager = support::manager();
    let mut job = job::start(&manager, plan, options(&python.path)).unwrap();

    // The first line arrives while the script still runs.
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), job.next())
            .await
            .expect("output arrives while it runs")
            .unwrap();
        match event {
            JobEvent::Output { text, .. } => {
                assert_eq!(text, "started\n");
                break;
            }
            JobEvent::Finished(report) => panic!("ended before any output: {report:?}"),
            _ => {}
        }
    }
    let cancelled = std::time::Instant::now();
    job.cancel();
    let (_, report) = finish(&mut job).await;
    assert!(
        cancelled.elapsed() < Duration::from_secs(10),
        "{:?}",
        cancelled.elapsed()
    );
    let result = &report.hosts[0];
    assert_eq!(result.state, HostState::Cancelled);
    assert_eq!(result.output, "started\n");
}

/// This test binary run again as a helper process (see
/// `support::processes`): a tree of processes, or [`JOB_OWNER`].
#[test]
fn helper() {
    processes::play();
    if processes::role().as_deref() == Some(JOB_OWNER) {
        own_a_job();
    }
}

/// A process that starts a script job running a [`tree`] and then goes
/// away without cancelling it (killed by the test).
const JOB_OWNER: &str = "job owner";

/// A stand-in interpreter that runs a process tree (`support::processes`):
/// the interpreter, its child and its grandchild.
fn tree(folder: &Path) -> FakePython {
    let line = processes::tree_line();
    FakePython::running(folder, "tree", &line, &line)
}

/// Script options running `python` with what a tree needs to know.
fn tree_options(python: &Path, pids: &Path) -> JobOptions {
    let mut options = options(python);
    if let Some(script) = options.script.as_mut() {
        script.environment.extend(processes::tree_environment(pids));
    }
    options
}

/// The ids of a started tree's child and grandchild.
fn tree_pids(pids: &Path) -> [u32; 2] {
    let found = processes::wait_for_pids(
        pids,
        &[processes::CHILD, processes::GRANDCHILD],
        Duration::from_secs(60),
    )
    .expect("the script started its child and grandchild");
    [found[0], found[1]]
}

/// Asserts that every process in `pids` ends (waiting on them, not on a
/// time: a busy machine only makes it slower).
fn assert_gone(pids: &[u32], what: &str) {
    let alive = processes::wait_gone(pids, Duration::from_secs(30));
    assert!(
        alive.is_empty(),
        "{what}: still running {alive:?} of {pids:?}"
    );
}

/// Until the cause was found (#124) this test watched for a file a
/// process the script left behind wrote two seconds after it started:
/// when cancelling took longer than that on a busy machine (taskkill had
/// to start first), the file came in time and the test failed. It now
/// waits for the processes themselves to be gone.
#[tokio::test]
async fn cancelling_also_ends_the_processes_the_script_started() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let pids = temp.path().join("pids");
    fs::create_dir(&pids).unwrap();
    let python = tree(temp.path());
    let plan = plan(&workspace, &script("top.py"));
    let manager = support::manager();
    let mut job = job::start(&manager, plan, tree_options(&python.path, &pids)).unwrap();
    let started = tree_pids(&pids);
    job.cancel();
    let (_, report) = finish(&mut job).await;
    assert_eq!(report.hosts[0].state, HostState::Cancelled);
    assert_gone(&started, "the script's child and grandchild after a cancel");
}

/// A script that exits leaves nothing behind: what it started and left
/// running ends with it (#124; it used to keep running).
#[tokio::test]
async fn the_processes_a_script_leaves_behind_end_when_it_exits() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let pids = temp.path().join("pids");
    fs::create_dir(&pids).unwrap();
    let line = processes::tree_line();
    let python = FakePython::running(
        temp.path(),
        "leaving",
        &format!(
            "start \"\" /b {line}\r\n\
             :wait\r\n\
             if not exist \"%NETTERM_TEST_PIDS%\\{grandchild}\" (\r\n\
             ping -n 2 127.0.0.1 > nul\r\n\
             goto wait\r\n\
             )\r\n\
             echo leaving\r\n\
             exit /b 0",
            grandchild = processes::GRANDCHILD
        ),
        &format!(
            "{line} &\n\
             while [ ! -f \"$NETTERM_TEST_PIDS/{grandchild}\" ]; do sleep 0.05; done\n\
             echo leaving\n\
             exit 0",
            grandchild = processes::GRANDCHILD
        ),
    );
    let plan = plan(&workspace, &script("top.py"));
    let manager = support::manager();
    let mut job = job::start(&manager, plan, tree_options(&python.path, &pids)).unwrap();
    let (_, report) = finish(&mut job).await;
    assert_eq!(report.hosts[0].state, HostState::Done, "{report:?}");
    assert!(report.hosts[0].output.contains("leaving\n"));
    let left = tree_pids(&pids);
    assert_gone(&left, "what the script left behind after it exited");
}

/// Closing the app (#124): every running job is cancelled as Cancel does,
/// the script's processes end, and the kept run says the app was closed.
/// A run the user cancelled says nothing of the kind.
#[tokio::test]
async fn closing_the_app_cancels_a_running_script_and_its_kept_run_says_so() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let ws = workspace.id().unwrap().to_string();
    let store = RunStore::new(temp.path().join("data/jobs"));
    let jobs = RunningJobs::new();
    let manager = support::manager();
    let python = tree(temp.path());

    let follow = |pids: &Path| {
        let plan = plan(&workspace, &script("top.py"));
        let recorder = store.begin(&ws, &madrid(), &plan).unwrap();
        let mut job = job::start(&manager, plan, tree_options(&python.path, pids)).unwrap();
        let mut run = jobs.follow(job.canceller(), recorder);
        let id = (run.id(), run.run().to_string());
        let recording = tokio::spawn(async move {
            while let Some(event) = job.next().await {
                run.record(&event).unwrap();
            }
        });
        (id, recording)
    };

    // One the user cancels.
    let cancelled_pids = temp.path().join("cancelled");
    fs::create_dir(&cancelled_pids).unwrap();
    let ((id, cancelled), recording) = follow(&cancelled_pids);
    let started = tree_pids(&cancelled_pids);
    assert!(jobs.cancel(id));
    recording.await.unwrap();
    assert_gone(&started, "after Cancel");
    let kept = store.run(&ws, &cancelled).unwrap();
    assert!(kept.ended());
    assert_eq!(kept.hosts[0].state, HostState::Cancelled);
    assert_eq!(kept.cancel_reason, None);
    assert!(!jobs.cancel(id), "an ended job is no longer running");

    // One that runs when the app closes.
    let pids = temp.path().join("pids");
    fs::create_dir(&pids).unwrap();
    let ((_, run), recording) = follow(&pids);
    let started = tree_pids(&pids);
    assert_eq!(jobs.count(), 1);
    let closing = jobs.clone();
    let left = tokio::task::spawn_blocking(move || closing.end_all(APP_CLOSED, APP_CLOSE_WAIT))
        .await
        .unwrap();
    assert_eq!(left, 0, "every job ended within the wait");
    assert_eq!(jobs.count(), 0);
    recording.await.unwrap();
    assert_gone(&started, "after closing the app");

    // Its kept run can be opened, ended, and says why.
    let kept = store.run(&ws, &run).unwrap();
    assert!(kept.ended());
    assert_eq!(kept.cancel_reason.as_deref(), Some(APP_CLOSED));
    assert_eq!(kept.hosts[0].state, HostState::Cancelled);
    // Its output so far opens as ever.
    store
        .output(
            &ws,
            &OutputRef {
                run: run.clone(),
                host: HostId::new(SCRIPT_OUTPUT),
            },
        )
        .unwrap();
    let listed = store.list(&ws).unwrap();
    assert_eq!(listed.runs[0].cancel_reason.as_deref(), Some(APP_CLOSED));

    // Nothing runs: closing has nothing to wait for.
    assert_eq!(jobs.end_all(APP_CLOSED, APP_CLOSE_WAIT), 0);
}

/// The app is killed, or crashes (#124): no exit hook runs, yet the
/// script's processes end with it. The owner is this binary run again
/// ([`JOB_OWNER`]), which starts the job and is then killed by the test.
#[test]
fn a_scripts_processes_end_when_the_app_is_killed() {
    let temp = TempDir::new().unwrap();
    let pids = temp.path().join("pids");
    fs::create_dir(&pids).unwrap();
    let mut owner = processes::helper(JOB_OWNER, &pids)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let found = processes::wait_for_pids(
        &pids,
        &[processes::CHILD, processes::GRANDCHILD],
        Duration::from_secs(60),
    );
    let Some(started) = found else {
        let _ = owner.kill();
        panic!("the owner's script did not start: {:?}", owner.wait());
    };
    assert!(
        owner.try_wait().unwrap().is_none(),
        "the owner went away by itself"
    );
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert_gone(&started, "after the job's owner was killed");
}

/// [`JOB_OWNER`]: starts a script job running a tree in a workspace of its
/// own, and holds it without ever cancelling it until it is killed.
fn own_a_job() -> ! {
    let pids = std::path::PathBuf::from(std::env::var_os(processes::PIDS_VARIABLE).unwrap());
    let folder = pids.parent().unwrap().join("owner");
    fs::create_dir_all(&folder).unwrap();
    let workspace = workspace_in(&folder);
    let python = tree(&folder);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let plan = plan(&workspace, &script("top.py"));
        let manager = support::manager();
        let job = job::start(&manager, plan, tree_options(&python.path, &pids)).unwrap();
        // Held, never cancelled: the test kills this process.
        tokio::time::sleep(Duration::from_secs(60)).await;
        drop(job);
    });
    std::process::exit(0)
}
#[tokio::test]
async fn a_script_run_is_kept_with_its_targets_one_output_and_its_exit_status() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    let python = FakePython::running(
        temp.path(),
        "reporting",
        "echo checked 3 hosts\r\necho Madrid/r1.yaml ok\r\nexit /b 1",
        "echo checked 3 hosts\necho Madrid/r1.yaml ok\nexit 1",
    );
    let step = script("scripts/show.py");
    let plan = plan(&workspace, &step);
    let store = RunStore::new(temp.path().join("data/jobs"));
    let ws = workspace.id().unwrap().to_string();
    let mut recorder = store.begin(&ws, &madrid(), &plan).unwrap();
    let run = recorder.id().to_string();
    let manager = support::manager();
    let mut job = job::start(&manager, plan, options(&python.path)).unwrap();
    while let Some(event) = job.next().await {
        recorder.record(&event).unwrap();
    }

    let stored = store.run(&ws, &run).unwrap();
    assert!(stored.ended());
    assert_eq!(stored.step, step);
    let [script_host] = stored.hosts.as_slice() else {
        panic!("{stored:?}")
    };
    assert_eq!(script_host.id.as_str(), SCRIPT_OUTPUT);
    assert_eq!(script_host.name, "scripts/show.py");
    assert_eq!(script_host.file, format!("{OUTPUT_FOLDER}/script.txt"));
    assert_eq!(script_host.exit_status, Some(1));
    assert!(matches!(script_host.state, HostState::Failed { .. }));
    let targets: Vec<&str> = stored.targets.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        targets,
        ["Madrid/p1.yaml", "Madrid/r1.yaml", "Madrid/t1.yaml"]
    );
    assert_eq!(stored.skipped.len(), 1);

    let reference = OutputRef {
        run: run.clone(),
        host: HostId::new(SCRIPT_OUTPUT),
    };
    assert_eq!(
        store.output(&ws, &reference).unwrap(),
        "checked 3 hosts\nMadrid/r1.yaml ok\n"
    );
    let hits = store.search(&ws, &run, "r1.yaml").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].lines[0].number, 2);
    assert_eq!(store.list(&ws).unwrap().runs[0].id, run);

    // Run again: the same script and target, against the workspace now.
    let again = {
        let credentials = self::store(&workspace);
        job::plan_again(
            &workspace,
            &Profiles::new(&credentials, &workspace),
            &stored,
        )
        .unwrap()
    };
    assert_eq!(again.step, step);
    assert_eq!(
        ids(&again),
        ["Madrid/p1.yaml", "Madrid/r1.yaml", "Madrid/t1.yaml"]
    );
}

#[tokio::test]
async fn a_real_python_script_prints_its_targets() {
    let Some(python) = real_python() else {
        eprintln!("script job with a real Python SKIPPED: no Python 3.10+ found");
        return;
    };
    let temp = TempDir::new().unwrap();
    let workspace = workspace(&temp);
    fs::write(
        workspace.root().join("scripts/targets.py"),
        "import json, os, sys\n\
         with open(os.environ['NETTERM_JOB_TARGETS'], encoding='utf-8') as f:\n\
         \x20   targets = json.load(f)\n\
         for host in targets['hosts']:\n\
         \x20   print('target', host)\n\
         print('done', file=sys.stderr)\n\
         sys.exit(2)\n",
    )
    .unwrap();
    let plan = plan(&workspace, &script("scripts/targets.py"));
    let manager = support::manager();
    let mut job = job::start(&manager, plan, options(&python.executable)).unwrap();
    let (_, report) = finish(&mut job).await;
    let result = &report.hosts[0];
    assert_eq!(result.exit_status, Some(2), "{}", result.output);
    assert_eq!(
        result.output,
        "target Madrid/p1.yaml\ntarget Madrid/r1.yaml\ntarget Madrid/t1.yaml\ndone\n"
    );
}
