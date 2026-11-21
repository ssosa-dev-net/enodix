//! Saved jobs: a job saved under a name, kept in [`JOBS_FILE`] at the
//! workspace root (so the team shares them through git) as a mapping of
//! names to the step and the target:
//!
//! ```yaml
//! Madrid versions:
//!   snippet: Show version     # a snippet of the workspace, by name
//!   folder: Madrid            # every host under a folder ("" is the root)
//! Core interfaces:
//!   snippet: Interface
//!   answers:                  # optional: values for variables no host fills
//!     interface: Ethernet1
//!   filter: Core              # the hosts of a saved filter, by name
//! Check r1:
//!   script: scripts/check.py  # a Python script, relative to the root
//!   host: Madrid/r1.yaml      # one host, by id
//! ```
//!
//! Each entry has exactly one step (`snippet` or `script`) and one target
//! (`folder`, `filter` or `host`). An entry defined badly is a problem on
//! the file and is left out; one whose snippet, script, folder, filter or
//! host is gone still loads, and says so when it is run (see
//! [`crate::job::saved_job_problem`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use super::{scalar_text, HostId};
use crate::job::{JobStep, JobTarget};

/// The file at the workspace root holding the saved jobs.
pub const JOBS_FILE: &str = "_jobs.yaml";

const SNIPPET: &str = "snippet";
const SCRIPT: &str = "script";
const ANSWERS: &str = "answers";
const FOLDER: &str = "folder";
const FILTER: &str = "filter";
const HOST: &str = "host";

/// A job saved under a name: what it does and on which hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedJob {
    /// Unique in the workspace, ignoring case.
    pub name: String,
    /// A snippet (with the answers to keep, if any) or a script; never a
    /// config backup.
    pub step: JobStep,
    /// A folder, a saved filter or exactly one host.
    pub target: JobTarget,
    /// The variables whose answers the jobs file holds but a saved job
    /// never uses, because their names look like secrets (#126, see
    /// [`secret_answers`]): asked at each run instead. Names only; the
    /// answers are never read into the job. Never written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_answers: Vec<String>,
}

impl SavedJob {
    /// A saved job with no ignored answers.
    pub fn new(name: impl Into<String>, step: JobStep, target: JobTarget) -> Self {
        Self {
            name: name.into(),
            step,
            target,
            ignored_answers: Vec::new(),
        }
    }
}

/// The variables of `step`'s answers that a saved job does not keep,
/// because their names look like secrets (a password, a secret, a key, a
/// token, a community...), by the rule the CSV and Nornir imports use for
/// fields: they are asked at each run. Empty for anything but a snippet.
pub fn secret_answers(step: &JobStep) -> Vec<String> {
    match step {
        JobStep::Snippet { answers, .. } => answers
            .keys()
            .filter(|name| crate::import::is_secret_name(name))
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

/// `answers` without those whose names look like secrets.
fn kept_answers(answers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    answers
        .iter()
        .filter(|(name, _)| !crate::import::is_secret_name(name))
        .map(|(name, answer)| (name.clone(), answer.clone()))
        .collect()
}

/// The saved jobs of the jobs file (`document`), in file order, and a
/// message for each one defined badly (it is left out).
pub(super) fn load(document: &Mapping) -> (Vec<SavedJob>, Vec<String>) {
    let mut jobs: Vec<SavedJob> = Vec::new();
    let mut errors = Vec::new();
    for (key, definition) in document {
        let Some(name) = scalar_text(key) else {
            errors.push("saved job names must be text".into());
            continue;
        };
        let name = name.trim();
        match parse(name, definition).and_then(|job| clean(&job)) {
            Ok(job) if jobs.iter().any(|seen| same(&seen.name, &job.name)) => {
                errors.push(format!("saved job `{name}` is defined twice"));
            }
            Ok(job) => jobs.push(job),
            Err(message) => errors.push(format!("saved job `{name}`: {message}")),
        }
    }
    (jobs, errors)
}

fn parse(name: &str, definition: &Value) -> Result<SavedJob, String> {
    let Value::Mapping(fields) = definition else {
        return Err(format!(
            "expected `{SNIPPET}` or `{SCRIPT}`, and `{FOLDER}`, `{FILTER}` or `{HOST}`"
        ));
    };
    let mut steps = Vec::new();
    let mut targets = Vec::new();
    let mut answers = None;
    for (key, value) in fields {
        let key = scalar_text(key).ok_or("keys must be text")?;
        let text = || scalar_text(value).ok_or_else(|| format!("`{key}` must be text"));
        match key.as_str() {
            SNIPPET => steps.push(JobStep::Snippet {
                name: text()?,
                answers: BTreeMap::new(),
            }),
            SCRIPT => steps.push(JobStep::Script { path: text()? }),
            FOLDER => targets.push(JobTarget::Folder {
                path: folder_text(value).ok_or_else(|| format!("`{key}` must be text"))?,
            }),
            FILTER => targets.push(JobTarget::Filter { name: text()? }),
            HOST => targets.push(JobTarget::Hosts {
                ids: vec![HostId::new(text()?)],
            }),
            ANSWERS => answers = Some(parse_answers(value)?),
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    let mut step = match <[JobStep; 1]>::try_from(steps) {
        Ok([step]) => step,
        Err(steps) if steps.is_empty() => return Err(format!("missing `{SNIPPET}` or `{SCRIPT}`")),
        Err(_) => return Err(format!("give `{SNIPPET}` or `{SCRIPT}`, not both")),
    };
    let target = match <[JobTarget; 1]>::try_from(targets) {
        Ok([target]) => target,
        Err(targets) if targets.is_empty() => {
            return Err(format!("missing `{FOLDER}`, `{FILTER}` or `{HOST}`"))
        }
        Err(_) => {
            return Err(format!(
                "give one of `{FOLDER}`, `{FILTER}` and `{HOST}`, not several"
            ))
        }
    };
    let mut ignored_answers = Vec::new();
    if let Some(given) = answers {
        let JobStep::Snippet { answers, .. } = &mut step else {
            return Err(format!("`{ANSWERS}` only go with a `{SNIPPET}`"));
        };
        // A secret-looking answer in the file (saved before #126, or
        // written by hand) is left in the file but never used.
        ignored_answers = given
            .keys()
            .filter(|name| crate::import::is_secret_name(name))
            .cloned()
            .collect();
        *answers = kept_answers(&given);
    }
    Ok(SavedJob {
        name: name.to_string(),
        step,
        target,
        ignored_answers,
    })
}

/// A `folder` target's path: `folder:` alone is the root.
fn folder_text(value: &Value) -> Option<String> {
    if value.is_null() {
        Some(String::new())
    } else {
        scalar_text(value)
    }
}

/// Points the entries of the jobs file's `document` whose `host` names a
/// moved host, or whose `folder` is a moved folder, at the new path: `host`
/// and `folder` give it, if the value moved. Entries defined badly are
/// changed too. Returns whether anything changed.
pub(super) fn retarget(
    document: &mut Mapping,
    host: impl Fn(&str) -> Option<String>,
    folder: impl Fn(&str) -> Option<String>,
) -> bool {
    let mut changed = false;
    for (_, entry) in document.iter_mut() {
        let Value::Mapping(entry) = entry else {
            continue;
        };
        for (key, moved) in [
            (
                HOST,
                entry.get(HOST).and_then(scalar_text).and_then(|v| host(&v)),
            ),
            (
                FOLDER,
                entry
                    .get(FOLDER)
                    .and_then(folder_text)
                    .and_then(|v| folder(&v)),
            ),
        ] {
            if let Some(to) = moved {
                entry.insert(key.into(), to.into());
                changed = true;
            }
        }
    }
    changed
}

fn parse_answers(value: &Value) -> Result<BTreeMap<String, String>, String> {
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let Value::Mapping(entries) = value else {
        return Err(format!("`{ANSWERS}` must map variable names to values"));
    };
    entries
        .iter()
        .map(|(name, answer)| {
            let name = scalar_text(name)
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("`{ANSWERS}` must map variable names to values"))?;
            let answer = if answer.is_null() {
                String::new()
            } else {
                scalar_text(answer)
                    .ok_or_else(|| format!("the answer for `{name}` must be text"))?
            };
            Ok((name, answer))
        })
        .collect()
}

/// `job` with its name, snippet name, script path and target trimmed (a
/// folder path or script path with `/` separators), if it can be saved;
/// else why not.
pub(super) fn clean(job: &SavedJob) -> Result<SavedJob, String> {
    let name = job.name.trim();
    if name.is_empty() {
        return Err("it has no name".into());
    }
    let step = match &job.step {
        JobStep::Snippet { name, answers } => {
            let name = name.trim();
            if name.is_empty() {
                return Err("it names no snippet".into());
            }
            JobStep::Snippet {
                name: name.to_string(),
                // Never kept: asked at each run (#126).
                answers: kept_answers(answers),
            }
        }
        JobStep::Script { path } => {
            let path = path.trim().replace('\\', "/");
            if path.is_empty() {
                return Err("it names no script".into());
            }
            JobStep::Script { path }
        }
        JobStep::ConfigBackup => {
            return Err("a config backup cannot be saved as a job".into());
        }
        JobStep::Enrich => {
            return Err("filling in from devices cannot be saved as a job".into());
        }
        JobStep::SendFile { .. } => {
            return Err(
                "sending a file cannot be saved as a job (it names a file on this computer)".into(),
            );
        }
    };
    let target =
        match &job.target {
            JobTarget::Folder { path } => JobTarget::Folder {
                path: path.trim().replace('\\', "/").trim_matches('/').to_string(),
            },
            JobTarget::Filter { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err("it names no filter".into());
                }
                JobTarget::Filter {
                    name: name.to_string(),
                }
            }
            JobTarget::Hosts { ids } => match ids.as_slice() {
                [id] if !id.as_str().trim().is_empty() => JobTarget::Hosts {
                    ids: vec![HostId::new(id.as_str().trim().replace('\\', "/"))],
                },
                [_] => return Err("it names no host".into()),
                _ => return Err(
                    "a saved job runs on a folder, a saved filter or one host, not a list of hosts"
                        .into(),
                ),
            },
        };
    Ok(SavedJob {
        name: name.to_string(),
        step,
        target,
        ignored_answers: job.ignored_answers.clone(),
    })
}

/// A saved job's definition as written in the jobs file.
fn to_value(job: &SavedJob) -> Value {
    let mut definition = Mapping::new();
    match &job.step {
        JobStep::Snippet { name, answers } => {
            definition.insert(SNIPPET.into(), name.as_str().into());
            if !answers.is_empty() {
                let answers: Mapping = answers
                    .iter()
                    .map(|(name, answer)| (name.as_str().into(), answer.as_str().into()))
                    .collect();
                definition.insert(ANSWERS.into(), Value::Mapping(answers));
            }
        }
        JobStep::Script { path } => {
            definition.insert(SCRIPT.into(), path.as_str().into());
        }
        // Refused by `clean`.
        JobStep::ConfigBackup | JobStep::Enrich | JobStep::SendFile { .. } => {}
    }
    match &job.target {
        JobTarget::Folder { path } => definition.insert(FOLDER.into(), path.as_str().into()),
        JobTarget::Filter { name } => definition.insert(FILTER.into(), name.as_str().into()),
        JobTarget::Hosts { ids } => {
            definition.insert(HOST.into(), ids.first().map_or("", HostId::as_str).into())
        }
    };
    Value::Mapping(definition)
}

/// Writes `job` into the jobs file's `document`: in place of the entry
/// called `previous`, else after the others. Refused (with the name) when
/// another entry has its name.
pub(super) fn write(
    document: &mut Mapping,
    previous: Option<&str>,
    job: &SavedJob,
) -> Result<(), String> {
    // An entry of that name that is not the one replaced (one defined badly,
    // so not loaded) is never written over.
    let taken = document.keys().any(|key| {
        same_key(key, &job.name) && previous.is_none_or(|previous| !same_key(key, previous))
    });
    if taken {
        return Err(job.name.clone());
    }
    let name = Value::from(job.name.as_str());
    let mut definition = to_value(job);
    let Some(previous) =
        previous.filter(|previous| document.keys().any(|key| same_key(key, previous)))
    else {
        document.insert(name, definition);
        return Ok(());
    };
    // The secret-looking answers the replaced entry holds (unused, #126)
    // stay as they were: the app never removes them silently; the user does.
    if let Some(old) = document
        .iter()
        .find(|(key, _)| same_key(key, previous))
        .map(|(_, value)| value)
    {
        keep_ignored_answers(old, &mut definition);
    }
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

/// Copies into the snippet entry `definition` the answers of the entry
/// `old` whose names look like secrets, as written there.
fn keep_ignored_answers(old: &Value, definition: &mut Value) {
    let Some(Value::Mapping(old)) = old.get(ANSWERS) else {
        return;
    };
    let kept: Vec<(Value, Value)> = old
        .iter()
        .filter(|(name, _)| {
            scalar_text(name).is_some_and(|name| crate::import::is_secret_name(name.trim()))
        })
        .map(|(name, answer)| (name.clone(), answer.clone()))
        .collect();
    let Value::Mapping(definition) = definition else {
        return;
    };
    if kept.is_empty() || !definition.contains_key(SNIPPET) {
        return;
    }
    let answers = definition
        .entry(ANSWERS.into())
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if let Value::Mapping(answers) = answers {
        answers.extend(kept);
    }
}

/// Removes the entry called `name` (ignoring case); false if there is none.
pub(super) fn remove(document: &mut Mapping, name: &str) -> bool {
    let key = document.keys().find(|key| same_key(key, name)).cloned();
    key.is_some_and(|key| document.shift_remove(&key).is_some())
}

fn same_key(key: &Value, name: &str) -> bool {
    scalar_text(key).is_some_and(|key| same(&key, name))
}

/// Saved job names are the same ignoring case and surrounding blanks.
pub(super) fn same(a: &str, b: &str) -> bool {
    crate::names::same_trimmed_name(a, b)
}
