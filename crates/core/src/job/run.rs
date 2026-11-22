//! Running a planned job: one headless session per host, at most
//! [`JobOptions::parallel`] at once.

use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::Instant;

use super::script::{self, ScriptOptions};
use super::{file, BackupWork, EnableSecret, JobPlan, JobStep, PlannedHost};
use crate::configs::{ConfigStore, SaveOutcome};
use crate::enrich::CollectedFacts;
use crate::files::Hashes;
use crate::logging::PlainText;
use crate::session::{
    CloseReason, HostKeyDecision, HostKeyQuestion, LoginPrompt, OpenError, SessionEvent,
    SessionEvents, SessionId, SessionManager, SessionState, SessionTarget, SshTarget, TerminalSize,
};
use crate::workspace::HostId;

/// How many hosts run at once when nothing says otherwise.
pub const DEFAULT_PARALLEL: usize = 10;
/// The most hosts that may run at once.
pub const MAX_PARALLEL: usize = 50;
/// How long a host may stay quiet without showing its prompt, after a line
/// is sent (or after logging in), when nothing says otherwise: counted
/// from the last output, so a command still printing is waited for.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest one command (or the wait after logging in) may go on
/// showing output without getting back to the prompt, when nothing says
/// otherwise: it ends output that never stops.
pub const DEFAULT_COMMAND_LIMIT: Duration = Duration::from_secs(30 * 60);
/// How long output must stay quiet at the prompt for it to count, when the
/// prompt is the very line the command was typed at (or the first prompt
/// after logging in, a line the platform's pattern matches whole), so that
/// output merely ending in it for a moment is not taken for it (the
/// containerlab suite's rule).
pub const PROMPT_SETTLE: Duration = Duration::from_millis(300);
/// How long output must stay quiet at a prompt that changed for it to
/// count, after a line expected to change it (the privilege command, a line
/// that enters or leaves a configuration mode or renames the device, any
/// line typed in such a mode). Also the first prompt after logging in, when
/// the platform's pattern matches only the end of its line.
pub const PROMPT_CHANGE_SETTLE: Duration = Duration::from_secs(1);
/// How long output must stay quiet at a prompt that changed after a line
/// not expected to change it (a `show` command, the disable-paging
/// command) for it to count: such a line is far more likely the device's
/// answer paused half way than a new prompt, but a command the job does not
/// know to change the prompt still ends.
pub const PROMPT_DOUBT_SETTLE: Duration = Duration::from_secs(3);
/// How much of one host's (or one script's) output a job keeps when
/// nothing says otherwise: 32 MiB of plain text. Past it the output is
/// still read (the device or script goes on), but no longer kept, shown or
/// written to the kept run; a line says so ([`output_cut`]).
pub const DEFAULT_OUTPUT_LIMIT: usize = 32 * 1024 * 1024;
/// The terminal size job sessions ask for: wide, so that devices wrap as
/// few lines as possible.
pub const JOB_TERMINAL_SIZE: TerminalSize = TerminalSize {
    cols: 400,
    rows: 100,
};

/// How a job runs.
#[derive(Debug, Clone)]
pub struct JobOptions {
    /// How many hosts run at once (each with its one connection open), 1
    /// to [`MAX_PARALLEL`]; [`start`] refuses other values.
    pub parallel: usize,
    /// How long a host may show nothing more without its prompt after each
    /// line (counted from its last output); a host quiet for longer fails.
    pub command_timeout: Duration,
    /// How long a host may go on showing output after each line without
    /// getting back to its prompt; a host that does fails.
    pub command_limit: Duration,
    /// How many bytes of plain text of each host's output (a script's
    /// output) are kept: in its result, its events and its kept run.
    pub output_limit: usize,
    /// Where a config backup keeps the configurations it fetches; a config
    /// backup cannot start without it.
    pub configs: Option<ConfigStore>,
    /// How a script step runs its script (the interpreter, the headless
    /// binary); a script job cannot start without it.
    pub script: Option<ScriptOptions>,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            parallel: DEFAULT_PARALLEL,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            command_limit: DEFAULT_COMMAND_LIMIT,
            output_limit: DEFAULT_OUTPUT_LIMIT,
            configs: None,
            script: None,
        }
    }
}

/// Where a host of a running job has got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HostState {
    /// Not started yet: other hosts take all the room.
    Waiting,
    /// Its session is being opened.
    Connecting,
    /// Logged in; the snippet is being sent.
    Running,
    /// Every line was sent and answered with the prompt (for a file job:
    /// the file was sent).
    Done,
    /// A file job without overwrite, and the host already has a file of
    /// that name and size: nothing was sent.
    AlreadyThere,
    /// It could not be reached, logged in to, or driven: `reason` says why.
    Failed { reason: String },
    /// The job was cancelled before the host finished.
    Cancelled,
}

impl HostState {
    /// Whether the host has finished, one way or another.
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            Self::Done | Self::AlreadyThere | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// Something a running job reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JobEvent {
    /// A host's state changed. Every host starts [`HostState::Waiting`].
    State { host: HostId, state: HostState },
    /// More of a host's output, as plain text (control sequences dropped;
    /// complete lines, except the last piece of a host's output).
    Output { host: HostId, text: String },
    /// A config backup kept (or found unchanged) the host's configuration;
    /// the host's [`HostState::Done`] follows.
    Backup { host: HostId, outcome: SaveOutcome },
    /// A file job's host has sent `done` bytes of `total` (`None` while
    /// not known). Sent at most every [`PROGRESS_EVERY`], and not kept
    /// with the run.
    Progress {
        host: HostId,
        done: u64,
        total: Option<u64>,
    },
    /// The job has ended: every host's final state and output. The last
    /// event.
    Finished(JobReport),
}

/// How often a file job reports a host's progress at most.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// What a job did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobReport {
    /// Every host that ran, in plan order.
    pub hosts: Vec<HostResult>,
    /// For a file job: the file sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<SentFile>,
}

/// The file a file job sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentFile {
    pub name: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its MD5 and SHA-256, once the job has read it (`None` if it could
    /// not be read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<Hashes>,
}

/// What a job did on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostResult {
    pub id: HostId,
    pub name: String,
    /// Final: done, failed or cancelled.
    pub state: HostState,
    /// Everything the host's session showed, as plain text.
    pub output: String,
    /// For a config backup that got the configuration: what was kept.
    pub backup: Option<SaveOutcome>,
    /// For a script step: the script's exit status, if it exited with one
    /// (not when it could not start, or was ended by a signal).
    pub exit_status: Option<i32>,
    /// For filling in from devices: what the script reported for each of
    /// the job's hosts.
    pub facts: Option<CollectedFacts>,
    /// For a file job: the bytes sent to the host (`None` when nothing
    /// went through).
    pub bytes: Option<u64>,
}

/// Why a plan cannot start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// The plan has no host to run.
    #[error("no host can run")]
    NothingToRun,
    /// The plan still lacks values for these variables.
    #[error("no value for {}", .0.join(", "))]
    MissingVariables(Vec<String>),
    /// [`JobOptions::parallel`] is not between 1 and [`MAX_PARALLEL`].
    #[error("between 1 and {MAX_PARALLEL} hosts can run at once, not {0}")]
    Parallel(usize),
    /// A config backup with no [`JobOptions::configs`].
    #[error("there is nowhere to keep the configurations")]
    NoConfigStore,
    /// A config backup planned in a workspace without an id, which keys
    /// the config history.
    #[error("the workspace has no id, which config backups are kept by")]
    NoWorkspaceId,
    /// A script step with no [`JobOptions::script`]: no usable Python.
    #[error("no Python interpreter is set up to run the script")]
    NoPython,
}

/// A running job: its events, and a way to cancel it. Dropping it does not
/// stop the job (its events are then lost); cancel it for that.
pub struct Job {
    events: mpsc::UnboundedReceiver<JobEvent>,
    cancel: JobCanceller,
}

impl Job {
    /// The next event; `None` after [`JobEvent::Finished`].
    pub async fn next(&mut self) -> Option<JobEvent> {
        self.events.recv().await
    }

    /// Cancels the job: see [`JobCanceller::cancel`].
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Something that cancels the job, to keep apart from its events.
    pub fn canceller(&self) -> JobCanceller {
        self.cancel.clone()
    }
}

/// Cancels a running job.
#[derive(Clone)]
pub struct JobCanceller(Arc<watch::Sender<bool>>);

impl JobCanceller {
    /// Closes every session of the job; hosts not finished yet end
    /// [`HostState::Cancelled`], keeping the output collected so far.
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
}

/// Why [`start`] would refuse `plan` with `options`, if it would.
pub fn check(plan: &JobPlan, options: &JobOptions) -> Result<(), StartError> {
    if !plan.missing_variables.is_empty() {
        return Err(StartError::MissingVariables(plan.missing_variables.clone()));
    }
    if !(1..=MAX_PARALLEL).contains(&options.parallel) {
        return Err(StartError::Parallel(options.parallel));
    }
    if plan.hosts.is_empty() {
        return Err(StartError::NothingToRun);
    }
    if plan.step == JobStep::ConfigBackup {
        if options.configs.is_none() {
            return Err(StartError::NoConfigStore);
        }
        if plan.workspace.is_none() {
            return Err(StartError::NoWorkspaceId);
        }
    }
    if plan.step.runs_script() && options.script.is_none() {
        return Err(StartError::NoPython);
    }
    Ok(())
}

/// Starts running `plan` (which must be [ready](JobPlan::ready)) with
/// sessions opened by `manager`; returns at once.
///
/// Must be called from within a Tokio runtime.
pub fn start(
    manager: &SessionManager,
    plan: JobPlan,
    options: JobOptions,
) -> Result<Job, StartError> {
    check(&plan, &options)?;
    let (events_tx, events) = mpsc::unbounded_channel();
    let (cancel_tx, cancel) = watch::channel(false);
    if let Some(script) = options.script.as_ref().filter(|_| plan.step.runs_script()) {
        let planned = plan
            .script
            .as_ref()
            .expect("plan keeps the script of a script step");
        script::spawn(
            &plan,
            planned,
            script,
            options.output_limit,
            events_tx,
            cancel,
        );
        return Ok(Job {
            events,
            cancel: JobCanceller(Arc::new(cancel_tx)),
        });
    }
    if let JobStep::SendFile { overwrite, .. } = plan.step {
        file::spawn(
            plan,
            overwrite,
            options.parallel,
            manager.known_hosts(),
            events_tx,
            cancel,
        );
        return Ok(Job {
            events,
            cancel: JobCanceller(Arc::new(cancel_tx)),
        });
    }
    let room = Arc::new(Semaphore::new(options.parallel));
    let configs = match (&options.configs, &plan.workspace) {
        (Some(store), Some(workspace)) => Some((store.clone(), workspace.clone())),
        _ => None,
    };

    for host in &plan.hosts {
        let _ = events_tx.send(JobEvent::State {
            host: host.id.clone(),
            state: HostState::Waiting,
        });
    }
    let tasks: Vec<_> = plan
        .hosts
        .into_iter()
        .map(|host| {
            let named = (host.id.clone(), host.name.clone());
            let runner = HostRunner::new(
                manager,
                events_tx.clone(),
                &host.id,
                options.clone(),
                configs.clone(),
            );
            (
                named,
                tokio::spawn(runner.run(host, room.clone(), cancel.clone())),
            )
        })
        .collect();
    tokio::spawn(async move {
        let mut hosts = Vec::with_capacity(tasks.len());
        for ((id, name), task) in tasks {
            // A host whose task failed still gets its final state.
            let result = task.await.unwrap_or_else(|error| {
                let state = failed(format!("the job stopped on this host: {error}"));
                let _ = events_tx.send(JobEvent::State {
                    host: id.clone(),
                    state: state.clone(),
                });
                HostResult {
                    id,
                    name,
                    state,
                    output: String::new(),
                    backup: None,
                    exit_status: None,
                    facts: None,
                    bytes: None,
                }
            });
            hosts.push(result);
        }
        let _ = events_tx.send(JobEvent::Finished(JobReport { hosts, file: None }));
    });
    Ok(Job {
        events,
        cancel: JobCanceller(Arc::new(cancel_tx)),
    })
}

/// Runs one host of a job.
struct HostRunner {
    manager: SessionManager,
    events: mpsc::UnboundedSender<JobEvent>,
    id: HostId,
    options: JobOptions,
    /// Where a config backup keeps what it fetches, and the workspace id.
    configs: Option<(ConfigStore, String)>,
    text: PlainText,
    /// The plain text so far, up to the output limit.
    output: KeptOutput,
    /// What a config backup kept.
    backup: Option<SaveOutcome>,
    /// The enable secret once it has been typed, never shown in the
    /// output (a device that echoes it would put it there).
    typed_secret: Option<EnableSecret>,
    /// The prompt line the device last came back to (trimmed): the one the
    /// next line is typed at. `None` before the first prompt.
    at: Option<String>,
    /// Whether the platform's pattern matched the whole first prompt line:
    /// then a changed prompt must be a whole line the pattern matches;
    /// otherwise (a pattern written for the end of the line only, such as
    /// `[>#]`) the pattern at the end of the line, as before #136.
    whole_lines: bool,
}

/// The host's session, once opened.
type Opened = Option<(SessionId, SessionEvents)>;

/// How a host's session is opened.
#[derive(Clone, Copy)]
enum Open {
    /// A connection of its own, to the host's profile target.
    Connect,
    /// A second channel on the connection of the session `of`; if the
    /// device refuses it, a connection of its own if the host has a
    /// target.
    Channel { of: SessionId },
}

/// How waiting for a prompt ended, if not with the prompt.
enum NoPrompt {
    /// Nothing more arrived for the command timeout.
    TimedOut,
    /// Output was still arriving when the command limit passed.
    Endless,
    Closed(CloseReason),
    /// The line could not be sent: the session had ended.
    NotSent,
}

/// The patterns a host is driven by.
struct Patterns {
    /// The prompt, at the end of the line being shown.
    prompt_end: Regex,
    /// The prompt as the whole line being shown (blanks around it aside).
    prompt_line: Regex,
    /// The privilege password question, at the end of the line.
    privilege_prompt: Option<Regex>,
}

/// What came back after something was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Prompt,
    PrivilegePrompt,
}

/// Which prompt a wait expects: what counts as the end of the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The first prompt after logging in: there is no line to compare with.
    First,
    /// The prompt the line was typed at; a changed one counts only after
    /// [`PROMPT_DOUBT_SETTLE`].
    Same,
    /// The prompt the line was typed at, or a changed one after
    /// [`PROMPT_CHANGE_SETTLE`]: the line is expected to change it.
    Change,
    /// Only the prompt the line was typed at (the show-config command,
    /// which never changes it: a configuration is never kept cut short).
    Only,
}

/// Whether `line`, typed at the prompt a snippet starts at, is expected to
/// change the prompt: its first word (a leading `/` aside, any case) is, or
/// abbreviates, a command that enters or leaves a mode, a shell or the
/// device, or renames it.
fn changes_prompt(line: &str) -> bool {
    /// Each command, and the shortest abbreviation taken for it.
    const COMMANDS: [(&str, usize); 24] = [
        ("configure", 4),
        ("enable", 2),
        ("disable", 4),
        ("end", 3),
        ("exit", 3),
        ("quit", 4),
        ("return", 6),
        ("edit", 4),
        ("enter", 5),
        ("commit", 6),
        ("prompt", 6),
        ("hostname", 8),
        ("system-view", 3),
        ("cli", 3),
        ("vtysh", 5),
        ("start", 5),
        ("shell", 5),
        ("bash", 4),
        ("tclsh", 5),
        ("ssh", 3),
        ("telnet", 6),
        ("su", 2),
        ("sudo", 4),
        ("cd", 2),
    ];
    let Some(word) = line.split_whitespace().next() else {
        return false;
    };
    let word = word.trim_start_matches('/').to_lowercase();
    COMMANDS
        .iter()
        .any(|(command, shortest)| word.len() >= *shortest && command.starts_with(&word))
}

/// How long a closed session may take to say so before it is let go.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Backs `host` up over a second channel on the connected session `of`
/// (or, if the device refuses one and the host has a target, a second
/// connection); keeps the configuration in `configs`' store under its
/// workspace id. Asks nothing, as a job; reports no events.
pub(super) async fn back_up_on_login(
    manager: &SessionManager,
    host: PlannedHost,
    of: SessionId,
    configs: (ConfigStore, String),
    command_timeout: Duration,
) -> HostResult {
    let options = JobOptions {
        command_timeout,
        ..JobOptions::default()
    };
    // Nobody follows its events: the outcome is the result.
    let (events, _) = mpsc::unbounded_channel();
    let mut runner = HostRunner::new(manager, events, &host.id, options, Some(configs));
    let mut session = None;
    let state = runner
        .drive(&host, &mut session, Open::Channel { of })
        .await;
    runner.finish(host, session, state).await
}

impl HostRunner {
    fn new(
        manager: &SessionManager,
        events: mpsc::UnboundedSender<JobEvent>,
        id: &HostId,
        options: JobOptions,
        configs: Option<(ConfigStore, String)>,
    ) -> Self {
        Self {
            manager: manager.clone(),
            events,
            id: id.clone(),
            output: KeptOutput::new(options.output_limit),
            options,
            configs,
            text: PlainText::new(),
            backup: None,
            typed_secret: None,
            at: None,
            whole_lines: false,
        }
    }

    async fn run(
        mut self,
        host: PlannedHost,
        room: Arc<Semaphore>,
        mut cancel: watch::Receiver<bool>,
    ) -> HostResult {
        let cancelled = async move {
            // An error means the job's handle is gone: run to the end.
            if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        // Held until the session has closed, so that no more than
        // `parallel` connections are ever open.
        let mut permit = None;
        let mut session: Opened = None;
        let state = tokio::select! {
            state = async {
                match room.acquire_owned().await {
                    Ok(room) => permit = Some(room),
                    Err(_) => return HostState::Cancelled,
                }
                self.drive(&host, &mut session, Open::Connect).await
            } => state,
            () = cancelled => HostState::Cancelled,
        };
        let result = self.finish(host, session, state).await;
        drop(permit);
        result
    }

    /// Closes the host's session, if it has one, and gives the result.
    async fn finish(
        mut self,
        host: PlannedHost,
        mut session: Opened,
        state: HostState,
    ) -> HostResult {
        if let Some((id, events)) = session.take() {
            self.close(id, events).await;
        }
        let rest = self.text.finish();
        self.add_output(rest);
        self.state(state.clone());
        HostResult {
            id: host.id,
            name: host.name,
            state,
            output: self.output.text,
            backup: self.backup,
            exit_status: None,
            facts: None,
            bytes: None,
        }
    }

    /// Closes the session `id` and waits (a little) until it has, keeping
    /// the output that still arrives; then forgets it.
    async fn close(&mut self, id: SessionId, mut events: SessionEvents) {
        if self.manager.close(id).is_ok() {
            let _ = tokio::time::timeout(CLOSE_WAIT, async {
                while let Some(event) = events.next().await {
                    match event {
                        SessionEvent::Output(bytes) => self.take(&events, &bytes),
                        SessionEvent::State(SessionState::Closed(_)) => break,
                        _ => {}
                    }
                }
            })
            .await;
        }
        self.manager.discard(id);
    }

    /// Connects, gets ready (privileged mode, no paging) and sends every
    /// line; the host's final state.
    async fn drive(&mut self, host: &PlannedHost, session: &mut Opened, open: Open) -> HostState {
        let work = &host.work;
        let patterns = match self.patterns(host) {
            Ok(patterns) => patterns,
            Err(reason) => return failed(reason),
        };
        self.state(HostState::Connecting);
        if let Err(reason) = self.open(host, session, open).await {
            return failed(reason);
        }
        let Some((id, events)) = session.as_mut() else {
            unreachable!("opened")
        };
        let id = *id;
        self.state(HostState::Running);

        match self.wait_for(events, &patterns, false, Expect::First).await {
            Ok(Shown::Prompt) => {}
            // A password question before the first prompt is the login's
            // (a second login in-band), never the privilege command's.
            Ok(Shown::PrivilegePrompt) => {
                return failed(format!(
                    "the device asked {:?} after logging in, before its first prompt; a job never answers login questions",
                    self.text.current_line().trim()
                ))
            }
            Err(error) => {
                return failed(match error {
                    NoPrompt::TimedOut => format!(
                        "no prompt after logging in: the device showed nothing more for {} (does the platform {}'s prompt pattern match?)",
                        span(self.options.command_timeout),
                        host.platform
                    ),
                    NoPrompt::Endless => format!(
                        "no prompt after logging in: it was still showing output after {} (does the platform {}'s prompt pattern match?)",
                        span(self.options.command_limit),
                        host.platform
                    ),
                    NoPrompt::Closed(reason) => {
                        format!("the session ended before the first prompt: {reason}")
                    }
                    NoPrompt::NotSent => unreachable!("nothing was sent"),
                })
            }
        }
        if let Some(command) = &work.privilege_command {
            if let Err(reason) = self
                .enter_privileged(id, events, host, command, &patterns)
                .await
            {
                return failed(reason);
            }
        }
        if let Some(command) = &work.disable_paging {
            match self
                .send(id, events, command, &patterns, Expect::Same)
                .await
            {
                Ok(Shown::Prompt) => {}
                Ok(Shown::PrivilegePrompt) => return failed(asked_for_password(command)),
                Err(error) => return failed(self.no_prompt_after(&quoted(command), error)),
            }
        }
        // The prompt the snippet starts at: a line typed at any other (in a
        // configuration mode, say) may change it.
        let start = self.at.clone();
        let count = work.lines.len();
        for (index, line) in work.lines.iter().enumerate() {
            let expect = if self.at != start || changes_prompt(line) {
                Expect::Change
            } else {
                Expect::Same
            };
            match self.send(id, events, line, &patterns, expect).await {
                Ok(Shown::Prompt) => {}
                // A job never types into a password question.
                Ok(Shown::PrivilegePrompt) => return failed(asked_for_password(line)),
                // A snippet may end by logging out.
                Err(NoPrompt::Closed(CloseReason::RemoteExited { .. })) if index + 1 == count => {
                    return HostState::Done
                }
                Err(error) => return failed(self.no_prompt_after(&quoted(line), error)),
            }
        }
        if let Some(backup) = &work.backup {
            if let Err(reason) = self.back_up(id, events, host, backup, &patterns).await {
                return failed(reason);
            }
        }
        HostState::Done
    }

    /// Sends the show-config command and keeps what it showed, without
    /// its echo and the prompt after it, in the config history.
    async fn back_up(
        &mut self,
        id: SessionId,
        events: &mut SessionEvents,
        host: &PlannedHost,
        backup: &BackupWork,
        patterns: &Patterns,
    ) -> Result<(), String> {
        let command = &backup.command;
        // Output holds complete lines only: the prompt the command is
        // typed at is not in it yet, and the one after it never is.
        let start = self.output.text.len();
        let prompt = self.text.current_line().trim_end().to_string();
        match self.send(id, events, command, patterns, Expect::Only).await {
            Ok(Shown::Prompt) => {}
            Ok(Shown::PrivilegePrompt) => return Err(asked_for_password(command)),
            // Only the very prompt it was typed at ends a configuration: say
            // so if the device seems to have stopped at another.
            Err(NoPrompt::TimedOut) if patterns.prompt_end.is_match(&self.text.current_line()) => {
                return Err(format!(
                    "{}, and its last line {:?} is not the prompt {:?} was typed at ({}); nothing was backed up",
                    self.no_prompt_after(&quoted(command), NoPrompt::TimedOut),
                    self.text.current_line().trim(),
                    command,
                    prompt.trim()
                ))
            }
            Err(error) => return Err(self.no_prompt_after(&quoted(command), error)),
        }
        // Only part of it was kept: never a version.
        if self.output.is_cut() {
            return Err(format!(
                "{command:?} showed more than the {} a job keeps of a host's output; nothing was backed up",
                bytes(self.options.output_limit)
            ));
        }
        // A bad pattern is refused when the configuration is kept.
        let ignored = backup
            .ignored_lines
            .as_deref()
            .and_then(|pattern| Regex::new(pattern).ok());
        let config = config_shown(
            &self.output.text[start..],
            &prompt,
            command,
            ignored.as_ref(),
        );
        if config.is_empty() {
            return Err(format!("{command:?} showed nothing to back up"));
        }
        if let Some(error) = only_an_error(&config) {
            return Err(format!(
                "{command:?} showed only an error, not a configuration: {error:?}"
            ));
        }
        let Some((store, workspace)) = &self.configs else {
            unreachable!("a config backup starts only with a store and a workspace id")
        };
        let outcome = store
            .save(
                workspace,
                &host.id,
                &config,
                backup.ignored_lines.as_deref(),
            )
            .map_err(|error| format!("the configuration could not be kept: {error}"))?;
        let _ = self.events.send(JobEvent::Backup {
            host: self.id.clone(),
            outcome: outcome.clone(),
        });
        self.backup = Some(outcome);
        Ok(())
    }

    /// The host's patterns, or why one cannot be used.
    fn patterns(&self, host: &PlannedHost) -> Result<Patterns, String> {
        let compile = |pattern: String, what: &str| {
            Regex::new(&pattern).map_err(|error| {
                format!(
                    "the {what} pattern of the platform {} cannot be used: {error}",
                    host.platform
                )
            })
        };
        let prompt = &host.work.prompt;
        Ok(Patterns {
            prompt_end: compile(format!(r"(?:{prompt})[ \t]*\z"), "prompt")?,
            prompt_line: compile(format!(r"\A[ \t]*(?:{prompt})[ \t]*\z"), "prompt")?,
            privilege_prompt: match &host.work.privilege_prompt {
                Some(pattern) => Some(compile(
                    format!(r"(?:{pattern})[ \t]*\z"),
                    "privilege password",
                )?),
                None => None,
            },
        })
    }

    /// Sends the privilege command `command`, answering its password
    /// question with the profile's enable secret; why not, if the host does
    /// not get back to its prompt, or gets back to the very prompt it had
    /// before (the device refused the secret without asking again).
    ///
    /// Without a question, coming back to the very prompt it was typed at
    /// is privileged mode reached only if the device said nothing (a login
    /// privileged already); if it said something ("% No password set"),
    /// the device refused, and the host fails with what it said. A changed
    /// prompt is privileged mode, whatever was said on the way.
    async fn enter_privileged(
        &mut self,
        id: SessionId,
        events: &mut SessionEvents,
        host: &PlannedHost,
        command: &str,
        patterns: &Patterns,
    ) -> Result<(), String> {
        let work = &host.work;
        let before = self.text.current_line().trim().to_string();
        let start = self.output.text.len();
        match self.send(id, events, command, patterns, Expect::Change).await {
            Ok(Shown::Prompt) => {
                let after = self.text.current_line().trim().to_string();
                let said = said_after(&self.output.text[start..], command);
                if after != before || said.is_empty() {
                    return Ok(());
                }
                return Err(format!(
                    "{command:?} did not reach privileged mode: the device answered {said:?} and stayed at {after}"
                ));
            }
            Ok(Shown::PrivilegePrompt) => {}
            Err(NoPrompt::TimedOut) if patterns.privilege_prompt.is_none() => {
                return Err(format!(
                    "no prompt after {command:?}: the device showed nothing more for {}; if it asked for a password, the platform {} has no privilege password pattern to recognise it",
                    span(self.options.command_timeout),
                    host.platform
                ))
            }
            Err(error) => return Err(self.no_prompt_after(&quoted(command), error)),
        }
        let Some(secret) = &work.enable_secret else {
            return Err(match &work.profile {
                Some(profile) => format!(
                    "{command:?} asked for a password, and the credential profile {profile} has no enable secret stored"
                ),
                None => format!(
                    "{command:?} asked for a password, and no credential profile applies to give an enable secret"
                ),
            });
        };
        let refused = || match &work.profile {
            Some(profile) => {
                format!("the device refused the enable secret of the credential profile {profile}")
            }
            None => "the device refused the enable secret".to_string(),
        };
        if !secret.0.trim().is_empty() {
            self.typed_secret = Some(secret.clone());
        }
        let shown = if self
            .manager
            .write(id, format!("{}\r", secret.0).into_bytes())
            .is_err()
        {
            Err(NoPrompt::NotSent)
        } else {
            self.wait_for(events, patterns, true, Expect::Change).await
        };
        // Only the answer to the question can be its echo. What the device
        // shows afterwards is the user's: a configuration that holds the
        // same text as the secret is kept as it is, not with part of it
        // replaced.
        self.typed_secret = None;
        match shown {
            Ok(Shown::Prompt) if self.text.current_line().trim() == before => Err(refused()),
            Ok(Shown::Prompt) => Ok(()),
            Ok(Shown::PrivilegePrompt) => Err(refused()),
            Err(error) => Err(self.no_prompt_after("the enable secret", error)),
        }
    }

    /// Sends `line` and waits for the device to answer with the prompt
    /// `expect` says (or its privilege password question).
    async fn send(
        &mut self,
        id: SessionId,
        events: &mut SessionEvents,
        line: &str,
        patterns: &Patterns,
        expect: Expect,
    ) -> Result<Shown, NoPrompt> {
        if self
            .manager
            .write(id, format!("{line}\r").into_bytes())
            .is_err()
        {
            return Err(NoPrompt::NotSent);
        }
        self.wait_for(events, patterns, true, expect).await
    }

    /// Why the host failed when `what` (a quoted line, or a description
    /// such as "the enable secret") got no prompt back.
    fn no_prompt_after(&self, what: &str, error: NoPrompt) -> String {
        match error {
            NoPrompt::TimedOut => format!(
                "no prompt after {what}: the device showed nothing more for {}",
                span(self.options.command_timeout)
            ),
            NoPrompt::Endless => format!(
                "no prompt after {what}: it was still showing output after {}, the most one command may take",
                span(self.options.command_limit)
            ),
            NoPrompt::Closed(reason) => format!("the session ended after {what}: {reason}"),
            NoPrompt::NotSent => format!("the session ended before {what} was sent"),
        }
    }

    /// Opens the host's session into `session` and follows it until it is
    /// connected; why not, if it does not get there.
    async fn open(
        &mut self,
        host: &PlannedHost,
        session: &mut Opened,
        open: Open,
    ) -> Result<(), String> {
        let connect = |runner: &Self, target: &SshTarget| {
            runner
                .manager
                .open(SessionTarget::Ssh(target.clone()), JOB_TERMINAL_SIZE)
        };
        let Open::Channel { of } = open else {
            let Some(target) = &host.work.target else {
                return Err("there is nothing to connect it with".into());
            };
            let (id, events) = session.insert(connect(self, target));
            return self.connect(*id, events).await;
        };
        let (id, mut events) = self
            .manager
            .open_channel(of, JOB_TERMINAL_SIZE)
            .map_err(|error| format!("no second channel could be opened: {error}"))?;
        let refused = match self.channel_ready(&mut events).await {
            Ok(()) => {
                *session = Some((id, events));
                return Ok(());
            }
            Err(CloseReason::Failed(OpenError::ChannelRefused { reason })) => reason,
            Err(reason) => {
                self.manager.discard(id);
                return Err(format!("the second channel closed: {reason}"));
            }
        };
        self.manager.discard(id);
        let Some(target) = &host.work.target else {
            return Err(format!(
                "the device refused a second channel ({refused}), and the login was typed or asked for, so a second connection would have to ask for it again"
            ));
        };
        let (id, events) = session.insert(connect(self, target));
        self.connect(*id, events)
            .await
            .map_err(|reason| format!("the device refused a second channel ({refused}), and a second connection failed: {reason}"))
    }

    /// Follows a second channel until it is connected; why not, if it
    /// closes instead.
    async fn channel_ready(&mut self, events: &mut SessionEvents) -> Result<(), CloseReason> {
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => self.take(events, &bytes),
                Some(SessionEvent::State(SessionState::Connected)) => return Ok(()),
                Some(SessionEvent::State(SessionState::Closed(reason))) => return Err(reason),
                Some(_) => {}
                None => return Err(CloseReason::ClosedLocally),
            }
        }
    }

    /// Follows the session `id` until it is connected; why not, if it does
    /// not get there. Host keys that are not trusted are refused and login
    /// questions are not answered: the host fails, naming what was asked.
    async fn connect(&mut self, id: SessionId, events: &mut SessionEvents) -> Result<(), String> {
        let mut refused = None;
        loop {
            match events.next().await {
                Some(SessionEvent::Output(bytes)) => self.take(events, &bytes),
                Some(SessionEvent::State(SessionState::Connected)) => return Ok(()),
                Some(SessionEvent::State(SessionState::Closed(reason))) => {
                    return Err(refused.unwrap_or_else(|| reason.to_string()))
                }
                Some(SessionEvent::State(SessionState::Connecting)) => {}
                Some(SessionEvent::HostKey(question)) => {
                    refused = Some(host_key_refused(&question));
                    let _ = self.manager.decide_host_key(id, HostKeyDecision::Cancel);
                }
                Some(SessionEvent::Prompt(prompt)) => {
                    refused = Some(login_question_refused(&prompt));
                    let _ = self.manager.close(id);
                }
                Some(
                    SessionEvent::Algorithms(_)
                    | SessionEvent::LogFailed(_)
                    | SessionEvent::Backup(_),
                ) => {}
                None => return Err(refused.unwrap_or_else(|| "the session went away".into())),
            }
        }
    }

    /// Takes output until the line being shown is the prompt `expect` says
    /// (or the privilege password question) and nothing more arrives for as
    /// long as that prompt must settle ([`Self::shown`]). With
    /// `after_input`, what is already shown does not count: the device must
    /// answer first. The prompt found is the one the next line is typed at.
    ///
    /// It gives up when nothing has arrived for the command timeout
    /// (counted from the last output) or, while output keeps arriving,
    /// once the command limit has passed since it started waiting. A settle
    /// longer than what is left of the command timeout ends with it.
    async fn wait_for(
        &mut self,
        events: &mut SessionEvents,
        patterns: &Patterns,
        after_input: bool,
        expect: Expect,
    ) -> Result<Shown, NoPrompt> {
        let started = Instant::now();
        let limit = started + self.options.command_limit;
        let mut quiet_until = started + self.options.command_timeout;
        let mut answered = !after_input;
        loop {
            let deadline = quiet_until.min(limit);
            let shown = if answered {
                self.shown(&self.text.current_line(), patterns, expect)
            } else {
                None
            };
            let until = match shown {
                Some((_, settle)) => (Instant::now() + settle).min(deadline),
                None => deadline,
            };
            match tokio::time::timeout_at(until, events.next()).await {
                // Quiet at the prompt (even if only just in time).
                Err(_) => {
                    let Some((shown, _)) = shown else {
                        return Err(if limit <= quiet_until {
                            NoPrompt::Endless
                        } else {
                            NoPrompt::TimedOut
                        });
                    };
                    if shown == Shown::Prompt {
                        let line = self.text.current_line();
                        if expect == Expect::First {
                            self.whole_lines = patterns.prompt_line.is_match(&line);
                        }
                        self.at = Some(line.trim().to_string());
                    }
                    return Ok(shown);
                }
                Ok(Some(SessionEvent::Output(bytes))) => {
                    self.take(events, &bytes);
                    answered = true;
                    quiet_until = Instant::now() + self.options.command_timeout;
                }
                Ok(Some(SessionEvent::State(SessionState::Closed(reason)))) => {
                    return Err(NoPrompt::Closed(reason))
                }
                Ok(Some(_)) => {}
                Ok(None) => return Err(NoPrompt::Closed(CloseReason::ClosedLocally)),
            }
        }
    }

    /// What `line`, the line being shown, is (the prompt, or the privilege
    /// password question) when the job waits for `expect`, and how long it
    /// must stay quiet to count; `None` if it is neither.
    ///
    /// The prompt the line was typed at, the same line again, counts after
    /// [`PROMPT_SETTLE`] (except at the first prompt, which has none to be
    /// compared with). A changed prompt is a line the pattern matches whole
    /// (or, for a pattern written for the end of the line only, a line it
    /// matches at the end): after [`PROMPT_CHANGE_SETTLE`] where a change
    /// is expected, after [`PROMPT_DOUBT_SETTLE`] where it is not, and never
    /// after the show-config command. The first prompt is a line the pattern
    /// matches whole after [`PROMPT_SETTLE`], or at its end after
    /// [`PROMPT_CHANGE_SETTLE`].
    fn shown(&self, line: &str, patterns: &Patterns, expect: Expect) -> Option<(Shown, Duration)> {
        let trimmed = line.trim();
        let same = !trimmed.is_empty() && self.at.as_deref() == Some(trimmed);
        let settle = match expect {
            Expect::First if patterns.prompt_line.is_match(line) => Some(PROMPT_SETTLE),
            Expect::First if patterns.prompt_end.is_match(line) => Some(PROMPT_CHANGE_SETTLE),
            Expect::First => None,
            _ if same => Some(PROMPT_SETTLE),
            Expect::Only => None,
            Expect::Same | Expect::Change => {
                let pattern = if self.whole_lines {
                    &patterns.prompt_line
                } else {
                    &patterns.prompt_end
                };
                pattern
                    .is_match(line)
                    .then_some(if expect == Expect::Change {
                        PROMPT_CHANGE_SETTLE
                    } else {
                        PROMPT_DOUBT_SETTLE
                    })
            }
        };
        if let Some(settle) = settle {
            return Some((Shown::Prompt, settle));
        }
        patterns
            .privilege_prompt
            .as_ref()
            .is_some_and(|question| question.is_match(line))
            .then_some((Shown::PrivilegePrompt, PROMPT_SETTLE))
    }

    /// Takes output from the device: acknowledged at once, kept as text.
    fn take(&mut self, events: &SessionEvents, bytes: &[u8]) {
        events.acknowledge(bytes.len());
        let lines = self.text.push(bytes);
        self.add_output(lines);
    }

    fn add_output(&mut self, mut text: String) {
        if text.is_empty() {
            return;
        }
        // Complete lines: an echoed secret is whole in one of them.
        if let Some(secret) = &self.typed_secret {
            if text.contains(&secret.0) {
                text = text.replace(&secret.0, HIDDEN_SECRET);
            }
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

/// What a job shows in place of the enable secret, should a device echo it.
pub const HIDDEN_SECRET: &str = "********";

/// The configuration in `shown`, the plain-text output of `command` typed
/// at `prompt`: without the echo (the first line, if it is the prompt and
/// the command), without blank lines at either end, and without the lines
/// at its end that match the `ignored` lines pattern (the first lines of a
/// prompt of several, such as SR Linux's `--{ running }--[  ]--`); every
/// line ends with a line break.
fn config_shown(shown: &str, prompt: &str, command: &str, ignored: Option<&Regex>) -> String {
    let mut lines = crate::diff::lines(shown);
    if lines.first().is_some_and(|echo| {
        echo.strip_prefix(prompt)
            .is_some_and(|typed| typed.trim() == command.trim())
    }) {
        lines.remove(0);
    }
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| {
        line.trim().is_empty() || ignored.is_some_and(|ignored| ignored.is_match(line))
    }) {
        lines.pop();
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// What the device said in `shown`, the plain-text output (complete lines)
/// after `command` was typed: its lines without the echo (a first line
/// ending with the command) and without blank ones, joined by spaces.
fn said_after(shown: &str, command: &str) -> String {
    let mut lines = crate::diff::lines(shown);
    if lines
        .first()
        .is_some_and(|echo| echo.trim_end().ends_with(command.trim()))
    {
        lines.remove(0);
    }
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The device's error, when `config` (what the show-config command showed)
/// is nothing but one: every line a `%` message, or the `^` marker IOS
/// puts above one. Such a text is never a configuration.
fn only_an_error(config: &str) -> Option<String> {
    let lines: Vec<&str> = config
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let messages: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with('%'))
        .collect();
    let all = lines
        .iter()
        .all(|line| line.starts_with('%') || line.chars().all(|c| c == '^'));
    (all && !messages.is_empty()).then(|| messages.join(" "))
}

fn failed(reason: String) -> HostState {
    HostState::Failed { reason }
}

/// The line that ends a host's kept output once it reached `limit` bytes
/// ([`JobOptions::output_limit`]).
pub fn output_cut(limit: usize) -> String {
    format!(
        "[enodix: the output went on past {}, the most a job keeps of it; the rest was not kept]\n",
        bytes(limit)
    )
}

/// `count` bytes for the user: whole MiB or KiB where it is, else bytes.
fn bytes(count: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;
    if count >= MIB && count.is_multiple_of(MIB) {
        format!("{} MiB", count / MIB)
    } else if count >= KIB && count.is_multiple_of(KIB) {
        format!("{} KiB", count / KIB)
    } else {
        format!("{count} bytes")
    }
}

/// A host's (or a script's) output as a job keeps it: everything up to its
/// limit, then a line saying the rest was not kept ([`output_cut`]), then
/// nothing more. So a flood is never held whole: not in the result, not in
/// the events, not in the kept run.
#[derive(Debug)]
pub(super) struct KeptOutput {
    pub(super) text: String,
    limit: usize,
    cut: bool,
}

impl KeptOutput {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
            cut: false,
        }
    }

    /// Keeps what fits of `piece` (whole lines where it can); what was
    /// kept, to report, if anything.
    pub(super) fn add(&mut self, piece: String) -> Option<String> {
        if self.cut || piece.is_empty() {
            return None;
        }
        if self.text.len() + piece.len() <= self.limit {
            self.text.push_str(&piece);
            return Some(piece);
        }
        let mut end = self.limit.saturating_sub(self.text.len());
        while !piece.is_char_boundary(end) {
            end -= 1;
        }
        let end = piece[..end].rfind('\n').map_or(end, |at| at + 1);
        let mut kept = piece[..end].to_string();
        let ends_a_line = if kept.is_empty() {
            self.text.is_empty() || self.text.ends_with('\n')
        } else {
            kept.ends_with('\n')
        };
        if !ends_a_line {
            kept.push('\n');
        }
        kept.push_str(&output_cut(self.limit));
        self.text.push_str(&kept);
        self.cut = true;
        Some(kept)
    }

    /// Whether the limit was reached.
    pub(super) fn is_cut(&self) -> bool {
        self.cut
    }
}

/// `line` quoted, for messages.
fn quoted(line: &str) -> String {
    format!("{line:?}")
}

/// Why a host failed whose device asked for a password after `line`.
fn asked_for_password(line: &str) -> String {
    format!("{line:?} asked for a password, and a job only answers the privilege command's")
}

/// `duration` for the user: `60 s`, `0.5 s`, or whole minutes from two
/// minutes on (`30 min`).
fn span(duration: Duration) -> String {
    let whole = duration.as_secs();
    if whole >= 120 && whole.is_multiple_of(60) && duration.subsec_nanos() == 0 {
        format!("{} min", whole / 60)
    } else {
        format!("{} s", duration.as_secs_f64())
    }
}

/// Why a host whose key was not trusted failed.
pub(crate) fn host_key_refused(question: &HostKeyQuestion) -> String {
    let at = format!("{} port {}", question.host, question.port);
    if question.changed() {
        format!(
            "the host key of {at} has changed since it was trusted ({}); a job never accepts a changed key: connect in a terminal to check it",
            question.key.fingerprint
        )
    } else {
        format!(
            "the host key of {at} is not trusted yet ({}); a job never trusts a key: connect once in a terminal to check and trust it",
            question.key.fingerprint
        )
    }
}

/// Why a host that asked a login question failed.
pub(crate) fn login_question_refused(prompt: &LoginPrompt) -> String {
    let questions: Vec<String> = prompt
        .questions
        .iter()
        .map(|question| format!("{:?}", question.text.trim()))
        .collect();
    let questions = if questions.is_empty() {
        "a question".to_string()
    } else {
        questions.join(", ")
    };
    let from = if prompt.name.is_empty() {
        String::new()
    } else {
        format!(" ({})", prompt.name)
    };
    format!("the login asked {questions}{from}; a job never answers login questions")
}
