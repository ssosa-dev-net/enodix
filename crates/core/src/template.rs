//! The templating shared by snippets and web links: text with variables
//! written `{{name}}`, filled from a resolved host's fields
//! ([`crate::workspace::ResolvedHost::variables`]) and, for what the host
//! has no value for, from what the user is asked.
//!
//! A variable's name is the text between `{{` and `}}`, without the blanks
//! around it, and is matched ignoring case: `{{address}}`, `{{ Site }}`,
//! `{{custom.circuit}}`, `{{vlan}}`. Braces that do not make a variable (an
//! unclosed `{{`, `{{}}`, a name holding a brace or a line break) are kept
//! as written. A filled-in value is inserted as it is; it is never read for
//! variables itself.

/// Text with variables, parsed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    /// A variable's name, as written (trimmed).
    Variable(String),
}

const OPEN: &str = "{{";
const CLOSE: &str = "}}";

impl Template {
    pub fn parse(text: &str) -> Self {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut rest = text;
        while let Some(start) = rest.find(OPEN) {
            let after = &rest[start + OPEN.len()..];
            let variable = after.find(CLOSE).and_then(|end| {
                let name = after[..end].trim();
                let valid = !name.is_empty() && !name.contains(['{', '}', '\n', '\r']);
                valid.then_some((name, end))
            });
            match variable {
                Some((name, end)) => {
                    literal.push_str(&rest[..start]);
                    if !literal.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut literal)));
                    }
                    parts.push(Part::Variable(name.to_string()));
                    rest = &after[end + CLOSE.len()..];
                }
                None => {
                    // Not a variable here: keep one brace, look again after it
                    // (so that `{{{a}}}` is a brace and the variable `a`).
                    literal.push_str(&rest[..=start]);
                    rest = &rest[start + 1..];
                }
            }
        }
        literal.push_str(rest);
        if !literal.is_empty() {
            parts.push(Part::Text(literal));
        }
        Self { parts }
    }

    /// The names of the variables, once each (ignoring case), in the order
    /// they first appear, as first written.
    pub fn variables(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for part in &self.parts {
            if let Part::Variable(name) = part {
                if !names.iter().any(|seen| same_name(seen, name)) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// The text with every variable replaced by its value in `values`; if
    /// some have none, which ones (and no text).
    pub fn fill(&self, values: &Variables) -> Result<String, MissingVariables> {
        let missing: Vec<String> = self
            .variables()
            .into_iter()
            .filter(|name| values.get(name).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(MissingVariables { names: missing });
        }
        let mut text = String::new();
        for part in &self.parts {
            match part {
                Part::Text(literal) => text.push_str(literal),
                Part::Variable(name) => text.push_str(values.get(name).unwrap_or_default()),
            }
        }
        Ok(text)
    }
}

/// Values for variables, by name (ignoring case).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Variables {
    values: Vec<(String, String)>,
}

impl Variables {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gives `name` the value `value`, replacing the one it had.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into().trim().to_string();
        let value = value.into();
        match self
            .values
            .iter_mut()
            .find(|(seen, _)| same_name(seen, &name))
        {
            Some(entry) => *entry = (name, value),
            None => self.values.push((name, value)),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(seen, _)| same_name(seen, name.trim()))
            .map(|(_, value)| value.as_str())
    }

    /// Takes the values of `other` for the names that have none here.
    pub fn fill_gaps(&mut self, other: &Variables) {
        for (name, value) in &other.values {
            if self.get(name).is_none() {
                self.values.push((name.clone(), value.clone()));
            }
        }
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Variables {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut variables = Self::new();
        for (name, value) in iter {
            variables.set(name, value);
        }
        variables
    }
}

/// The variables of a template that have no value, in the order they first
/// appear.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no value for {}", .names.join(", "))]
pub struct MissingVariables {
    pub names: Vec<String>,
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}
