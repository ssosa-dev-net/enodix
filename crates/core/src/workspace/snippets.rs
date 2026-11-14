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

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use super::scalar_text;
use crate::template::{MissingVariables, Template, Variables};

/// The file at the workspace root holding the snippets.
pub const SNIPPETS_FILE: &str = "_snippets.yaml";

const TEXT: &str = "text";
const GROUP: &str = "group";

/// A named piece of text to send to a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    /// Unique in the workspace, ignoring case.
    pub name: String,
    /// What the app lists it under; `None`: no group.
    #[serde(default)]
    pub group: Option<String>,
    /// A template; each line is sent as a line of its own.
    pub text: String,
}

impl Snippet {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            group: None,
            text: text.into(),
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
        Template::parse(&self.text).variables()
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
        let name = self.name.to_lowercase();
        let group = self.group.as_deref().unwrap_or_default().to_lowercase();
        let text = self.text.to_lowercase();
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
    for (key, value) in fields {
        match scalar_text(key).as_deref() {
            Some(TEXT) => {
                text = Some(scalar_text(value).ok_or_else(|| format!("`{TEXT}` must be text"))?);
            }
            Some(GROUP) if value.is_null() => {}
            Some(GROUP) => {
                group = Some(scalar_text(value).ok_or_else(|| format!("`{GROUP}` must be text"))?);
            }
            Some(other) => return Err(format!("unknown key `{other}`")),
            None => return Err("keys must be text".into()),
        }
    }
    let text = text.ok_or_else(|| format!("missing `{TEXT}`"))?;
    Ok(Snippet {
        name: name.to_string(),
        group,
        text,
    })
}

/// `snippet` with its name and group trimmed (a blank group is none), if it
/// can be saved; else why not.
pub(super) fn clean(snippet: &Snippet) -> Result<Snippet, String> {
    let name = snippet.name.trim();
    if name.is_empty() {
        return Err("it has no name".into());
    }
    if snippet.text.trim().is_empty() {
        return Err("it has no text".into());
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
    definition.insert(TEXT.into(), snippet.text.as_str().into());
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
    a.trim().to_lowercase() == b.trim().to_lowercase()
}
