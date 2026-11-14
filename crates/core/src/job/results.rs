//! Job results: every run kept on disk until the user deletes it.
//!
//! A [`RunStore`] lives under the app's data area, never in a workspace,
//! and keeps each workspace's runs apart by its id:
//! `<root>/<workspace id>/<run id>/`, the run id being its local start time
//! (`YYYY-MM-DD_HH-MM-SS`, `_2`... when several start in one second). A run
//! folder holds its description, [`RUN_FILE`] (step, target, start and end,
//! each host's state and reason, the skipped hosts), and one plain-text
//! file per host under [`OUTPUT_FOLDER`], at the host's tree path with
//! `.txt` for its extension. [`RunStore::begin`] writes them before the job
//! starts and the [`RunRecorder`] it returns keeps them up to date from the
//! job's events, so a run interrupted by the app closing is still listed
//! (without an end). Nothing is pruned and nothing is encrypted: device
//! output can hold secrets the device shows.
//!
//! A script run has one output instead of one per host: its `hosts` hold
//! the script alone (id [`super::SCRIPT_OUTPUT`], its output in
//! `output/script.txt`, its exit status), and its `targets` the hosts it
//! ran for.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use super::{HostState, JobEvent, JobPlan, JobStep, JobTarget, SCRIPT_OUTPUT};
use crate::configs::SaveOutcome;
use crate::diff::{diff_lines, DiffLine};
use crate::enrich::{CollectedFacts, ENRICH_TITLE};
use crate::workspace::HostId;

/// The description file of a run.
pub const RUN_FILE: &str = "run.yaml";
/// The folder of a run holding the hosts' output.
pub const OUTPUT_FOLDER: &str = "output";
/// How many matching lines a search shows per host (it counts them all).
pub const MAX_FOUND_LINES: usize = 100;

/// A kept run, as its description says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRun {
    /// The run's id: its folder's name.
    #[serde(default)]
    pub id: String,
    /// The id of the workspace it ran in.
    pub workspace: String,
    pub target: JobTarget,
    pub step: JobStep,
    /// When it started, RFC 3339 in local time.
    pub started: String,
    /// When it ended; `None` while it runs, or if the app closed first.
    #[serde(default)]
    pub ended: Option<String>,
    /// The hosts that ran, in plan order.
    pub hosts: Vec<RunHost>,
    /// The hosts pre-flight skipped.
    #[serde(default)]
    pub skipped: Vec<RunSkipped>,
    /// For a script run: the hosts it ran for (its `hosts` holding the
    /// script's one output).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<RunTarget>,
    /// For filling in from devices, once it ended: what the script
    /// reported for each target (see [`crate::enrich`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<CollectedFacts>,
}

/// A host a script run ran for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTarget {
    pub id: HostId,
    pub name: String,
    pub address: String,
}

impl StoredRun {
    /// Whether the run got to its end.
    pub fn ended(&self) -> bool {
        self.ended.is_some()
    }

    /// The host `id` of the run.
    pub fn host(&self, id: &HostId) -> Option<&RunHost> {
        self.hosts.iter().find(|host| &host.id == id)
    }
}

/// A host of a kept run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunHost {
    pub id: HostId,
    pub name: String,
    pub address: String,
    /// Its last state: final once the run has ended.
    pub state: HostState,
    /// Its output file, relative to the run folder, `/`-separated.
    pub file: String,
    /// For a config backup: the version that holds its configuration
    /// (saved by this run, or found unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<SaveOutcome>,
    /// For a script: its exit status, once it exited with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i32>,
}

/// A host pre-flight skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSkipped {
    pub id: HostId,
    pub name: String,
    pub reason: String,
}

/// A workspace's kept runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunList {
    /// Newest first.
    pub runs: Vec<StoredRun>,
    /// Run folders whose description cannot be read, by id.
    pub damaged: Vec<DamagedRun>,
}

/// A run folder whose description cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DamagedRun {
    pub id: String,
    pub reason: String,
}

/// The hosts of a run whose output holds a text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub host: HostId,
    pub name: String,
    /// How many of its lines hold the text.
    pub count: usize,
    /// The first [`MAX_FOUND_LINES`] of them.
    pub lines: Vec<FoundLine>,
}

/// A line that holds the text searched for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundLine {
    /// 1-based.
    pub number: usize,
    pub text: String,
}

/// One host's output in a kept run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRef {
    pub run: String,
    pub host: HostId,
}

/// Why kept results cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ResultsError {
    /// A workspace id that cannot name a folder.
    #[error("job results cannot be kept for the workspace id {0:?}")]
    BadWorkspaceId(String),
    #[error("there is no job run {0}")]
    NoSuchRun(String),
    #[error("the job run {run} has no host {host}")]
    NoSuchHost { run: String, host: HostId },
    #[error("the job run {run} cannot be read: {reason}")]
    Damaged { run: String, reason: String },
    #[error("{0} is not a folder")]
    NotAFolder(PathBuf),
    /// The run is still being kept by its job: cancel it first.
    #[error("the job run {0} is still running: cancel it first")]
    Running(String),
    #[error("{}: {error}", .path.display())]
    Io { path: PathBuf, error: io::Error },
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ResultsError + '_ {
    move |error| ResultsError::Io {
        path: path.to_path_buf(),
        error,
    }
}

/// Where job runs are kept. Clones share which runs are being recorded
/// (running), so that those are never deleted or exported half written.
#[derive(Debug, Clone)]
pub struct RunStore {
    root: PathBuf,
    active: Active,
}

/// The runs being recorded: (workspace id, run id).
type Active = Arc<Mutex<BTreeSet<(String, String)>>>;

fn held(active: &Active) -> MutexGuard<'_, BTreeSet<(String, String)>> {
    active.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RunStore {
    /// A store in `root` (created when the first run is kept).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            active: Active::default(),
        }
    }

    /// The runs of `workspace` still being recorded (their jobs run).
    pub fn running(&self, workspace: &str) -> Vec<String> {
        held(&self.active)
            .iter()
            .filter(|(ws, _)| ws == workspace)
            .map(|(_, run)| run.clone())
            .collect()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Starts keeping a run of `plan` (made for `target`) in the workspace
    /// `workspace`: writes its folder, its description and an empty output
    /// file per host. Feed the job's events to the recorder.
    pub fn begin(
        &self,
        workspace: &str,
        target: &JobTarget,
        plan: &JobPlan,
    ) -> Result<RunRecorder, ResultsError> {
        let parent = self.workspace_folder(workspace)?;
        fs::create_dir_all(&parent).map_err(io_error(&parent))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        // Held until the run is marked active, so that no delete comes
        // between.
        let mut active = held(&self.active);
        // Another run may have started in the same second.
        let (id, folder) = create_unused(&parent, &stamp).map_err(io_error(&parent))?;
        active.insert((workspace.to_string(), id.clone()));
        drop(active);

        let run_host = |id: &HostId, name: &str, address: &str| RunHost {
            id: id.clone(),
            name: name.to_string(),
            address: address.to_string(),
            state: HostState::Waiting,
            file: output_file(id),
            backup: None,
            exit_status: None,
        };
        let (hosts, targets) = match &plan.step {
            step @ (JobStep::Script { .. } | JobStep::Enrich) => (
                vec![run_host(
                    &HostId::new(SCRIPT_OUTPUT),
                    match step {
                        JobStep::Script { path } => path,
                        _ => ENRICH_TITLE,
                    },
                    "",
                )],
                plan.hosts
                    .iter()
                    .map(|host| RunTarget {
                        id: host.id.clone(),
                        name: host.name.clone(),
                        address: host.address.clone(),
                    })
                    .collect(),
            ),
            _ => (
                plan.hosts
                    .iter()
                    .map(|host| run_host(&host.id, &host.name, &host.address))
                    .collect(),
                Vec::new(),
            ),
        };
        let run = StoredRun {
            id,
            workspace: workspace.to_string(),
            target: target.clone(),
            step: plan.step.clone(),
            started: now(),
            ended: None,
            hosts,
            skipped: plan
                .skipped
                .iter()
                .map(|host| RunSkipped {
                    id: host.id.clone(),
                    name: host.name.clone(),
                    reason: host.message.clone(),
                })
                .collect(),
            targets,
            facts: None,
        };
        let recorder = RunRecorder {
            folder,
            run,
            active: Some(self.active.clone()),
        };
        for host in &recorder.run.hosts {
            let path = recorder.folder.join(&host.file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io_error(parent))?;
            }
            fs::write(&path, "").map_err(io_error(&path))?;
        }
        recorder.save()?;
        Ok(recorder)
    }

    /// The runs kept for `workspace`, newest first.
    pub fn list(&self, workspace: &str) -> Result<RunList, ResultsError> {
        let parent = self.workspace_folder(workspace)?;
        let mut list = RunList {
            runs: Vec::new(),
            damaged: Vec::new(),
        };
        let entries = match fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(list),
            Err(error) => return Err(io_error(&parent)(error)),
        };
        for entry in entries {
            let entry = entry.map_err(io_error(&parent))?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            match read_run(&entry.path(), &id) {
                Ok(run) => list.runs.push(run),
                Err(error) => list.damaged.push(DamagedRun {
                    reason: match error {
                        ResultsError::Damaged { reason, .. } => reason,
                        other => other.to_string(),
                    },
                    id,
                }),
            }
        }
        list.runs.sort_by(|a, b| {
            let started = |run: &StoredRun| chrono::DateTime::parse_from_rfc3339(&run.started).ok();
            started(b)
                .cmp(&started(a))
                .then_with(|| natural(&b.id).cmp(&natural(&a.id)))
        });
        list.damaged.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(list)
    }

    /// The run `run` of `workspace`.
    pub fn run(&self, workspace: &str, run: &str) -> Result<StoredRun, ResultsError> {
        read_run(&self.run_folder(workspace, run)?, run)
    }

    /// One host's output in a run of `workspace`.
    pub fn output(&self, workspace: &str, output: &OutputRef) -> Result<String, ResultsError> {
        let folder = self.run_folder(workspace, &output.run)?;
        let run = read_run(&folder, &output.run)?;
        let host = run
            .host(&output.host)
            .ok_or_else(|| ResultsError::NoSuchHost {
                run: output.run.clone(),
                host: output.host.clone(),
            })?;
        read_output(&folder, &run, host)
    }

    /// The hosts of a run whose output holds `text` (ignoring case), in
    /// run order, with the lines that hold it. Nothing for a blank text.
    pub fn search(
        &self,
        workspace: &str,
        run: &str,
        text: &str,
    ) -> Result<Vec<SearchHit>, ResultsError> {
        let folder = self.run_folder(workspace, run)?;
        let stored = read_run(&folder, run)?;
        let wanted = text.trim().to_lowercase();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = Vec::new();
        for host in &stored.hosts {
            let output = read_output(&folder, &stored, host)?;
            let mut hit = SearchHit {
                host: host.id.clone(),
                name: host.name.clone(),
                count: 0,
                lines: Vec::new(),
            };
            for (index, line) in crate::diff::lines(&output).into_iter().enumerate() {
                if line.to_lowercase().contains(&wanted) {
                    hit.count += 1;
                    if hit.lines.len() < MAX_FOUND_LINES {
                        hit.lines.push(FoundLine {
                            number: index + 1,
                            text: line.to_string(),
                        });
                    }
                }
            }
            if hit.count > 0 {
                hits.push(hit);
            }
        }
        Ok(hits)
    }

    /// A line diff of two hosts' output: two hosts of one run, or one host
    /// in two runs (`old` first).
    pub fn compare(
        &self,
        workspace: &str,
        old: &OutputRef,
        new: &OutputRef,
    ) -> Result<Vec<DiffLine>, ResultsError> {
        Ok(diff_lines(
            &self.output(workspace, old)?,
            &self.output(workspace, new)?,
        ))
    }

    /// Copies the run `run` (its description and output files) into the
    /// folder `to`, as a folder named after the run (`_2`... if taken);
    /// the copy's path.
    pub fn export(&self, workspace: &str, run: &str, to: &Path) -> Result<PathBuf, ResultsError> {
        let folder = self.run_folder(workspace, run)?;
        let active = held(&self.active);
        self.refuse_running(&active, workspace, run)?;
        read_run(&folder, run)?;
        if !to.is_dir() {
            return Err(ResultsError::NotAFolder(to.to_path_buf()));
        }
        let (_, copy) = create_unused(to, run).map_err(io_error(to))?;
        copy_folder(&folder, &copy)?;
        Ok(copy)
    }

    /// Deletes the run `run` from disk, output and all; refused while it
    /// runs.
    pub fn delete(&self, workspace: &str, run: &str) -> Result<(), ResultsError> {
        let folder = self.run_folder(workspace, run)?;
        let active = held(&self.active);
        self.refuse_running(&active, workspace, run)?;
        fs::remove_dir_all(&folder).map_err(io_error(&folder))
    }

    fn refuse_running(
        &self,
        active: &BTreeSet<(String, String)>,
        workspace: &str,
        run: &str,
    ) -> Result<(), ResultsError> {
        if active.contains(&(workspace.to_string(), run.to_string())) {
            return Err(ResultsError::Running(run.to_string()));
        }
        Ok(())
    }

    /// The folder of `workspace`'s runs.
    fn workspace_folder(&self, workspace: &str) -> Result<PathBuf, ResultsError> {
        if !is_plain_name(workspace) {
            return Err(ResultsError::BadWorkspaceId(workspace.to_string()));
        }
        Ok(self.root.join(workspace))
    }

    /// The folder of an existing run.
    fn run_folder(&self, workspace: &str, run: &str) -> Result<PathBuf, ResultsError> {
        let folder = self.workspace_folder(workspace)?.join(run);
        if !is_plain_name(run) || !folder.join(RUN_FILE).is_file() {
            return Err(ResultsError::NoSuchRun(run.to_string()));
        }
        Ok(folder)
    }
}

/// Keeps a run's files up to date while its job runs. The run counts as
/// running until the job's report is recorded or the recorder is dropped.
#[derive(Debug)]
pub struct RunRecorder {
    folder: PathBuf,
    run: StoredRun,
    /// The store's running runs, until this one has ended.
    active: Option<Active>,
}

impl Drop for RunRecorder {
    fn drop(&mut self) {
        self.release();
    }
}

impl RunRecorder {
    /// The run's id.
    pub fn id(&self) -> &str {
        &self.run.id
    }

    /// The run as kept so far.
    pub fn run(&self) -> &StoredRun {
        &self.run
    }

    /// Keeps what `event` says: a host's state in the description, its
    /// output in its file; at the end, every host's final state and whole
    /// output, and when the run ended.
    pub fn record(&mut self, event: &JobEvent) -> Result<(), ResultsError> {
        match event {
            JobEvent::State { host, state } => {
                if let Some(entry) = self.run.hosts.iter_mut().find(|h| &h.id == host) {
                    entry.state = state.clone();
                }
                self.save()
            }
            JobEvent::Backup { host, outcome } => {
                if let Some(entry) = self.run.hosts.iter_mut().find(|h| &h.id == host) {
                    entry.backup = Some(outcome.clone());
                }
                self.save()
            }
            JobEvent::Output { host, text } => {
                let Some(entry) = self.run.host(host) else {
                    return Ok(());
                };
                let path = self.folder.join(&entry.file);
                OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&path)
                    .and_then(|mut file| file.write_all(text.as_bytes()))
                    .map_err(io_error(&path))
            }
            JobEvent::Finished(report) => {
                let mut first_error = None;
                for result in &report.hosts {
                    let Some(entry) = self.run.hosts.iter_mut().find(|h| h.id == result.id) else {
                        continue;
                    };
                    entry.state = result.state.clone();
                    entry.backup.clone_from(&result.backup);
                    entry.exit_status = result.exit_status;
                    if result.facts.is_some() {
                        self.run.facts.clone_from(&result.facts);
                    }
                    // The whole output again, in case a piece was not kept.
                    let path = self.folder.join(&entry.file);
                    if let Err(error) = fs::write(&path, &result.output) {
                        first_error.get_or_insert(io_error(&path)(error));
                    }
                }
                self.run.ended = Some(now());
                let saved = self.save();
                self.release();
                match first_error {
                    Some(error) => Err(error),
                    None => saved,
                }
            }
        }
    }

    /// No longer running.
    fn release(&mut self) {
        if let Some(active) = self.active.take() {
            held(&active).remove(&(self.run.workspace.clone(), self.run.id.clone()));
        }
    }

    /// Writes the description (whole, so that it is never read half
    /// written).
    fn save(&self) -> Result<(), ResultsError> {
        let text = serde_norway::to_string(&self.run).map_err(|error| ResultsError::Damaged {
            run: self.run.id.clone(),
            reason: error.to_string(),
        })?;
        let path = self.folder.join(RUN_FILE);
        let temporary = self.folder.join(format!(".{RUN_FILE}.tmp"));
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, &path))
            .map_err(io_error(&path))
    }
}

/// The run described in `folder`, called `id`.
fn read_run(folder: &Path, id: &str) -> Result<StoredRun, ResultsError> {
    let path = folder.join(RUN_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ResultsError::Damaged {
                run: id.to_string(),
                reason: format!("it has no {RUN_FILE}"),
            })
        }
        Err(error) => return Err(io_error(&path)(error)),
    };
    let mut run: StoredRun =
        serde_norway::from_str(&text).map_err(|error| ResultsError::Damaged {
            run: id.to_string(),
            reason: error.to_string(),
        })?;
    run.id = id.to_string();
    Ok(run)
}

/// The output of `host` in `run`, kept in `folder`.
fn read_output(folder: &Path, run: &StoredRun, host: &RunHost) -> Result<String, ResultsError> {
    let relative = Path::new(&host.file);
    if !relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(ResultsError::Damaged {
            run: run.id.clone(),
            reason: format!("the output file of {} is outside the run", host.id),
        });
    }
    let path = folder.join(relative);
    match fs::read(&path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        // Not written yet, or lost: nothing collected.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(io_error(&path)(error)),
    }
}

/// Where the output of host `id` goes in its run folder:
/// `output/<tree path>/<file stem>.txt`.
fn output_file(id: &HostId) -> String {
    let path = id.as_str();
    let (parents, file) = path.rsplit_once('/').unwrap_or(("", path));
    let stem = Path::new(file)
        .file_stem()
        .map_or_else(|| file.to_string(), |s| s.to_string_lossy().into_owned());
    let mut out = String::from(OUTPUT_FOLDER);
    for part in parents.split('/').filter(|part| !part.is_empty()) {
        out.push('/');
        out.push_str(part);
    }
    out.push('/');
    out.push_str(&stem);
    out.push_str(".txt");
    out
}

/// Creates the folder `<parent>/<base>`, or `<base>_2`, `_3`... if taken;
/// its name and path.
fn create_unused(parent: &Path, base: &str) -> io::Result<(String, PathBuf)> {
    for attempt in 1.. {
        let name = match attempt {
            1 => base.to_string(),
            n => format!("{base}_{n}"),
        };
        let folder = parent.join(&name);
        match fs::create_dir(&folder) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            result => return result.map(|()| (name, folder)),
        }
    }
    unreachable!("an unused name is found")
}

/// Whether `name` is one plain folder name (no separators, not `.`/`..`).
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
}

/// A run id's sort key, so that `_10` comes after `_9`.
fn natural(id: &str) -> (&str, u32) {
    match id.rsplit_once('_') {
        Some((stamp, n)) if stamp.len() >= 19 => (stamp, n.parse().unwrap_or(0)),
        _ => (id, 1),
    }
}

fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

fn copy_folder(from: &Path, to: &Path) -> Result<(), ResultsError> {
    for entry in fs::read_dir(from).map_err(io_error(from))? {
        let entry = entry.map_err(io_error(from))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        if source.is_dir() {
            fs::create_dir(&target).map_err(io_error(&target))?;
            copy_folder(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(io_error(&target))?;
        }
    }
    Ok(())
}
