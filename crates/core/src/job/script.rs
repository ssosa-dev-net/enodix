//! Script jobs: a Python script of the workspace run once for a job's
//! hosts.
//!
//! The script runs as its own process: `<interpreter> -u <script>`, in the
//! workspace folder, with no console window and no input. It learns its
//! hosts from [`ScriptTargets`], a small JSON file named by
//! [`TARGETS_VARIABLE`]: the headless binary (and so the Python package's
//! inventory and its Nornir plugin) then shows the script only those hosts
//! and hands over only their logins. [`WORKSPACE_VARIABLE`] names the
//! workspace folder (the plugin's default) and [`CLI_VARIABLE`] the
//! headless binary. Its output (stdout and stderr share one pipe, so their
//! lines keep the order they were written in; as plain text, line by line
//! as it comes) is one stream, reported under the host id
//! [`SCRIPT_OUTPUT`]; the exit status decides how it ends: 0 is
//! [`HostState::Done`], anything else [`HostState::Failed`]. Cancelling
//! ends the process and the processes it started, keeping the output so
//! far. What a script that exits leaves running ends with it, and the
//! whole tree ends with this process however it ends, a crash or a kill
//! included (`crate::process`). The targets file is removed when the
//! script has ended.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use super::run::{HostResult, HostState, JobEvent, JobReport, KeptOutput};
use super::{JobPlan, JobStep, PlannedScript, ScriptRun};
use crate::enrich::{CollectedFacts, ENRICH_TITLE, FACTS_VARIABLE};
use crate::logging::PlainText;
use crate::process;
use crate::python::quiet_command;
use crate::workspace::HostId;

/// The host id a script job reports its output and state under (in
/// [`JobEvent`]s, the report and kept runs). No host has it: host ids end
/// with their file's `.yaml`/`.yml`.
pub const SCRIPT_OUTPUT: &str = "script";
/// Names the file listing the job's hosts ([`ScriptTargets`]).
pub const TARGETS_VARIABLE: &str = "NETTERM_JOB_TARGETS";
/// Names the workspace folder the script runs for.
pub const WORKSPACE_VARIABLE: &str = "NETTERM_WORKSPACE";
/// Names the headless binary, for the Python package.
pub const CLI_VARIABLE: &str = "NETTERM_CLI";
/// The version of the targets file's format.
pub const TARGETS_FORMAT: u32 = 1;

/// How a script job runs its script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOptions {
    /// The Python interpreter (the user's choice, checked by the caller).
    pub interpreter: PathBuf,
    /// The headless binary, set as [`CLI_VARIABLE`] (`None`: not set, the
    /// package looks for it itself).
    pub cli: Option<PathBuf>,
    /// More environment variables for the script (the app's data folder,
    /// say, so that the binary asks this app for credentials).
    pub environment: Vec<(OsString, OsString)>,
}

/// The hosts a script job may see: the file [`TARGETS_VARIABLE`] names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptTargets {
    /// [`TARGETS_FORMAT`].
    pub format: u32,
    /// The workspace folder the job runs in.
    pub workspace: PathBuf,
    /// The job's hosts, in plan order.
    pub hosts: Vec<HostId>,
    /// For filling in from devices: the NAPALM driver of each host.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub napalm_drivers: BTreeMap<HostId, String>,
}

/// Why the targets file cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum TargetsError {
    #[error("the job's targets file {} cannot be read: {error}", .path.display())]
    Read { path: PathBuf, error: io::Error },
    #[error("the job's targets file {} is damaged: {reason}", .path.display())]
    Damaged { path: PathBuf, reason: String },
}

impl ScriptTargets {
    /// The targets the environment names, if it names a file: a process
    /// started by a script job. A file that cannot be read is an error, so
    /// that a script is never shown more than its job's hosts by mistake.
    pub fn from_environment() -> Option<Result<Self, TargetsError>> {
        let path = std::env::var_os(TARGETS_VARIABLE).filter(|path| !path.is_empty())?;
        Some(Self::read(Path::new(&path)))
    }

    pub fn read(path: &Path) -> Result<Self, TargetsError> {
        let text = fs::read_to_string(path).map_err(|error| TargetsError::Read {
            path: path.to_path_buf(),
            error,
        })?;
        let damaged = |reason: String| TargetsError::Damaged {
            path: path.to_path_buf(),
            reason,
        };
        let targets: Self =
            serde_json::from_str(&text).map_err(|error| damaged(error.to_string()))?;
        if targets.format != TARGETS_FORMAT {
            return Err(damaged(format!(
                "it is in format {}, and this version reads format {TARGETS_FORMAT}",
                targets.format
            )));
        }
        Ok(targets)
    }

    /// Whether `id` is one of the job's hosts.
    pub fn allows(&self, id: &HostId) -> bool {
        self.hosts.contains(id)
    }
}

/// How many pieces of a script's output (8 KiB each at most) may wait to
/// be taken.
const PIECES_IN_FLIGHT: usize = 64;

/// How the process ended.
enum Ended {
    Exited(ExitStatus),
    Cancelled,
}

/// Runs the planned script on its own thread, reporting on `events`.
pub(super) fn spawn(
    plan: &JobPlan,
    script: &PlannedScript,
    options: &ScriptOptions,
    output_limit: usize,
    events: mpsc::UnboundedSender<JobEvent>,
    cancel: watch::Receiver<bool>,
) {
    let name = match &plan.step {
        JobStep::Script { path } => path.clone(),
        JobStep::Enrich => ENRICH_TITLE.to_string(),
        _ => unreachable!("a script is planned only for a script step"),
    };
    let runner = Runner {
        id: HostId::new(SCRIPT_OUTPUT),
        name,
        facts: (plan.step == JobStep::Enrich).then(|| {
            std::env::temp_dir().join(format!("netterm-facts-{}.json", uuid::Uuid::new_v4()))
        }),
        events,
        text: PlainText::new(),
        output: KeptOutput::new(output_limit),
    };
    let targets = ScriptTargets {
        format: TARGETS_FORMAT,
        workspace: script.root.clone(),
        hosts: plan.hosts.iter().map(|host| host.id.clone()).collect(),
        napalm_drivers: plan
            .hosts
            .iter()
            .filter_map(|host| {
                let driver = host.work.napalm_driver?;
                Some((host.id.clone(), driver.to_string()))
            })
            .collect(),
    };
    let script = script.clone();
    let options = options.clone();
    let _ = runner.events.send(JobEvent::State {
        host: runner.id.clone(),
        state: HostState::Waiting,
    });
    thread::Builder::new()
        .name("script job".into())
        .spawn(move || runner.run(&script, &options, &targets, &cancel))
        .expect("a thread starts");
}

struct Runner {
    id: HostId,
    /// The script's path, as the step names it.
    name: String,
    /// For filling in from devices: the file the script writes the facts
    /// to.
    facts: Option<PathBuf>,
    events: mpsc::UnboundedSender<JobEvent>,
    /// The text of stdout and stderr, which share one pipe, so their
    /// lines come in the order the script wrote them (as in a terminal).
    text: PlainText,
    /// Up to the output limit: a flood is not held whole.
    output: KeptOutput,
}

impl Runner {
    fn run(
        mut self,
        script: &PlannedScript,
        options: &ScriptOptions,
        targets: &ScriptTargets,
        cancel: &watch::Receiver<bool>,
    ) {
        let file = std::env::temp_dir().join(format!("netterm-job-{}.json", uuid::Uuid::new_v4()));
        let (state, exit_status) = match self.execute(script, options, targets, &file, cancel) {
            Ok(Ended::Exited(status)) => (exited(status), status.code()),
            Ok(Ended::Cancelled) => (HostState::Cancelled, None),
            Err(reason) => (HostState::Failed { reason }, None),
        };
        let _ = fs::remove_file(&file);
        let rest = self.text.finish();
        self.add_output(rest);
        let facts = self.facts.take().map(|path| {
            let missing = match &state {
                HostState::Cancelled => "the job was cancelled before the script reported it",
                _ => "the script reported nothing for it (see its output)",
            };
            let facts = CollectedFacts::read(&path, &targets.hosts, missing);
            let _ = fs::remove_file(&path);
            facts
        });
        self.state(state.clone());
        let report = JobReport {
            hosts: vec![HostResult {
                id: self.id.clone(),
                name: self.name.clone(),
                state,
                output: std::mem::take(&mut self.output.text),
                backup: None,
                exit_status,
                facts,
                bytes: None,
            }],
            file: None,
        };
        let _ = self.events.send(JobEvent::Finished(report));
    }

    /// Writes the targets file, starts the script and follows it until it
    /// ends or the job is cancelled.
    fn execute(
        &mut self,
        script: &PlannedScript,
        options: &ScriptOptions,
        targets: &ScriptTargets,
        file: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Ended, String> {
        let json = serde_json::to_string_pretty(targets).expect("targets serialise");
        fs::write(file, json).map_err(|error| {
            format!(
                "the job's hosts could not be written for the script to {}: {error}",
                file.display()
            )
        })?;
        if *cancel.borrow() {
            return Ok(Ended::Cancelled);
        }
        let mut command = quiet_command(options.interpreter.as_os_str());
        command.arg("-u");
        match &script.run {
            ScriptRun::File(file) => command.arg(file),
            ScriptRun::Module(module) => command.arg("-m").arg(module),
        };
        if let Some(facts) = &self.facts {
            command.env(FACTS_VARIABLE, facts);
        }
        command
            .current_dir(&script.root)
            .stdin(Stdio::null())
            .env("PYTHONUNBUFFERED", "1")
            .env(TARGETS_VARIABLE, file)
            .env(WORKSPACE_VARIABLE, &script.root);
        if let Some(cli) = &options.cli {
            command.env(CLI_VARIABLE, cli);
        }
        for (name, value) in &options.environment {
            command.env(name, value);
        }
        let not_started = |error: io::Error| {
            format!(
                "the script could not be started with {}: {error}",
                options.interpreter.display()
            )
        };
        let (output, writer) = io::pipe().map_err(not_started)?;
        command
            .stdout(writer.try_clone().map_err(not_started)?)
            .stderr(writer);
        // The script and what it starts end when it is cancelled, when it
        // has ended (what it left behind), and when this process ends,
        // however it ends (`crate::process`).
        let started = process::start(&mut command);
        // The command holds this process's copies of the pipe's writing
        // end: gone, the pipe ends once the script (and what it started)
        // closed theirs.
        drop(command);
        let mut tree = started.map_err(not_started)?;
        self.state(HostState::Running);

        // Bounded: a script writing faster than its output is taken waits
        // on its pipe, rather than its output piling up here.
        let (sender, pieces) = std_mpsc::sync_channel(PIECES_IN_FLIGHT);
        read_pipe(output, sender);

        loop {
            match pieces.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) => self.take(&bytes),
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                // The pipe closed; the process may still run.
                Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(50));
                }
            }
            if *cancel.borrow() {
                tree.stop();
                self.drain(&pieces, Duration::from_millis(200));
                return Ok(Ended::Cancelled);
            }
            match tree.child.try_wait() {
                Ok(Some(status)) => {
                    // What is still in the pipes, for a moment (a process
                    // it left behind may hold them open).
                    self.drain(&pieces, Duration::from_millis(500));
                    return Ok(Ended::Exited(status));
                }
                Ok(None) => {}
                Err(error) => {
                    tree.stop();
                    return Err(format!("the script could not be followed: {error}"));
                }
            }
        }
    }

    /// Takes what is left in the pipe until it has ended (its reader
    /// gone) or `wait` has passed.
    fn drain(&mut self, pieces: &std_mpsc::Receiver<Vec<u8>>, wait: Duration) {
        let until = Instant::now() + wait;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match pieces.recv_timeout(left) {
                Ok(bytes) => self.take(&bytes),
                Err(_) => break,
            }
        }
    }

    fn take(&mut self, bytes: &[u8]) {
        let lines = self.text.push(bytes);
        self.add_output(lines);
    }

    fn add_output(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let Some(text) = self.output.add(text) else {
            return;
        };
        let _ = self.events.send(JobEvent::Output {
            host: self.id.clone(),
            text,
        });
    }

    fn state(&self, state: HostState) {
        let _ = self.events.send(JobEvent::State {
            host: self.id.clone(),
            state,
        });
    }
}

/// How a script that exited with `status` ended.
fn exited(status: ExitStatus) -> HostState {
    match status.code() {
        Some(0) => HostState::Done,
        Some(code) => HostState::Failed {
            reason: format!("the script exited with status {code}"),
        },
        None => HostState::Failed {
            reason: "the script was ended without an exit status (by a signal)".into(),
        },
    }
}

fn read_pipe(mut stream: impl Read + Send + 'static, sender: std_mpsc::SyncSender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buffer = vec![0; 8192];
        // Returning drops the sender: the receiver then sees the end.
        while let Ok(read @ 1..) = stream.read(&mut buffer) {
            if sender.send(buffer[..read].to_vec()).is_err() {
                return;
            }
        }
    });
}
