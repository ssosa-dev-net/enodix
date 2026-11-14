//! Running a planned job: one headless session per host, at most
//! [`JobOptions::parallel`] at once.

use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::Instant;

use super::{JobPlan, PlannedHost};
use crate::logging::PlainText;
use crate::session::{
    CloseReason, HostKeyDecision, HostKeyQuestion, LoginPrompt, SessionEvent, SessionEvents,
    SessionId, SessionManager, SessionState, SessionTarget, TerminalSize,
};
use crate::workspace::HostId;

/// How many hosts run at once when nothing says otherwise.
pub const DEFAULT_PARALLEL: usize = 10;
/// The most hosts that may run at once.
pub const MAX_PARALLEL: usize = 50;
/// How long a host may take to show its prompt after a line is sent (or
/// after logging in) when nothing says otherwise.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// How long output must stay quiet after it ends with the prompt for the
/// prompt to count, so that output merely ending in `#` for a moment is not
/// taken for it (the containerlab suite's rule).
pub const PROMPT_SETTLE: Duration = Duration::from_millis(300);
/// The terminal size job sessions ask for: wide, so that devices wrap as
/// few lines as possible.
pub const JOB_TERMINAL_SIZE: TerminalSize = TerminalSize {
    cols: 400,
    rows: 100,
};

/// How a job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobOptions {
    /// How many hosts run at once (each with its one connection open), 1
    /// to [`MAX_PARALLEL`]; [`start`] refuses other values.
    pub parallel: usize,
    /// How long a host may take to show its prompt after each line; a host
    /// that takes longer fails.
    pub command_timeout: Duration,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            parallel: DEFAULT_PARALLEL,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
}

/// Where a host of a running job has got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HostState {
    /// Not started yet: other hosts take all the room.
    Waiting,
    /// Its session is being opened.
    Connecting,
    /// Logged in; the snippet is being sent.
    Running,
    /// Every line was sent and answered with the prompt.
    Done,
    /// It could not be reached, logged in to, or driven: `reason` says why.
    Failed { reason: String },
    /// The job was cancelled before the host finished.
    Cancelled,
}

impl HostState {
    /// Whether the host has finished, one way or another.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Done | Self::Failed { .. } | Self::Cancelled)
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
    /// The job has ended: every host's final state and output. The last
    /// event.
    Finished(JobReport),
}

/// What a job did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobReport {
    /// Every host that ran, in plan order.
    pub hosts: Vec<HostResult>,
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

/// Starts running `plan` (which must be [ready](JobPlan::ready)) with
/// sessions opened by `manager`; returns at once.
///
/// Must be called from within a Tokio runtime.
pub fn start(
    manager: &SessionManager,
    plan: JobPlan,
    options: JobOptions,
) -> Result<Job, StartError> {
    if !plan.missing_variables.is_empty() {
        return Err(StartError::MissingVariables(plan.missing_variables));
    }
    if !(1..=MAX_PARALLEL).contains(&options.parallel) {
        return Err(StartError::Parallel(options.parallel));
    }
    if plan.hosts.is_empty() {
        return Err(StartError::NothingToRun);
    }
    let (events_tx, events) = mpsc::unbounded_channel();
    let (cancel_tx, cancel) = watch::channel(false);
    let room = Arc::new(Semaphore::new(options.parallel));

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
            let runner = HostRunner {
                manager: manager.clone(),
                events: events_tx.clone(),
                id: host.id.clone(),
                options,
                text: PlainText::new(),
                output: String::new(),
            };
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
                }
            });
            hosts.push(result);
        }
        let _ = events_tx.send(JobEvent::Finished(JobReport { hosts }));
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
    text: PlainText,
    /// The plain text so far.
    output: String,
}

/// The host's session, once opened.
type Opened = Option<(SessionId, SessionEvents)>;

/// How waiting for a prompt ended, if not with the prompt.
enum NoPrompt {
    TimedOut,
    Closed(CloseReason),
    /// The line could not be sent: the session had ended.
    NotSent,
}

/// The patterns a host is driven by.
struct Patterns {
    /// The prompt, at the end of the line being shown.
    prompt: Regex,
    /// The privilege password question, likewise.
    privilege_prompt: Option<Regex>,
}

/// What came back after something was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Prompt,
    PrivilegePrompt,
}

/// How long a closed session may take to say so before it is let go.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

impl HostRunner {
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
                self.drive(&host, &mut session).await
            } => state,
            () = cancelled => HostState::Cancelled,
        };
        if let Some((id, events)) = session.take() {
            self.close(id, events).await;
        }
        drop(permit);
        let rest = self.text.finish();
        self.add_output(rest);
        self.state(state.clone());
        HostResult {
            id: host.id,
            name: host.name,
            state,
            output: self.output,
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
    async fn drive(&mut self, host: &PlannedHost, session: &mut Opened) -> HostState {
        let work = &host.work;
        let patterns = match self.patterns(host) {
            Ok(patterns) => patterns,
            Err(reason) => return failed(reason),
        };
        self.state(HostState::Connecting);
        let (id, events) = session.insert(
            self.manager
                .open(SessionTarget::Ssh(work.target.clone()), JOB_TERMINAL_SIZE),
        );
        let id = *id;
        if let Err(reason) = self.connect(id, events).await {
            return failed(reason);
        }
        self.state(HostState::Running);

        if let Err(error) = self.wait_for(events, &patterns, false).await {
            return failed(match error {
                NoPrompt::TimedOut => format!(
                    "no prompt within {} of logging in (does the platform {}'s prompt pattern match?)",
                    seconds(self.options.command_timeout),
                    host.platform
                ),
                NoPrompt::Closed(reason) => {
                    format!("the session ended before the first prompt: {reason}")
                }
                NoPrompt::NotSent => unreachable!("nothing was sent"),
            });
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
            match self.send(id, events, command, &patterns).await {
                Ok(Shown::Prompt) => {}
                Ok(Shown::PrivilegePrompt) => return failed(asked_for_password(command)),
                Err(error) => return failed(self.no_prompt_after(&quoted(command), error)),
            }
        }
        let count = work.lines.len();
        for (index, line) in work.lines.iter().enumerate() {
            match self.send(id, events, line, &patterns).await {
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
        HostState::Done
    }

    /// The host's patterns, or why one cannot be used.
    fn patterns(&self, host: &PlannedHost) -> Result<Patterns, String> {
        let at_end = |pattern: &str, what: &str| {
            Regex::new(&format!(r"(?:{pattern})[ \t]*\z")).map_err(|error| {
                format!(
                    "the {what} pattern of the platform {} cannot be used: {error}",
                    host.platform
                )
            })
        };
        Ok(Patterns {
            prompt: at_end(&host.work.prompt, "prompt")?,
            privilege_prompt: match &host.work.privilege_prompt {
                Some(pattern) => Some(at_end(pattern, "privilege password")?),
                None => None,
            },
        })
    }

    /// Sends the privilege command `command`, answering its password
    /// question with the profile's enable secret; why not, if the host does
    /// not get back to its prompt, or gets back to the very prompt it had
    /// before (the device refused the secret without asking again).
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
        match self.send(id, events, command, patterns).await {
            Ok(Shown::Prompt) => return Ok(()),
            Ok(Shown::PrivilegePrompt) => {}
            Err(NoPrompt::TimedOut) if patterns.privilege_prompt.is_none() => {
                return Err(format!(
                    "no prompt within {} after {command:?}; if it asked for a password, the platform {} has no privilege password pattern to recognise it",
                    seconds(self.options.command_timeout),
                    host.platform
                ))
            }
            Err(error) => return Err(self.no_prompt_after(&quoted(command), error)),
        }
        let Some(secret) = &work.enable_secret else {
            return Err(format!(
                "{command:?} asked for a password, and the credential profile {} has no enable secret stored",
                work.profile
            ));
        };
        let refused = || {
            format!(
                "the device refused the enable secret of the credential profile {}",
                work.profile
            )
        };
        let shown = if self
            .manager
            .write(id, format!("{}\r", secret.0).into_bytes())
            .is_err()
        {
            Err(NoPrompt::NotSent)
        } else {
            self.wait_for(events, patterns, true).await
        };
        match shown {
            Ok(Shown::Prompt) if self.text.current_line().trim() == before => Err(refused()),
            Ok(Shown::Prompt) => Ok(()),
            Ok(Shown::PrivilegePrompt) => Err(refused()),
            Err(error) => Err(self.no_prompt_after("the enable secret", error)),
        }
    }

    /// Sends `line` and waits for the device to answer with its prompt (or
    /// its privilege password question).
    async fn send(
        &mut self,
        id: SessionId,
        events: &mut SessionEvents,
        line: &str,
        patterns: &Patterns,
    ) -> Result<Shown, NoPrompt> {
        if self
            .manager
            .write(id, format!("{line}\r").into_bytes())
            .is_err()
        {
            return Err(NoPrompt::NotSent);
        }
        self.wait_for(events, patterns, true).await
    }

    /// Why the host failed when `what` (a quoted line, or a description
    /// such as "the enable secret") got no prompt back.
    fn no_prompt_after(&self, what: &str, error: NoPrompt) -> String {
        match error {
            NoPrompt::TimedOut => format!(
                "no prompt within {} after {what}",
                seconds(self.options.command_timeout)
            ),
            NoPrompt::Closed(reason) => format!("the session ended after {what}: {reason}"),
            NoPrompt::NotSent => format!("the session ended before {what} was sent"),
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
                Some(SessionEvent::Algorithms(_) | SessionEvent::LogFailed(_)) => {}
                None => return Err(refused.unwrap_or_else(|| "the session went away".into())),
            }
        }
    }

    /// Takes output until the line being shown ends with the prompt (or
    /// the privilege password question) and nothing more arrives for
    /// [`PROMPT_SETTLE`]. With `after_input`, what is already shown does
    /// not count: the device must answer first.
    async fn wait_for(
        &mut self,
        events: &mut SessionEvents,
        patterns: &Patterns,
        after_input: bool,
    ) -> Result<Shown, NoPrompt> {
        let deadline = Instant::now() + self.options.command_timeout;
        let mut answered = !after_input;
        loop {
            let shown = if answered {
                let line = self.text.current_line();
                if patterns.prompt.is_match(&line) {
                    Some(Shown::Prompt)
                } else if patterns
                    .privilege_prompt
                    .as_ref()
                    .is_some_and(|question| question.is_match(&line))
                {
                    Some(Shown::PrivilegePrompt)
                } else {
                    None
                }
            } else {
                None
            };
            let until = if shown.is_some() {
                (Instant::now() + PROMPT_SETTLE).min(deadline)
            } else {
                deadline
            };
            match tokio::time::timeout_at(until, events.next()).await {
                // Quiet at the prompt (even if only just in time).
                Err(_) => return shown.ok_or(NoPrompt::TimedOut),
                Ok(Some(SessionEvent::Output(bytes))) => {
                    self.take(events, &bytes);
                    answered = true;
                }
                Ok(Some(SessionEvent::State(SessionState::Closed(reason)))) => {
                    return Err(NoPrompt::Closed(reason))
                }
                Ok(Some(_)) => {}
                Ok(None) => return Err(NoPrompt::Closed(CloseReason::ClosedLocally)),
            }
        }
    }

    /// Takes output from the device: acknowledged at once, kept as text.
    fn take(&mut self, events: &SessionEvents, bytes: &[u8]) {
        events.acknowledge(bytes.len());
        let lines = self.text.push(bytes);
        self.add_output(lines);
    }

    fn add_output(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.output.push_str(&text);
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

fn failed(reason: String) -> HostState {
    HostState::Failed { reason }
}

/// `line` quoted, for messages.
fn quoted(line: &str) -> String {
    format!("{line:?}")
}

/// Why a host failed whose device asked for a password after `line`.
fn asked_for_password(line: &str) -> String {
    format!("{line:?} asked for a password, and a job only answers the privilege command's")
}

/// `duration` in seconds, for the user: `60 s`, `0.5 s`.
fn seconds(duration: Duration) -> String {
    format!("{} s", duration.as_secs_f64())
}

/// Why a host whose key was not trusted failed.
fn host_key_refused(question: &HostKeyQuestion) -> String {
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
fn login_question_refused(prompt: &LoginPrompt) -> String {
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
