//! A snippet's steps: sending lines to the session that is open, waiting
//! for the device between them, answering its questions and stopping at its
//! errors (#23).
//!
//! A plain snippet is sent as a paste, line after line. A snippet with steps
//! is a short script for the session in front of the user, over SSH, telnet
//! or serial alike:
//!
//! - **send** a line, then by default **wait** for the device's prompt (the
//!   `prompt` pattern of the host's platform definition, else
//!   [`DEFAULT_PROMPT`]); or wait for a pattern of its own, or not at all;
//! - **answer** a question whenever its pattern shows while waiting (the
//!   `[confirm]` of a `copy`, a destination file name);
//! - **stop** whenever one of the stop patterns shows (`% Invalid input`),
//!   saying at which step and after which line.
//!
//! Each wait has a timeout; the run stops when it passes. The run also stops
//! when the user stops it, when the user's own input reaches the session
//! ([`StepRuns::user_input`]: the input that stops it is not sent, nor the
//! rest of a paste it began), and when the session's connection ends. Nothing
//! is ever retried.
//!
//! Patterns are plain text, matched as written (case counts); a pattern
//! between slashes (`/Proceed.*\?/`) is a regular expression. They are
//! looked for in what the device showed since the line was sent (the echo of
//! the line itself aside), as plain text ([`crate::logging::PlainText`]).
//!
//! The steps of a snippet are a template like its text: `{{variables}}` in
//! the lines sent and the answers given are filled from the session's host
//! and the user's answers ([`Steps::fill`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::layout::InputOrigin;
use crate::logging::PlainText;
use crate::paste::{confirms_steps, PasteSafety};
use crate::session::{OutputWatch, SessionId, SessionManager};
use crate::template::{MissingVariables, Template, Variables};

/// How long a wait goes on, when neither the step nor the snippet says.
pub const DEFAULT_WAIT_TIMEOUT_SECS: u32 = 30;
/// The longest a wait may be given, in seconds (a day).
pub const MAX_WAIT_TIMEOUT_SECS: u32 = 24 * 60 * 60;
/// The prompt waited for on a session whose host has no platform (or no
/// host): a word ending in `>`, `#`, `$` or `%` at the end of the line.
pub const DEFAULT_PROMPT: &str = r"\S*[>#$%]";
/// How long the device must stay quiet at its prompt for the prompt to
/// count: a line merely ending like a prompt for a moment does not (the
/// job engine's rule, [`crate::job::PROMPT_SETTLE`]).
pub const PROMPT_SETTLE: Duration = crate::job::PROMPT_SETTLE;
/// How much of what the device showed a run keeps to look for patterns in.
const KEPT_TEXT: usize = 64 * 1024;

/// What a step waits for after its line (or, with no line, by itself).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "pattern", rename_all = "camelCase")]
pub enum Wait {
    /// The device's prompt, quiet for [`PROMPT_SETTLE`].
    Prompt,
    /// A pattern (see the module docs).
    For(String),
    /// Nothing: the next step follows at once (after the host's paste line
    /// delay).
    Nothing,
}

/// One step of a snippet: a line to send, what to wait for then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// The line sent (a template), its line break added; `None`: the step
    /// only waits.
    #[serde(default)]
    pub send: Option<String>,
    pub wait: Wait,
    /// Seconds this wait may take; `None`: the snippet's.
    #[serde(default)]
    pub timeout_secs: Option<u32>,
}

impl Step {
    /// Sends `line`, then waits for the prompt.
    pub fn send(line: impl Into<String>) -> Self {
        Self {
            send: Some(line.into()),
            wait: Wait::Prompt,
            timeout_secs: None,
        }
    }

    /// Only waits for `pattern`.
    pub fn wait_for(pattern: impl Into<String>) -> Self {
        Self {
            send: None,
            wait: Wait::For(pattern.into()),
            timeout_secs: None,
        }
    }

    /// This step, waiting for `wait` instead.
    pub fn waiting(self, wait: Wait) -> Self {
        Self { wait, ..self }
    }

    /// This step, its wait given `seconds`.
    pub fn within(self, seconds: u32) -> Self {
        Self {
            timeout_secs: Some(seconds),
            ..self
        }
    }
}

/// A question the device may ask while a step waits, and the line that
/// answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    /// The pattern of the question.
    pub when: String,
    /// The line sent (a template; may be empty: just the line break).
    pub send: String,
}

/// The steps of a snippet, as written in the snippets file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Steps {
    pub steps: Vec<Step>,
    /// Answered whenever their question shows while a step waits, the first
    /// that matches first.
    #[serde(default)]
    pub answers: Vec<Answer>,
    /// The run stops whenever one of these shows.
    #[serde(default)]
    pub stops: Vec<String>,
    /// Seconds each wait may take, unless its step says.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u32,
}

fn default_timeout() -> u32 {
    DEFAULT_WAIT_TIMEOUT_SECS
}

impl Steps {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            answers: Vec::new(),
            stops: Vec::new(),
            timeout_secs: DEFAULT_WAIT_TIMEOUT_SECS,
        }
    }

    /// These steps, answering `when` with `send`.
    pub fn answering(mut self, when: impl Into<String>, send: impl Into<String>) -> Self {
        self.answers.push(Answer {
            when: when.into(),
            send: send.into(),
        });
        self
    }

    /// These steps, stopping at `pattern`.
    pub fn stopping_at(mut self, pattern: impl Into<String>) -> Self {
        self.stops.push(pattern.into());
        self
    }

    /// These steps, each wait given `seconds` unless its step says.
    pub fn within(self, seconds: u32) -> Self {
        Self {
            timeout_secs: seconds,
            ..self
        }
    }

    /// Why these steps cannot run as written, if they cannot: no step, a
    /// step that neither sends nor waits, a pattern that is empty or not a
    /// valid regular expression, a timeout out of bounds.
    pub fn problem(&self) -> Option<String> {
        if self.steps.is_empty() {
            return Some("it has no steps".into());
        }
        let timeout = |seconds: u32| {
            (!(1..=MAX_WAIT_TIMEOUT_SECS).contains(&seconds)).then(|| {
                format!(
                    "a timeout must be from 1 to {MAX_WAIT_TIMEOUT_SECS} seconds, not {seconds}"
                )
            })
        };
        if let Some(problem) = timeout(self.timeout_secs) {
            return Some(problem);
        }
        for (index, step) in self.steps.iter().enumerate() {
            let number = index + 1;
            if step.send.is_none() && step.wait == Wait::Nothing {
                return Some(format!("step {number} neither sends a line nor waits"));
            }
            if let Wait::For(pattern) = &step.wait {
                if let Err(problem) = Pattern::new(pattern) {
                    return Some(format!("step {number}: {problem}"));
                }
            }
            if let Some(problem) = step.timeout_secs.and_then(timeout) {
                return Some(format!("step {number}: {problem}"));
            }
        }
        for answer in &self.answers {
            if let Err(problem) = Pattern::new(&answer.when) {
                return Some(format!("an answer: {problem}"));
            }
        }
        for stop in &self.stops {
            if let Err(problem) = Pattern::new(stop) {
                return Some(format!("a stop: {problem}"));
            }
        }
        None
    }

    /// Every template of these steps: the lines sent, then the answers.
    fn templates(&self) -> impl Iterator<Item = &str> {
        self.steps
            .iter()
            .filter_map(|step| step.send.as_deref())
            .chain(self.answers.iter().map(|answer| answer.send.as_str()))
    }

    /// The names of the variables of the lines sent and the answers given,
    /// once each (ignoring case), in order.
    pub fn variables(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for template in self.templates() {
            for name in Template::parse(template).variables() {
                if !names
                    .iter()
                    .any(|seen| seen.to_lowercase() == name.to_lowercase())
                {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Every word of the steps, for search.
    pub(crate) fn text(&self) -> String {
        let mut text = String::new();
        for step in &self.steps {
            if let Some(line) = &step.send {
                text.push_str(line);
                text.push('\n');
            }
            if let Wait::For(pattern) = &step.wait {
                text.push_str(pattern);
                text.push('\n');
            }
        }
        for answer in &self.answers {
            text.push_str(&answer.when);
            text.push('\n');
        }
        for stop in &self.stops {
            text.push_str(stop);
            text.push('\n');
        }
        text
    }

    /// The steps ready to run on a session: their variables filled from
    /// `values`, waiting for the prompt `prompt` (a platform's pattern;
    /// `None`: [`DEFAULT_PROMPT`]). Without a value for every variable,
    /// which ones are missing.
    pub fn fill(&self, values: &Variables, prompt: Option<&str>) -> Result<StepsPlan, FillError> {
        let missing: Vec<String> = self
            .variables()
            .into_iter()
            .filter(|name| values.get(name).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(FillError::Missing(MissingVariables { names: missing }));
        }
        let fill = |template: &str| {
            Template::parse(template)
                .fill(values)
                .map_err(FillError::Missing)
        };
        let pattern = |source: &str| Pattern::new(source).map_err(FillError::Invalid);
        let prompt = prompt
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
            .unwrap_or(DEFAULT_PROMPT);
        let prompt_end = Regex::new(&format!(r"(?:{prompt})[ \t]*\z")).map_err(|error| {
            FillError::Invalid(format!(
                "the prompt pattern {prompt:?} cannot be used: {error}"
            ))
        })?;
        let mut steps = Vec::new();
        for step in &self.steps {
            steps.push(PlannedStep {
                send: step.send.as_deref().map(fill).transpose()?.map(one_line),
                wait: match &step.wait {
                    Wait::Prompt => PlannedWait::Prompt,
                    Wait::For(source) => PlannedWait::For(pattern(source)?),
                    Wait::Nothing => PlannedWait::Nothing,
                },
                timeout: Duration::from_secs(u64::from(
                    step.timeout_secs.unwrap_or(self.timeout_secs),
                )),
            });
        }
        let mut answers = Vec::new();
        for answer in &self.answers {
            answers.push(PlannedAnswer {
                when: pattern(&answer.when)?,
                send: one_line(fill(&answer.send)?),
            });
        }
        let stops = self
            .stops
            .iter()
            .map(|source| pattern(source))
            .collect::<Result<_, _>>()?;
        Ok(StepsPlan {
            steps,
            answers,
            stops,
            prompt: prompt.to_string(),
            prompt_end,
        })
    }
}

/// A line to send never holds a line break of its own: a value with one
/// is cut there.
fn one_line(text: String) -> String {
    match text.find(['\r', '\n']) {
        Some(end) => text[..end].to_string(),
        None => text,
    }
}

/// Why steps could not be made ready.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FillError {
    /// Some variables have no value: the user is to be asked for them.
    #[error(transparent)]
    Missing(MissingVariables),
    /// A pattern cannot be used.
    #[error("{0}")]
    Invalid(String),
}

/// A pattern looked for in a device's output: plain text, or a regular
/// expression between slashes.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    regex: Regex,
}

impl Pattern {
    pub fn new(source: &str) -> Result<Self, String> {
        if source.is_empty() {
            return Err("a pattern cannot be empty".into());
        }
        let expression = match source
            .strip_prefix('/')
            .and_then(|rest| rest.strip_suffix('/'))
        {
            Some("") => return Err("a pattern cannot be empty".into()),
            Some(expression) => expression.to_string(),
            None => regex::escape(source),
        };
        let regex = Regex::new(&expression)
            .map_err(|error| format!("the pattern {source:?} cannot be used: {error}"))?;
        Ok(Self {
            source: source.to_string(),
            regex,
        })
    }

    /// As written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Where it first shows in `text`: the start and end of the match.
    fn find(&self, text: &str) -> Option<(usize, usize)> {
        self.regex
            .find(text)
            .map(|found| (found.start(), found.end()))
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

/// Steps ready to run on one session (see [`Steps::fill`]).
#[derive(Debug, Clone)]
pub struct StepsPlan {
    steps: Vec<PlannedStep>,
    answers: Vec<PlannedAnswer>,
    stops: Vec<Pattern>,
    /// The prompt pattern, as given.
    prompt: String,
    /// The prompt at the end of the line being shown.
    prompt_end: Regex,
}

#[derive(Debug, Clone)]
struct PlannedStep {
    send: Option<String>,
    wait: PlannedWait,
    timeout: Duration,
}

#[derive(Debug, Clone)]
enum PlannedWait {
    Prompt,
    For(Pattern),
    Nothing,
}

#[derive(Debug, Clone)]
struct PlannedAnswer {
    when: Pattern,
    send: String,
}

impl StepsPlan {
    /// How many steps there are.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The prompt pattern its waits for the prompt look for.
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// What identifies these steps exactly as filled: every line sent,
    /// every answer, pattern, wait and timeout, and the prompt. The user
    /// confirms a plan by its digest ([`Permission::previewed`]); a plan
    /// filled again from a snippet that changed meanwhile has another.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut field = |text: &str| {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        };
        field(&self.prompt);
        for step in &self.steps {
            field(step.send.as_deref().unwrap_or("\0none"));
            match &step.wait {
                PlannedWait::Prompt => field("\0prompt"),
                PlannedWait::For(pattern) => field(pattern.source()),
                PlannedWait::Nothing => field("\0nothing"),
            }
            field(&step.timeout.as_millis().to_string());
        }
        field("\0answers");
        for answer in &self.answers {
            field(answer.when.source());
            field(&answer.send);
        }
        field("\0stops");
        for stop in &self.stops {
            field(stop.source());
        }
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The lines sent by the steps, in order (answers aside).
    pub fn lines(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter_map(|step| step.send.as_deref())
            .collect()
    }

    /// What the steps do, one line each, for the user to check before they
    /// run: the answers and stops first, then each step, numbered from 1.
    pub fn preview(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for answer in &self.answers {
            lines.push(format!(
                "Whenever {} shows: answer {}",
                quoted(answer.when.source()),
                answered(&answer.send)
            ));
        }
        for stop in &self.stops {
            lines.push(format!("Whenever {} shows: stop", quoted(stop.source())));
        }
        for (index, step) in self.steps.iter().enumerate() {
            lines.push(format!("{}. {}", index + 1, self.describe(step)));
        }
        lines
    }

    /// What `step` does, in a few words.
    fn describe(&self, step: &PlannedStep) -> String {
        let wait = self.waiting_for(step);
        match (&step.send, &step.wait) {
            (Some(line), PlannedWait::Nothing) => format!("Send {}", quoted(line)),
            (Some(line), _) => format!("Send {}, wait for {wait}", quoted(line)),
            (None, _) => format!("Wait for {wait}"),
        }
    }

    /// What `step` waits for, with its timeout.
    fn waiting_for(&self, step: &PlannedStep) -> String {
        let seconds = step.timeout.as_secs();
        match &step.wait {
            PlannedWait::Prompt => format!("the prompt (up to {seconds} s)"),
            PlannedWait::For(pattern) => {
                format!("{} (up to {seconds} s)", quoted(pattern.source()))
            }
            PlannedWait::Nothing => "nothing".into(),
        }
    }
}

fn quoted(text: &str) -> String {
    format!("\u{201c}{text}\u{201d}")
}

fn answered(line: &str) -> String {
    if line.is_empty() {
        "with Enter".into()
    } else {
        quoted(line)
    }
}

/// What a run tells while it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RunEvent {
    /// The step `index` (from 0) of `count` begins: `doing` says what it
    /// does.
    #[serde(rename_all = "camelCase")]
    Step {
        index: usize,
        count: usize,
        doing: String,
    },
    /// A question showed while the step `index` waited, and was answered.
    /// The answer itself is not told (it may be a secret).
    #[serde(rename_all = "camelCase")]
    Answered { index: usize, question: String },
    /// The run ended: the last event. `message`: what the user is told
    /// ([`RunEnd::message`]).
    Ended { end: RunEnd, message: String },
}

/// Who called a run off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CalledOff {
    /// The user stopped it ([`StepRuns::stop`]).
    User,
    /// The user's own typed input reached the session ([`StepRuns::user_input`]).
    Input,
    /// A paste (or a snippet) reached the session: none of it was sent.
    Paste,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RunEnd {
    /// Every step was done.
    Finished,
    /// A stop pattern showed at step `step` (from 0), after `line` (the
    /// last line sent, if any): `pattern` as written, `shown` the line of
    /// output it showed in.
    #[serde(rename_all = "camelCase")]
    Stopped {
        step: usize,
        line: Option<String>,
        pattern: String,
        shown: String,
    },
    /// The wait of step `step` passed its timeout.
    #[serde(rename_all = "camelCase")]
    TimedOut {
        step: usize,
        line: Option<String>,
        waiting_for: String,
    },
    /// The user stopped it, or the user's input did.
    #[serde(rename_all = "camelCase")]
    CalledOff {
        step: usize,
        line: Option<String>,
        by: CalledOff,
    },
    /// The session's connection ended during step `step`.
    #[serde(rename_all = "camelCase")]
    SessionEnded { step: usize, line: Option<String> },
}

impl RunEnd {
    /// What the user is told.
    pub fn message(&self) -> String {
        let at = |step: usize, line: &Option<String>| match line {
            Some(line) => format!("at step {} (after {})", step + 1, quoted(line)),
            None => format!("at step {}", step + 1),
        };
        match self {
            Self::Finished => "Done: every step ran.".into(),
            Self::Stopped {
                step,
                line,
                pattern,
                shown,
            } => format!(
                "Stopped {}: {} showed ({}). Nothing more was sent.",
                at(*step, line),
                quoted(pattern),
                shown.trim()
            ),
            Self::TimedOut {
                step,
                line,
                waiting_for,
            } => format!(
                "Stopped {}: waited for {waiting_for}, in vain. Nothing more was sent.",
                at(*step, line)
            ),
            Self::CalledOff {
                step,
                line,
                by: CalledOff::User,
            } => format!("Stopped by you {}. Nothing more was sent.", at(*step, line)),
            Self::CalledOff {
                step,
                line,
                by: CalledOff::Input,
            } => format!(
                "Stopped {} as you typed in the session (that key was not sent). Nothing more was sent.",
                at(*step, line)
            ),
            Self::CalledOff {
                step,
                line,
                by: CalledOff::Paste,
            } => format!(
                "Stopped {} as you pasted in the session (none of that paste was sent). Nothing more was sent.",
                at(*step, line)
            ),
            Self::SessionEnded { step, line } => {
                format!("Stopped {}: the session ended.", at(*step, line))
            }
        }
    }
}

/// What a run may start on: what the user was shown and said, and where
/// the session stands. [`StepRuns::start`] refuses what these do not allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permission {
    /// The session's host's paste safety (its environment, its line delay).
    pub safety: PasteSafety,
    /// The user's preference for confirming pastes of several lines.
    pub confirm_multi_line: bool,
    /// The digest ([`StepsPlan::digest`]) of the plan the user was shown.
    pub previewed: String,
    /// Whether the user said yes to it.
    pub confirmed: bool,
    /// Whether the session is shown by a tab of the multi-send group.
    pub in_group: bool,
}

/// Why a run could not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    /// Its steps must be confirmed first (always on production), and were
    /// not: nothing was sent.
    #[error("the steps run only once they are confirmed, so nothing was sent")]
    NotConfirmed,
    /// The steps differ from those shown (the snippet, the host or its
    /// platform changed since): nothing was sent.
    #[error("the snippet changed since you checked it, so nothing was sent; look again")]
    Changed,
    /// The session is in the multi-send group: steps run on one session at
    /// a time.
    #[error(
        "steps run on one session at a time: take the tab out of the multi-send group to run them"
    )]
    InGroup,
    #[error("steps are already running on this session; stop them first")]
    AlreadyRunning,
    #[error("the session is not connected")]
    NotConnected,
}

/// The snippet steps running on the app's sessions, one run per session at
/// most. Cloning gives another handle to the same runs.
#[derive(Clone, Default)]
pub struct StepRuns {
    inner: Arc<Mutex<Runs>>,
}

#[derive(Default)]
struct Runs {
    next: u64,
    running: HashMap<SessionId, Running>,
    /// The paste last refused on each session because it stopped a run
    /// ([`StepRuns::user_input_of`]): its other lines are refused too.
    refused: HashMap<SessionId, u64>,
}

struct Running {
    run: u64,
    call_off: watch::Sender<Option<CalledOff>>,
}

impl StepRuns {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Runs> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `plan` on the connected session `session`, telling `report`
    /// each step, each answer and how it ended (always last), as
    /// `permission` allows: refused unless it is the plan the user was
    /// shown, unconfirmed where steps are confirmed
    /// ([`crate::paste::confirms_steps`]: always on production), on a
    /// session of the multi-send group, and while steps run on that session
    /// already. The host's paste line delay is kept after a line no wait
    /// follows.
    ///
    /// Must be called within a Tokio runtime.
    pub fn start(
        &self,
        manager: &SessionManager,
        session: SessionId,
        plan: StepsPlan,
        permission: &Permission,
        mut report: impl FnMut(RunEvent) + Send + 'static,
    ) -> Result<(), StartError> {
        if permission.in_group {
            return Err(StartError::InGroup);
        }
        if permission.previewed != plan.digest() {
            return Err(StartError::Changed);
        }
        if !permission.confirmed
            && confirms_steps(&permission.safety, permission.confirm_multi_line)
        {
            return Err(StartError::NotConfirmed);
        }
        let line_delay = Duration::from_millis(u64::from(
            permission
                .safety
                .line_delay_ms
                .min(crate::paste::MAX_PASTE_LINE_DELAY_MS),
        ));
        let mut runs = self.lock();
        if runs.running.contains_key(&session) {
            return Err(StartError::AlreadyRunning);
        }
        // Watched from before the first line, so no answer is missed.
        let watch = manager
            .watch_output(session)
            .map_err(|_| StartError::NotConnected)?;
        let (call_off, called_off) = watch::channel(None);
        runs.next += 1;
        let run = runs.next;
        runs.running.insert(session, Running { run, call_off });
        drop(runs);

        let manager = manager.clone();
        let runs = self.clone();
        tokio::spawn(async move {
            let mut runner = Runner {
                manager,
                session,
                plan: &plan,
                watch,
                called_off,
                screen: Screen::default(),
                line: None,
                line_delay,
                report: &mut report,
            };
            let end = runner.run().await;
            // Gone from the runs before anybody learns it ended.
            let mut all = runs.lock();
            if all
                .running
                .get(&session)
                .is_some_and(|running| running.run == run)
            {
                all.running.remove(&session);
            }
            drop(all);
            let message = end.message();
            report(RunEvent::Ended { end, message });
        });
        Ok(())
    }

    /// Whether steps run on `session` now.
    pub fn running(&self, session: SessionId) -> bool {
        self.lock().running.contains_key(&session)
    }

    /// Stops the steps running on `session` (nothing more is sent); false
    /// if none were.
    pub fn stop(&self, session: SessionId) -> bool {
        self.call_off(session, CalledOff::User)
    }

    fn call_off(&self, session: SessionId, by: CalledOff) -> bool {
        Self::call_off_in(&mut self.lock(), session, by)
    }

    fn call_off_in(runs: &mut Runs, session: SessionId, by: CalledOff) -> bool {
        match runs.running.remove(&session) {
            Some(running) => {
                let _ = running.call_off.send(Some(by));
                true
            }
            None => false,
        }
    }

    /// [`Self::user_input_of`] for input that is not a line of a paste the
    /// caller tracks.
    pub fn user_input(&self, targets: Vec<SessionId>, origin: InputOrigin) -> Vec<SessionId> {
        self.user_input_of(targets, origin, None)
    }

    /// The user's input from a terminal, of `origin`, is about to be written
    /// to `targets`: the sessions it may still go to. A run on a target is
    /// stopped by the user's own input (typed, pasted, mouse), and that
    /// target does not get the input, so the key that stops a run never
    /// reaches the device. The terminal's replies to the device go on (they
    /// are not the user's).
    ///
    /// `paste` identifies the paste a line belongs to (the interface sends
    /// each line of a paste as its own input, and gives them one id, new for
    /// each paste): when a paste stops a run, none of its lines reaches that
    /// target, not only the first. Typing, and a later paste (another id),
    /// go through as usual.
    pub fn user_input_of(
        &self,
        targets: Vec<SessionId>,
        origin: InputOrigin,
        paste: Option<u64>,
    ) -> Vec<SessionId> {
        if origin == InputOrigin::Reply {
            return targets;
        }
        let pasted = matches!(origin, InputOrigin::Pasted | InputOrigin::GroupPaste);
        let by = if pasted {
            CalledOff::Paste
        } else {
            CalledOff::Input
        };
        let mut runs = self.lock();
        targets
            .into_iter()
            .filter(|&session| {
                if pasted && paste.is_some() && runs.refused.get(&session) == paste.as_ref() {
                    return false;
                }
                if Self::call_off_in(&mut runs, session, by) {
                    if let (true, Some(paste)) = (pasted, paste) {
                        runs.refused.insert(session, paste);
                    }
                    return false;
                }
                true
            })
            .collect()
    }
}

/// What the device showed since a run began, as plain text, and where the
/// part still to look in starts.
#[derive(Default)]
struct Screen {
    text: PlainText,
    /// The lines completed (kept up to [`KEPT_TEXT`]).
    done: String,
    /// Where in `done` followed by the current line the text still to look
    /// in starts.
    from: usize,
    /// Whether output came since the last line was sent: until it does, a
    /// prompt shown is the one it was typed at.
    fresh: bool,
    /// The line last sent: its echo is not looked in.
    echo: Option<String>,
}

impl Screen {
    fn push(&mut self, bytes: &[u8]) {
        self.done.push_str(&self.text.push(bytes));
        self.fresh = true;
        let over = self.done.len().saturating_sub(KEPT_TEXT);
        if over > 0 {
            let mut cut = over;
            while !self.done.is_char_boundary(cut) {
                cut += 1;
            }
            self.done.drain(..cut);
            self.from = self.from.saturating_sub(cut);
        }
    }

    fn all(&self) -> String {
        let mut all = self.done.clone();
        all.push_str(&self.text.current_line());
        all
    }

    /// The text still to look in, and where it starts in [`Self::all`].
    fn window(&self) -> (String, usize) {
        let all = self.all();
        let mut start = self.from.min(all.len());
        while !all.is_char_boundary(start) {
            start -= 1;
        }
        if let Some(echo) = &self.echo {
            let rest = &all[start..];
            let blanks = rest.len() - rest.trim_start().len();
            if rest[blanks..].starts_with(echo.as_str()) {
                start += blanks + echo.len();
            }
        }
        (all[start..].to_string(), start)
    }

    /// Nothing up to `end` (in [`Self::all`]) is looked in again.
    fn consume(&mut self, end: usize) {
        self.from = end;
        self.echo = None;
    }

    /// A line is about to be sent (`echo`: a step's, whose echo is not
    /// looked in): only what comes after counts.
    fn sent(&mut self, echo: Option<&str>) {
        self.from = self.all().len();
        self.fresh = false;
        self.echo = echo
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string);
    }

    fn current_line(&self) -> String {
        self.text.current_line()
    }

    /// The whole line of `all` holding the byte at `at`.
    fn line_at(all: &str, at: usize) -> String {
        let start = all[..at].rfind('\n').map_or(0, |found| found + 1);
        let end = all[at..].find('\n').map_or(all.len(), |found| at + found);
        all[start..end].to_string()
    }
}

/// One run of a plan on a session.
struct Runner<'a, R: FnMut(RunEvent)> {
    manager: SessionManager,
    session: SessionId,
    plan: &'a StepsPlan,
    watch: OutputWatch,
    called_off: watch::Receiver<Option<CalledOff>>,
    screen: Screen,
    /// The line last sent by a step.
    line: Option<String>,
    line_delay: Duration,
    report: &'a mut R,
}

/// Why a wait did not end as it should.
enum Waited {
    Done,
    Ended(RunEnd),
}

impl<R: FnMut(RunEvent)> Runner<'_, R> {
    async fn run(&mut self) -> RunEnd {
        let count = self.plan.steps.len();
        for (index, step) in self.plan.steps.iter().enumerate() {
            if let Some(by) = *self.called_off.borrow() {
                return self.called_off_at(index, by);
            }
            (self.report)(RunEvent::Step {
                index,
                count,
                doing: self.plan.describe(step),
            });
            if let Some(line) = &step.send {
                self.screen.sent(Some(line));
                if !self.write(line) {
                    return RunEnd::SessionEnded {
                        step: index,
                        line: self.line.clone(),
                    };
                }
                self.line = Some(line.clone());
            }
            match self.wait(index, step).await {
                Waited::Done => {}
                Waited::Ended(end) => return end,
            }
        }
        RunEnd::Finished
    }

    fn write(&self, line: &str) -> bool {
        self.manager
            .write(self.session, format!("{line}\r").into_bytes())
            .is_ok()
    }

    /// Resolves once the run is called off, saying by whom (the runs gone
    /// counts as the user).
    async fn called(called_off: &mut watch::Receiver<Option<CalledOff>>) -> CalledOff {
        match called_off.wait_for(Option::is_some).await {
            Ok(by) => by.unwrap_or(CalledOff::User),
            Err(_) => CalledOff::User,
        }
    }

    fn called_off_at(&self, step: usize, by: CalledOff) -> RunEnd {
        RunEnd::CalledOff {
            step,
            line: self.line.clone(),
            by,
        }
    }

    /// Takes output until `step`'s wait is over, answering questions on the
    /// way; or why the run ends there.
    async fn wait(&mut self, index: usize, step: &PlannedStep) -> Waited {
        let deadline = Instant::now() + step.timeout;
        if matches!(step.wait, PlannedWait::Nothing) {
            if self.line_delay.is_zero() {
                return Waited::Done;
            }
            // Only the delay; output meanwhile is kept for the next wait.
            let until = Instant::now() + self.line_delay;
            tokio::select! {
                () = tokio::time::sleep_until(until) => return Waited::Done,
                by = Self::called(&mut self.called_off) => {
                    return Waited::Ended(self.called_off_at(index, by));
                }
            }
        }
        loop {
            let (window, start) = self.screen.window();
            let all = self.screen.all();
            // Stops first: an error never gets an answer.
            for stop in &self.plan.stops {
                if let Some((found, _)) = stop.find(&window) {
                    return Waited::Ended(RunEnd::Stopped {
                        step: index,
                        line: self.line.clone(),
                        pattern: stop.source().to_string(),
                        shown: Screen::line_at(&all, start + found),
                    });
                }
            }
            if let PlannedWait::For(pattern) = &step.wait {
                if let Some((_, end)) = pattern.find(&window) {
                    self.screen.consume(start + end);
                    return Waited::Done;
                }
            }
            let question = self
                .plan
                .answers
                .iter()
                .filter_map(|answer| answer.when.find(&window).map(|found| (found, answer)))
                .min_by_key(|((found, _), _)| *found);
            if let Some(((found, end), answer)) = question {
                let question = Screen::line_at(&all, start + found).trim().to_string();
                self.screen.consume(start + end);
                self.screen.sent(None);
                if !self.write(&answer.send) {
                    return Waited::Ended(RunEnd::SessionEnded {
                        step: index,
                        line: self.line.clone(),
                    });
                }
                (self.report)(RunEvent::Answered { index, question });
                continue;
            }
            // At the prompt, once output came since the line: done if it
            // stays quiet a moment.
            let at_prompt = matches!(step.wait, PlannedWait::Prompt)
                && self.screen.fresh
                && self.plan.prompt_end.is_match(&self.screen.current_line());
            let until = if at_prompt {
                (Instant::now() + PROMPT_SETTLE).min(deadline)
            } else {
                deadline
            };
            tokio::select! {
                output = self.watch.next() => match output {
                    Some(bytes) => self.screen.push(&bytes),
                    None => return Waited::Ended(RunEnd::SessionEnded {
                        step: index,
                        line: self.line.clone(),
                    }),
                },
                () = tokio::time::sleep_until(until) => {
                    if at_prompt {
                        // Quiet at the prompt (even if only just in time).
                        self.screen.consume(self.screen.all().len());
                        return Waited::Done;
                    }
                    return Waited::Ended(RunEnd::TimedOut {
                        step: index,
                        line: self.line.clone(),
                        waiting_for: self.plan.waiting_for(step),
                    });
                }
                by = Self::called(&mut self.called_off) => {
                    return Waited::Ended(self.called_off_at(index, by));
                }
            }
        }
    }
}
