//! The workspace's platform definitions, in [`PLATFORMS_FILE`] at its root:
//! a mapping of definition names to their values.
//!
//! ```yaml
//! mikrotik:
//!   prompt: '\[\S+@\S+\] >'       # required: a regular expression
//!   other_names: [routeros]       # optional: a name or a list of names
//!   disable_paging: ...           # optional commands and patterns:
//!   privilege_command: enable     #   disable_paging, privilege_command,
//!   privilege_prompt: 'Password:' #   privilege_prompt, show_config,
//!   show_config: /export          #   ignored_lines
//! eos:                            # a built-in name: changes only the keys
//!   disable_paging: terminal pager 0  # given ('' clears a built-in value)
//! ```
//!
//! The built-in definitions come first, then the workspace's own in file
//! order. A definition defined badly is a problem on the file and is left
//! out. A definition with a pattern that cannot be used is a problem too,
//! but stays listed (with why), so it can be fixed in the app; it is not
//! used.

use serde_norway::{Mapping, Value};

use super::scalar_text;
use crate::platform::{
    built_in_platform, built_in_platforms, same, BuiltInPlatform, ListedPlatform,
    PlatformDefinition, PlatformSource, DISABLE_PAGING, IGNORED_LINES, OTHER_NAMES,
    PRIVILEGE_COMMAND, PRIVILEGE_PROMPT, PROMPT, SHOW_CONFIG,
};

/// The file at the workspace root holding its platform definitions.
pub const PLATFORMS_FILE: &str = "_platforms.yaml";

/// The optional text values, in file order after the prompt.
const OPTIONAL: [&str; 5] = [
    DISABLE_PAGING,
    PRIVILEGE_COMMAND,
    PRIVILEGE_PROMPT,
    SHOW_CONFIG,
    IGNORED_LINES,
];

/// The definitions of the platforms file (`document`) and the built-in
/// ones, in order, and a message for each problem in it (file order).
pub(super) fn load(document: &Mapping) -> (Vec<ListedPlatform>, Vec<String>) {
    let mut listed = built_in_platforms();
    let mut errors = Vec::new();
    for (key, values) in document {
        let Some(name) = scalar_text(key).map(|name| name.trim().to_string()) else {
            errors.push("platform names must be text".into());
            continue;
        };
        if name.is_empty() {
            errors.push("a platform has an empty name".into());
            continue;
        }
        let original = built_in_platform(&name);
        let parsed = match original {
            Some(original) => changed(original, values),
            None => own(&name, values),
        };
        let definition = match parsed {
            Ok(definition) => definition,
            Err(message) => {
                errors.push(format!("platform `{name}`: {message}"));
                continue;
            }
        };
        let error = definition.check().err();
        if let Some(error) = &error {
            errors.push(format!("platform `{name}`: {error}"));
        }
        let entry = ListedPlatform {
            definition,
            source: if original.is_some() {
                PlatformSource::Changed
            } else {
                PlatformSource::Workspace
            },
            error,
        };
        match listed
            .iter_mut()
            .find(|listed| listed.definition.is_named(&name))
        {
            Some(existing) if existing.source == PlatformSource::BuiltIn => *existing = entry,
            Some(_) => errors.push(format!("platform `{name}` is defined twice")),
            None => listed.push(entry),
        }
    }
    // Other names that find another definition (save refuses them).
    for (at, entry) in listed.iter().enumerate() {
        for other in &entry.definition.other_names {
            let taken = listed.iter().enumerate().find(|(before, earlier)| {
                *before != at
                    && (earlier.definition.is_named(other)
                        || (*before < at && earlier.definition.answers_to(other)))
            });
            if let Some((_, earlier)) = taken {
                errors.push(format!(
                    "platform `{}`: the other name `{other}` finds platform `{}`",
                    entry.definition.name, earlier.definition.name
                ));
            }
        }
    }
    (listed, errors)
}

/// The keys of a definition, refusing a non-mapping and unknown keys.
fn fields(values: &Value) -> Result<&Mapping, String> {
    let Value::Mapping(fields) = values else {
        return Err(format!("expected a mapping with at least `{PROMPT}`"));
    };
    for key in fields.keys() {
        match scalar_text(key).as_deref() {
            Some(key) if key == OTHER_NAMES || key == PROMPT || OPTIONAL.contains(&key) => {}
            Some(other) => return Err(format!("unknown key `{other}`")),
            None => return Err("keys must be text".into()),
        }
    }
    Ok(fields)
}

/// A text value (`None` for null); a blank one is `Some("")`.
fn text(fields: &Mapping, key: &str) -> Result<Option<String>, String> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(Value::Number(number)) => Ok(Some(number.to_string())),
        Some(_) => Err(format!("`{key}` must be text")),
    }
}

/// The other names (`None` when not given): a name or a list of names.
fn other_names(fields: &Mapping) -> Result<Option<Vec<String>>, String> {
    let names = match fields.get(OTHER_NAMES) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Sequence(items)) => items
            .iter()
            .map(|item| scalar_text(item).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|()| format!("`{OTHER_NAMES}` must be a name or a list of names"))?,
        Some(value) => vec![scalar_text(value)
            .ok_or_else(|| format!("`{OTHER_NAMES}` must be a name or a list of names"))?],
    };
    Ok(Some(clean_names(&names)))
}

/// Names trimmed, blank ones dropped, each once (ignoring case).
fn clean_names(names: &[String]) -> Vec<String> {
    let mut clean: Vec<String> = Vec::new();
    for name in names.iter().map(|name| name.trim()) {
        if !name.is_empty() && !clean.iter().any(|kept| same(kept, name)) {
            clean.push(name.to_string());
        }
    }
    clean
}

/// A blank value is none.
fn optional(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// The optional values of `definition`, in [`OPTIONAL`] order.
fn optional_values(definition: &mut PlatformDefinition) -> [&mut Option<String>; 5] {
    [
        &mut definition.disable_paging,
        &mut definition.privilege_command,
        &mut definition.privilege_prompt,
        &mut definition.show_config,
        &mut definition.ignored_lines,
    ]
}

/// A workspace definition: the prompt required.
fn own(name: &str, values: &Value) -> Result<PlatformDefinition, String> {
    let fields = fields(values)?;
    let prompt = optional(text(fields, PROMPT)?).ok_or_else(|| format!("missing `{PROMPT}`"))?;
    let mut definition = PlatformDefinition {
        name: name.to_string(),
        other_names: other_names(fields)?.unwrap_or_default(),
        prompt,
        disable_paging: None,
        privilege_command: None,
        privilege_prompt: None,
        show_config: None,
        ignored_lines: None,
    };
    for (key, value) in OPTIONAL.into_iter().zip(optional_values(&mut definition)) {
        *value = optional(text(fields, key)?);
    }
    Ok(definition)
}

/// A built-in definition with what the file changes.
fn changed(original: &BuiltInPlatform, values: &Value) -> Result<PlatformDefinition, String> {
    let mut definition = original.definition();
    if values.is_null() {
        return Ok(definition);
    }
    let fields = fields(values)?;
    if let Some(names) = other_names(fields)? {
        definition.other_names = names;
    }
    if let Some(prompt) = text(fields, PROMPT)? {
        if prompt.trim().is_empty() {
            return Err(format!("`{PROMPT}` cannot be cleared"));
        }
        definition.prompt = prompt;
    }
    for (key, value) in OPTIONAL.into_iter().zip(optional_values(&mut definition)) {
        if let Some(given) = text(fields, key)? {
            *value = optional(Some(given));
        }
    }
    Ok(definition)
}

/// `definition` with its name and other names trimmed and blank values
/// dropped, if it can be saved; else why not.
pub(super) fn clean(definition: &PlatformDefinition) -> Result<PlatformDefinition, String> {
    let name = definition.name.trim();
    if name.is_empty() {
        return Err("the platform has no name".into());
    }
    let mut clean = PlatformDefinition {
        name: built_in_platform(name)
            .map_or(name, |built| built.name)
            .to_string(),
        other_names: clean_names(&definition.other_names)
            .into_iter()
            .filter(|other| !same(other, name))
            .collect(),
        ..definition.clone()
    };
    for value in optional_values(&mut clean) {
        *value = optional(value.take());
    }
    if clean.prompt.trim().is_empty() {
        return Err(format!("the platform {name} needs a prompt pattern"));
    }
    clean
        .check()
        .map_err(|error| format!("the platform {name} cannot be saved: {error}"))?;
    Ok(clean)
}

/// Writes `definition` into the platforms file's `document`: in place of
/// the entry called `previous` (or `definition`'s own name), else after the
/// others. A built-in definition gets only what differs from the built-in
/// one (a cleared value as `''`), and no entry when nothing does.
pub(super) fn write(document: &mut Mapping, previous: &str, definition: &PlatformDefinition) {
    let mut values = Mapping::new();
    let mut definition = definition.clone();
    let name = definition.name.clone();
    match built_in_platform(&name) {
        Some(original) => {
            let mut original = original.definition();
            if definition.other_names != original.other_names {
                values.insert(OTHER_NAMES.into(), names_value(&definition.other_names));
            }
            if definition.prompt != original.prompt {
                values.insert(PROMPT.into(), definition.prompt.as_str().into());
            }
            for ((key, value), built) in OPTIONAL
                .into_iter()
                .zip(optional_values(&mut definition))
                .zip(optional_values(&mut original))
            {
                if value != built {
                    values.insert(key.into(), value.as_deref().unwrap_or("").into());
                }
            }
            if values.is_empty() {
                remove(document, previous);
                return;
            }
        }
        None => {
            if !definition.other_names.is_empty() {
                values.insert(OTHER_NAMES.into(), names_value(&definition.other_names));
            }
            values.insert(PROMPT.into(), definition.prompt.as_str().into());
            for (key, value) in OPTIONAL.into_iter().zip(optional_values(&mut definition)) {
                if let Some(value) = value {
                    values.insert(key.into(), value.as_str().into());
                }
            }
        }
    }
    let key = Value::from(name.as_str());
    let values = Value::Mapping(values);
    if !document.keys().any(|key| same_key(key, previous)) {
        document.insert(key, values);
        return;
    }
    *document = std::mem::take(document)
        .into_iter()
        .map(|(old, old_values)| {
            if same_key(&old, previous) {
                (key.clone(), values.clone())
            } else {
                (old, old_values)
            }
        })
        .collect();
}

fn names_value(names: &[String]) -> Value {
    Value::Sequence(names.iter().map(|name| name.as_str().into()).collect())
}

/// Removes the entry called `name` (ignoring case); false if there is none.
pub(super) fn remove(document: &mut Mapping, name: &str) -> bool {
    let key = document.keys().find(|key| same_key(key, name)).cloned();
    key.is_some_and(|key| document.shift_remove(&key).is_some())
}

fn same_key(key: &Value, name: &str) -> bool {
    scalar_text(key).is_some_and(|key| same(&key, name))
}
