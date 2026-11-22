//! What the edge tests of jobs share (#119): a temporary workspace made of
//! host files, the stored profiles its hosts log in with, and a run
//! followed to its end with every event kept.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use enodix_core::credentials::{
    CredentialProfile, CredentialStore, MemoryStore, ProfileAuth, Profiles,
};
use enodix_core::job::{
    self, HostResult, HostState, Job, JobEvent, JobOptions, JobPlan, JobReport, JobStep, JobTarget,
};
use enodix_core::session::SessionManager;
use enodix_core::workspace::{HostId, Workspace};
use tempfile::TempDir;

use super::{TestSshServer, PASSWORD, USERNAME};

/// A workspace with an id in a temporary folder.
pub struct Lab {
    pub temp: TempDir,
    pub workspace: Workspace,
    pub store: MemoryStore,
}

impl Lab {
    /// A workspace made of `files` (relative path, text), with the profile
    /// `lab` (the test servers' login) stored for it.
    pub fn new(files: &[(&str, String)]) -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
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
        let store = MemoryStore::new();
        let lab = Self {
            temp,
            workspace,
            store,
        };
        lab.put(profile("lab", USERNAME));
        lab
    }

    /// Stores `profile` for the workspace.
    pub fn put(&self, profile: CredentialProfile) {
        self.store
            .put(&self.workspace.credential_scope(), &profile)
            .unwrap();
    }

    pub fn root(&self) -> &Path {
        self.workspace.root()
    }

    /// The workspace's id.
    pub fn id(&self) -> String {
        self.workspace.id().unwrap().to_string()
    }

    /// A folder of the temporary folder, outside the workspace.
    pub fn folder(&self, name: &str) -> PathBuf {
        let folder = self.temp.path().join(name);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// Reads the workspace again from disk (after files were changed).
    pub fn reload(&mut self) {
        self.workspace = Workspace::open(self.workspace.root()).unwrap();
    }

    pub fn plan(&self, target: &JobTarget, step: &JobStep) -> JobPlan {
        job::plan(
            &self.workspace,
            &Profiles::new(&self.store, &self.workspace),
            target,
            step,
        )
        .unwrap()
    }
}

/// A password profile named `name` for `username`.
pub fn profile(name: &str, username: &str) -> CredentialProfile {
    CredentialProfile {
        name: name.into(),
        username: username.into(),
        auth: ProfileAuth::Password {
            password: PASSWORD.into(),
        },
        enable_secret: None,
    }
}

/// A host file for a test server on the loopback address, on `platform`,
/// logging in with the profile `lab`.
pub fn host(server: &TestSshServer, platform: &str, extra: &str) -> String {
    host_at(server.port(), platform, extra)
}

/// A host file for a port of the loopback address.
/// (`extra` may name another profile.)
pub fn host_at(port: u16, platform: &str, extra: &str) -> String {
    let profile = if extra.contains("credential_profile:") {
        ""
    } else {
        "credential_profile: lab\n"
    };
    format!("address: 127.0.0.1\nport: {port}\nplatform: {platform}\n{profile}{extra}")
}

pub fn snippet(name: &str) -> JobStep {
    JobStep::Snippet {
        name: name.into(),
        answers: BTreeMap::new(),
    }
}

pub fn folder(path: &str) -> JobTarget {
    JobTarget::Folder { path: path.into() }
}

/// A run followed to its end.
pub struct Ran {
    pub report: JobReport,
    /// Every event but the report, in order.
    pub events: Vec<JobEvent>,
    /// When each of them came, from the start.
    pub times: Vec<Duration>,
    /// From the start to the report.
    pub took: Duration,
}

impl Ran {
    pub fn result(&self, id: &str) -> &HostResult {
        self.report
            .hosts
            .iter()
            .find(|host| host.id.as_str() == id)
            .unwrap_or_else(|| panic!("{id} is not in the report"))
    }

    pub fn state(&self, id: &str) -> &HostState {
        &self.result(id).state
    }

    /// The reason `id` failed (it must have).
    pub fn failure(&self, id: &str) -> String {
        match self.state(id) {
            HostState::Failed { reason } => reason.clone(),
            other => panic!(
                "{id} should have failed, but is {other:?}: {}",
                self.result(id).output
            ),
        }
    }

    /// Each host's states, in order.
    pub fn states(&self) -> BTreeMap<HostId, Vec<HostState>> {
        let mut states: BTreeMap<HostId, Vec<HostState>> = BTreeMap::new();
        for event in &self.events {
            if let JobEvent::State { host, state } = event {
                states.entry(host.clone()).or_default().push(state.clone());
            }
        }
        states
    }

    /// When `id` reached its final state, from the start.
    pub fn ended_at(&self, id: &str) -> Duration {
        self.events
            .iter()
            .zip(&self.times)
            .find_map(|(event, at)| match event {
                JobEvent::State { host, state } if host.as_str() == id && state.is_final() => {
                    Some(*at)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{id} never ended"))
    }

    /// Each host's output as its events streamed it.
    pub fn streamed(&self) -> BTreeMap<HostId, String> {
        let mut streamed: BTreeMap<HostId, String> = BTreeMap::new();
        for event in &self.events {
            if let JobEvent::Output { host, text } = event {
                streamed.entry(host.clone()).or_default().push_str(text);
            }
        }
        streamed
    }
}

/// Follows `job` to its end (at most `within`), calling `each` on every
/// event (with the job, to cancel it), and checks what every job owes: one
/// report last, every host of the plan in it once and final, streamed
/// output equal to the report's.
pub async fn follow(mut job: Job, within: Duration, mut each: impl FnMut(&Job, &JobEvent)) -> Ran {
    let started = Instant::now();
    let mut events = Vec::new();
    let mut times = Vec::new();
    let report = tokio::time::timeout(within, async {
        loop {
            let event = job.next().await.expect("the job ended without a report");
            each(&job, &event);
            if let JobEvent::Finished(report) = event {
                return report;
            }
            events.push(event);
            times.push(started.elapsed());
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the job did not end within {within:?}"));
    let took = started.elapsed();
    assert!(job.next().await.is_none(), "events after the report");
    let ran = Ran {
        report,
        events,
        times,
        took,
    };
    let streamed = ran.streamed();
    for host in &ran.report.hosts {
        assert!(host.state.is_final(), "{} ended {:?}", host.id, host.state);
        assert_eq!(
            streamed.get(&host.id).map(String::as_str).unwrap_or(""),
            host.output,
            "the streamed output of {} is its output",
            host.id
        );
        let states = &ran.states()[&host.id];
        assert_eq!(states.first(), Some(&HostState::Waiting), "{}", host.id);
        assert_eq!(states.last(), Some(&host.state), "{}", host.id);
        assert_eq!(
            states.iter().filter(|state| state.is_final()).count(),
            1,
            "{} has one final state: {states:?}",
            host.id
        );
    }
    ran
}

/// Starts `plan` with `options` and follows it to its end.
pub async fn run(
    manager: &SessionManager,
    plan: JobPlan,
    options: JobOptions,
    within: Duration,
) -> Ran {
    let hosts: Vec<HostId> = plan.hosts.iter().map(|host| host.id.clone()).collect();
    let job = job::start(manager, plan, options).unwrap();
    let ran = follow(job, within, |_, _| {}).await;
    let reported: Vec<&HostId> = ran.report.hosts.iter().map(|host| &host.id).collect();
    assert_eq!(
        reported,
        hosts.iter().collect::<Vec<_>>(),
        "every host once, in plan order"
    );
    ran
}

/// Waits until `server` has no connection with a session open (at most
/// `within`). (A server stalled on purpose inside a login or a shell
/// request keeps its side of the connection, whatever the client did.)
pub async fn closes(server: &TestSshServer, within: Duration) {
    let until = Instant::now() + within;
    while server.connections_open() > 0 {
        assert!(
            Instant::now() < until,
            "{} connections stayed open",
            server.connections_open()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
