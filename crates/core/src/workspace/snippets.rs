//! Snippets: named pieces of command text, kept in [`SNIPPETS_FILE`] at the
//! workspace root (so the team shares them) as a mapping of names to their
//! text, or to their text and group:
//!
//! ```yaml
//! Show version: show version     # just the text
//! Interface errors:
//!   group: Diagnostics           # optional; the app lists snippets by group
//!   text: |
//!     show interface {{interface}}
//!     show interface {{interface}} counters errors
//! ```
//!
//! The text is a template (see [`crate::template`]): its variables are
//! filled from the host of the session it is sent to, and the user is asked
//! for the others. Snippets are listed in file order; a snippet defined badly
//! is a problem on the file and is left out.
//!
//! In place of its text, a snippet may have steps (#23, [`crate::steps`]):
//! lines sent one at a time to the open session, each waiting for the
//! device, with questions answered and errors stopping the run:
//!
//! ```yaml
//! Save the configuration:
//!   group: Changes
//!   timeout: 60                    # seconds each wait may take (30)
//!   answer:                        # whenever this shows, send that line
//!     "Destination filename": ""   # (empty: just Enter)
//!   stop:                          # whenever one shows, stop
//!     - "% Invalid input"
//!   steps:
//!     - copy running-config startup-config   # send, wait for the prompt
//!     - send: reload in 5
//!       wait: /Proceed with reload\?/        # a pattern of its own
//!       timeout: 10
//!     - send: "y"
//!       wait: none                            # do not wait
//!     - wait: prompt                          # only wait
//! ```

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use super::scalar_text;
use crate::steps::{Answer, Step, Steps, Wait, DEFAULT_WAIT_TIMEOUT_SECS};
use crate::template::{MissingVariables, Template, Variables};

/// The file at the workspace root holding the snippets.
pub const SNIPPETS_FILE: &str = "_snippets.yaml";

const TEXT: &str = "text";
const GROUP: &str = "group";
const STEPS: &str = "steps";
const ANSWER: &str = "answer";
const STOP: &str = "stop";
const TIMEOUT: &str = "timeout";
const SEND: &str = "send";
const WAIT: &str = "wait";
/// A step's `wait` for the prompt (the default after a line).
const WAIT_PROMPT: &str = "prompt";
/// A step's `wait` for nothing.
const WAIT_NONE: &str = "none";

/// A named piece of text to send to a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    /// Unique in the workspace, ignoring case.
    pub name: String,
    /// What the app lists it under; `None`: no group.
    #[serde(default)]
    pub group: Option<String>,
    /// A template; each line is sent as a line of its own. Empty when the
    /// snippet has steps.
    #[serde(default)]
    pub text: String,
    /// Its steps (#23), sent to the open session one at a time, in place of
    /// the text; `None`: it is sent as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Steps>,
}

impl Snippet {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            group: None,
            text: text.into(),
            steps: None,
        }
    }

    /// A snippet called `name` with `steps` in place of a text.
    pub fn with_steps(name: impl Into<String>, steps: Steps) -> Self {
        Self {
            name: name.into(),
            group: None,
            text: String::new(),
            steps: Some(steps),
        }
    }

    /// This snippet in the group `group`.
    pub fn in_group(self, group: impl Into<String>) -> Self {
        Self {
            group: Some(group.into()),
            ..self
        }
    }

    /// The names of its variables, once each, in order.
    pub fn variables(&self) -> Vec<String> {
        match &self.steps {
            Some(steps) => steps.variables(),
            None => Template::parse(&self.text).variables(),
        }
    }

    /// The lines to send, with the variables filled from `values`: the text
    /// split at each line break (`\r\n`, `\n` or `\r`), a break at the very
    /// end ending the last line rather than starting an empty one. A value
    /// holding line breaks gives lines of its own.
    pub fn expand(&self, values: &Variables) -> Result<Vec<String>, MissingVariables> {
        let text = Template::parse(&self.text).fill(values)?;
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let text = text.strip_suffix('\n').unwrap_or(&text);
        Ok(text.split('\n').map(str::to_string).collect())
    }

    /// Whether every word of `query` is part of its name, group or text
    /// (ignoring case), and whether they all are part of its name.
    pub(super) fn matches(&self, words: &[String]) -> Option<bool> {
        let name = crate::names::folded(&self.name);
        let group = crate::names::folded(self.group.as_deref().unwrap_or_default());
        let text = match &self.steps {
            Some(steps) => crate::names::folded(&steps.text()),
            None => crate::names::folded(&self.text),
        };
        words
            .iter()
            .all(|word| name.contains(word) || group.contains(word) || text.contains(word))
            .then(|| words.iter().all(|word| name.contains(word)))
    }
}

/// Why a snippet could not be made ready to send.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExpandError {
    #[error("there is no snippet called {0}")]
    NoSuchSnippet(String),
    #[error("{0} is no longer in the workspace")]
    NoSuchHost(super::HostId),
    /// The snippet has steps (#23): they run on one open session at a
    /// time, waiting for its device, not as lines sent as a paste (to the
    /// multi-send group, or by a job).
    #[error(
        "the snippet {0} has steps, which run on one open session at a time (not on the multi-send group, nor in a job)"
    )]
    HasSteps(String),
    /// The snippet has no steps: it is sent as text.
    #[error("the snippet {0} has no steps")]
    NoSteps(String),
    /// A pattern of its steps cannot be used.
    #[error("the snippet {name}: {problem}")]
    BadSteps { name: String, problem: String },
    /// Some variables have no value: the user is to be asked for them.
    #[error(transparent)]
    Missing(MissingVariables),
}

/// The snippets of the snippets file (`document`), in file order, and a
/// message for each one defined badly (it is left out).
pub(super) fn load(document: &Mapping) -> (Vec<Snippet>, Vec<String>) {
    let mut snippets: Vec<Snippet> = Vec::new();
    let mut errors = Vec::new();
    for (key, definition) in document {
        let Some(name) = scalar_text(key) else {
            errors.push("snippet names must be text".into());
            continue;
        };
        let name = name.trim();
        match parse(name, definition).and_then(|snippet| clean(&snippet)) {
            Ok(snippet) if snippets.iter().any(|seen| same(&seen.name, &snippet.name)) => {
                errors.push(format!("snippet `{name}` is defined twice"));
            }
            Ok(snippet) => snippets.push(snippet),
            Err(message) => errors.push(format!("snippet `{name}`: {message}")),
        }
    }
    (snippets, errors)
}

fn parse(name: &str, definition: &Value) -> Result<Snippet, String> {
    if let Some(text) = scalar_text(definition) {
        return Ok(Snippet::new(name, text));
    }
    let Value::Mapping(fields) = definition else {
        return Err(format!("expected its text, or `{TEXT}` and `{GROUP}`"));
    };
    let mut text = None;
    let mut group = None;
    let mut steps = None;
    let mut answers = None;
    let mut stops = None;
    let mut timeout = None;
    for (key, value) in fields {
        match scalar_text(key).as_deref() {
            Some(TEXT) => {
                text = Some(scalar_text(value).ok_or_else(|| format!("`{TEXT}` must be text"))?);
            }
            Some(GROUP) if value.is_null() => {}
            Some(GROUP) => {
                group = Some(scalar_text(value).ok_or_else(|| format!("`{GROUP}` must be text"))?);
            }
            Some(STEPS) => steps = Some(parse_steps(value)?),
            Some(ANSWER) => answers = Some(parse_answers(value)?),
            Some(STOP) => stops = Some(parse_stops(value)?),
            Some(TIMEOUT) => timeout = Some(parse_timeout(value)?),
            Some(other) => return Err(format!("unknown key `{other}`")),
            None => return Err("keys must be text".into()),
        }
    }
    let steps = match (text, steps) {
        (Some(_), Some(_)) => return Err(format!("it has both `{TEXT}` and `{STEPS}`")),
        (None, None) => return Err(format!("missing `{TEXT}` (or `{STEPS}`)")),
        (Some(text), None) => {
            if answers.is_some() || stops.is_some() || timeout.is_some() {
                return Err(format!(
                    "`{ANSWER}`, `{STOP}` and `{TIMEOUT}` go with `{STEPS}`, not `{TEXT}`"
                ));
            }
            return Ok(Snippet {
                name: name.to_string(),
                group,
                text,
                steps: None,
            });
        }
        (None, Some(steps)) => Steps {
            steps,
            answers: answers.unwrap_or_default(),
            stops: stops.unwrap_or_default(),
            timeout_secs: timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS),
        },
    };
    Ok(Snippet {
        name: name.to_string(),
        group,
        text: String::new(),
        steps: Some(steps),
    })
}

fn parse_steps(value: &Value) -> Result<Vec<Step>, String> {
    let Value::Sequence(items) = value else {
        return Err(format!("`{STEPS}` must be a list"));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            parse_step(item).map_err(|message| format!("step {}: {message}", index + 1))
        })
        .collect()
}

fn parse_step(item: &Value) -> Result<Step, String> {
    if let Some(line) = scalar_text(item) {
        return Ok(Step::send(line));
    }
    let Value::Mapping(fields) = item else {
        return Err(format!(
            "expected a line, or `{SEND}`, `{WAIT}` and `{TIMEOUT}`"
        ));
    };
    let mut send = None;
    let mut wait = None;
    let mut timeout = None;
    for (key, value) in fields {
        match scalar_text(key).as_deref() {
            Some(SEND) => {
                send = Some(scalar_text(value).ok_or_else(|| format!("`{SEND}` must be text"))?);
            }
            Some(WAIT) => {
                let pattern = match value {
                    Value::Bool(false) | Value::Null => WAIT_NONE.to_string(),
                    value => scalar_text(value).ok_or_else(|| format!("`{WAIT}` must be text"))?,
                };
                wait = Some(match pattern.as_str() {
                    WAIT_PROMPT => Wait::Prompt,
                    WAIT_NONE => Wait::Nothing,
                    _ => Wait::For(pattern),
                });
            }
            Some(TIMEOUT) => timeout = Some(parse_timeout(value)?),
            Some(other) => return Err(format!("unknown key `{other}`")),
            None => return Err("keys must be text".into()),
        }
    }
    let wait = match (wait, &send) {
        (Some(wait), _) => wait,
        (None, Some(_)) => Wait::Prompt,
        (None, None) => return Err(format!("it has neither `{SEND}` nor `{WAIT}`")),
    };
    Ok(Step {
        send,
        wait,
        timeout_secs: timeout,
    })
}

fn parse_answers(value: &Value) -> Result<Vec<Answer>, String> {
    let Value::Mapping(fields) = value else {
        return Err(format!(
            "`{ANSWER}` must map each question to the line that answers it"
        ));
    };
    fields
        .iter()
        .map(|(when, send)| {
            let when = scalar_text(when).ok_or("questions must be text")?;
            let send = match send {
                Value::Null => String::new(),
                send => scalar_text(send)
                    .ok_or_else(|| format!("the answer to {when:?} must be text"))?,
            };
            Ok(Answer { when, send })
        })
        .collect()
}

fn parse_stops(value: &Value) -> Result<Vec<String>, String> {
    if let Some(pattern) = scalar_text(value) {
        return Ok(vec![pattern]);
    }
    let Value::Sequence(items) = value else {
        return Err(format!("`{STOP}` must be a pattern or a list of them"));
    };
    items
        .iter()
        .map(|item| scalar_text(item).ok_or_else(|| format!("`{STOP}` patterns must be text")))
        .collect()
}

fn parse_timeout(value: &Value) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|seconds| u32::try_from(seconds).ok())
        .ok_or_else(|| format!("`{TIMEOUT}` must be a number of seconds"))
}

/// `snippet` with its name and group trimmed (a blank group is none), if it
/// can be saved; else why not.
pub(super) fn clean(snippet: &Snippet) -> Result<Snippet, String> {
    let name = snippet.name.trim();
    if name.is_empty() {
        return Err("it has no name".into());
    }
    match &snippet.steps {
        Some(steps) => {
            if !snippet.text.is_empty() {
                return Err("it has both a text and steps".into());
            }
            if let Some(problem) = steps.problem() {
                return Err(problem);
            }
        }
        None if snippet.text.trim().is_empty() => return Err("it has no text".into()),
        None => {}
    }
    let group = snippet
        .group
        .as_deref()
        .map(str::trim)
        .filter(|group| !group.is_empty());
    Ok(Snippet {
        name: name.to_string(),
        group: group.map(str::to_string),
        text: snippet.text.clone(),
        steps: snippet.steps.clone(),
    })
}

/// Writes `snippet` into the snippets file's `document`: in place of the
/// entry called `previous`, else after the others. Refused (with the name)
/// when another entry has its name.
pub(super) fn write(
    document: &mut Mapping,
    previous: Option<&str>,
    snippet: &Snippet,
) -> Result<(), String> {
    let mut definition = Mapping::new();
    if let Some(group) = &snippet.group {
        definition.insert(GROUP.into(), group.as_str().into());
    }
    match &snippet.steps {
        Some(steps) => write_steps(&mut definition, steps),
        None => {
            definition.insert(TEXT.into(), snippet.text.as_str().into());
        }
    }
    // An entry of that name that is not the one replaced (one defined badly,
    // so not loaded) is never written over.
    let taken = document.keys().any(|key| {
        same_key(key, &snippet.name) && previous.is_none_or(|previous| !same_key(key, previous))
    });
    if taken {
        return Err(snippet.name.clone());
    }
    let name = Value::from(snippet.name.as_str());
    let definition = Value::Mapping(definition);
    let Some(previous) =
        previous.filter(|previous| document.keys().any(|key| same_key(key, previous)))
    else {
        document.insert(name, definition);
        return Ok(());
    };
    *document = std::mem::take(document)
        .into_iter()
        .map(|(key, value)| {
            if same_key(&key, previous) {
                (name.clone(), definition.clone())
            } else {
                (key, value)
            }
        })
        .collect();
    Ok(())
}

/// Writes `steps` into a snippet's `definition`, as [`parse`] reads them:
/// a step that sends a line and waits for the prompt as the line alone.
fn write_steps(definition: &mut Mapping, steps: &Steps) {
    if steps.timeout_secs != DEFAULT_WAIT_TIMEOUT_SECS {
        definition.insert(TIMEOUT.into(), u64::from(steps.timeout_secs).into());
    }
    if !steps.answers.is_empty() {
        let answers: Mapping = steps
            .answers
            .iter()
            .map(|answer| (answer.when.as_str().into(), answer.send.as_str().into()))
            .collect();
        definition.insert(ANSWER.into(), Value::Mapping(answers));
    }
    if !steps.stops.is_empty() {
        let stops = steps
            .stops
            .iter()
            .map(|stop| stop.as_str().into())
            .collect();
        definition.insert(STOP.into(), Value::Sequence(stops));
    }
    let items = steps
        .steps
        .iter()
        .map(|step| {
            if let (Some(line), Wait::Prompt, None) = (&step.send, &step.wait, step.timeout_secs) {
                return Value::from(line.as_str());
            }
            let mut fields = Mapping::new();
            if let Some(line) = &step.send {
                fields.insert(SEND.into(), line.as_str().into());
            }
            let wait = match &step.wait {
                Wait::Prompt if step.send.is_some() => None,
                Wait::Prompt => Some(WAIT_PROMPT.to_string()),
                Wait::Nothing => Some(WAIT_NONE.to_string()),
                // A pattern spelled as a keyword is written as the regular
                // expression of the same text, so it reads back the same.
                Wait::For(pattern) if pattern == WAIT_PROMPT || pattern == WAIT_NONE => {
                    Some(format!("/{pattern}/"))
                }
                Wait::For(pattern) => Some(pattern.clone()),
            };
            if let Some(wait) = wait {
                fields.insert(WAIT.into(), wait.into());
            }
            if let Some(seconds) = step.timeout_secs {
                fields.insert(TIMEOUT.into(), u64::from(seconds).into());
            }
            Value::Mapping(fields)
        })
        .collect();
    definition.insert(STEPS.into(), Value::Sequence(items));
}

/// Removes the entry called `name` (ignoring case); false if there is none.
pub(super) fn remove(document: &mut Mapping, name: &str) -> bool {
    let key = document.keys().find(|key| same_key(key, name)).cloned();
    key.is_some_and(|key| document.shift_remove(&key).is_some())
}

fn same_key(key: &Value, name: &str) -> bool {
    scalar_text(key).is_some_and(|key| same(&key, name))
}

/// Snippet names are the same ignoring case and surrounding blanks.
pub(super) fn same(a: &str, b: &str) -> bool {
    crate::names::same_trimmed_name(a, b)
}
