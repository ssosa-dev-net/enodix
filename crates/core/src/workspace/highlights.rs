//! The workspace's highlight rules, in [`HIGHLIGHTS_FILE`] at its root: a
//! mapping of rule names to their definition.
//!
//! ```yaml
//! bgp-idle:
//!   pattern: '\bIdle\b'   # a regular expression (`(?i)` ignores case)
//!   color: '#ff8800'      # #rrggbb, #rgb, or a terminal colour: red, bright-cyan, ...
//!   enabled: false        # optional; rules are on unless turned off
//! ipv6:                   # a built-in rule's name: changes that rule
//!   enabled: false        # (any of the three keys; the rest stays as built in)
//! ```
//!
//! The workspace's own rules come first, in file order, then the built-in
//! ones; where matches overlap the earlier rule wins. A rule defined badly is
//! a problem on the file and is left out. A rule whose pattern cannot be used
//! is a problem too, but stays listed (with why), so it can be fixed in the
//! app; it highlights nothing and the other rules are not affected.

use serde_norway::{Mapping, Value};

use super::scalar_text;
use crate::highlight::{
    built_in, built_in_rules, check_pattern, parse_color, BuiltInRule, HighlightRule,
    HighlightSource, ListedRule, COLOR_NAMES,
};

/// The file at the workspace root holding its highlight rules.
pub const HIGHLIGHTS_FILE: &str = "_highlights.yaml";

const PATTERN: &str = "pattern";
const COLOR: &str = "color";
const ENABLED: &str = "enabled";

/// The rules of the highlights file (`document`) and the built-in ones, in
/// order, and a message for each problem in it (file order).
pub(super) fn load(document: &Mapping) -> (Vec<ListedRule>, Vec<String>) {
    let mut own = Vec::new();
    let mut built: Vec<ListedRule> = built_in_rules();
    let mut errors = Vec::new();
    for (key, definition) in document {
        let Some(name) = scalar_text(key).map(|name| name.trim().to_string()) else {
            errors.push("highlight rule names must be text".into());
            continue;
        };
        if name.is_empty() {
            errors.push("a highlight rule has an empty name".into());
            continue;
        }
        let parsed = match built_in(&name) {
            Some(original) => changed(original, definition),
            None => own_rule(&name, definition),
        };
        let rule = match parsed {
            Ok(rule) => rule,
            Err(message) => {
                errors.push(format!("highlight rule `{name}`: {message}"));
                continue;
            }
        };
        let error = check_pattern(&rule.pattern).err();
        if let Some(error) = &error {
            errors.push(format!("highlight rule `{name}`: {error}"));
        }
        if built_in(&name).is_some() {
            let listed = built
                .iter_mut()
                .find(|listed| listed.rule.name == rule.name)
                .expect("every built-in rule is listed");
            if listed.source == HighlightSource::Changed {
                errors.push(format!("highlight rule `{name}` is defined twice"));
                continue;
            }
            *listed = ListedRule {
                rule,
                source: HighlightSource::Changed,
                error,
            };
        } else if own
            .iter()
            .any(|listed: &ListedRule| same(&listed.rule.name, &name))
        {
            errors.push(format!("highlight rule `{name}` is defined twice"));
        } else {
            own.push(ListedRule {
                rule,
                source: HighlightSource::Workspace,
                error,
            });
        }
    }
    own.extend(built);
    (own, errors)
}

/// The keys of a definition, refusing a non-mapping and unknown keys.
fn fields(definition: &Value) -> Result<&Mapping, String> {
    let Value::Mapping(fields) = definition else {
        return Err(format!("expected `{PATTERN}` and `{COLOR}`"));
    };
    for key in fields.keys() {
        match scalar_text(key).as_deref() {
            Some(PATTERN | COLOR | ENABLED) => {}
            Some(other) => return Err(format!("unknown key `{other}`")),
            None => return Err("keys must be text".into()),
        }
    }
    Ok(fields)
}

fn pattern(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("`{PATTERN}` must be text"))
}

fn color(value: &Value) -> Result<String, String> {
    value.as_str().and_then(parse_color).ok_or_else(|| {
        format!(
            "`{COLOR}` must be a colour like \"#ff8800\" or one of {}",
            COLOR_NAMES.join(", ")
        )
    })
}

fn enabled(value: &Value) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("`{ENABLED}` must be true or false"))
}

/// A workspace rule: pattern and colour required.
fn own_rule(name: &str, definition: &Value) -> Result<HighlightRule, String> {
    let fields = fields(definition)?;
    let required = |key: &str| {
        fields
            .get(key)
            .filter(|value| !value.is_null())
            .ok_or_else(|| format!("missing `{key}`"))
    };
    Ok(HighlightRule {
        name: name.to_string(),
        pattern: pattern(required(PATTERN)?)?,
        color: color(required(COLOR)?)?,
        enabled: match fields.get(ENABLED) {
            None | Some(Value::Null) => true,
            Some(value) => enabled(value)?,
        },
    })
}

/// A built-in rule with what the file changes.
fn changed(original: &BuiltInRule, definition: &Value) -> Result<HighlightRule, String> {
    let mut rule = original.rule();
    if definition.is_null() {
        return Ok(rule);
    }
    let fields = fields(definition)?;
    let given = |key: &str| fields.get(key).filter(|value| !value.is_null());
    if let Some(value) = given(PATTERN) {
        rule.pattern = pattern(value)?;
    }
    if let Some(value) = given(COLOR) {
        rule.color = color(value)?;
    }
    if let Some(value) = given(ENABLED) {
        rule.enabled = enabled(value)?;
    }
    Ok(rule)
}

/// `rule` with its name trimmed and colour normalised, if it can be saved;
/// else why not. A rule that is on needs a pattern that can be used.
pub(super) fn clean(rule: &HighlightRule) -> Result<HighlightRule, String> {
    let name = rule.name.trim();
    if name.is_empty() {
        return Err("the highlight rule has no name".into());
    }
    // A rule turned off may keep a broken pattern (to be fixed later).
    if rule.enabled {
        check_pattern(&rule.pattern)?;
    }
    let color = parse_color(&rule.color).ok_or_else(|| {
        format!(
            "\"{}\" is not a colour: use one like \"#ff8800\" or one of {}",
            rule.color.trim(),
            COLOR_NAMES.join(", ")
        )
    })?;
    Ok(HighlightRule {
        name: built_in(name).map_or(name, |built| built.name).to_string(),
        pattern: rule.pattern.clone(),
        color,
        enabled: rule.enabled,
    })
}

/// Writes `rule` into the highlights file's `document`: in place of the
/// entry called `previous` (or `rule`'s own name), else after the others. A
/// built-in rule gets only what differs from the built-in one, and no entry
/// when nothing does.
pub(super) fn write(document: &mut Mapping, previous: &str, rule: &HighlightRule) {
    let definition = match built_in(&rule.name) {
        Some(original) => {
            let original = original.rule();
            let mut changes = Mapping::new();
            if rule.pattern != original.pattern {
                changes.insert(PATTERN.into(), rule.pattern.as_str().into());
            }
            if rule.color != original.color {
                changes.insert(COLOR.into(), rule.color.as_str().into());
            }
            if !rule.enabled {
                changes.insert(ENABLED.into(), false.into());
            }
            if changes.is_empty() {
                remove(document, previous);
                return;
            }
            changes
        }
        None => {
            let mut definition = Mapping::new();
            definition.insert(PATTERN.into(), rule.pattern.as_str().into());
            definition.insert(COLOR.into(), rule.color.as_str().into());
            if !rule.enabled {
                definition.insert(ENABLED.into(), false.into());
            }
            definition
        }
    };
    let name = Value::from(rule.name.as_str());
    let definition = Value::Mapping(definition);
    if !document.keys().any(|key| same_key(key, previous)) {
        document.insert(name, definition);
        return;
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
}

/// Removes the entry called `name` (ignoring case); false if there is none.
pub(super) fn remove(document: &mut Mapping, name: &str) -> bool {
    let key = document.keys().find(|key| same_key(key, name)).cloned();
    key.is_some_and(|key| document.shift_remove(&key).is_some())
}

fn same_key(key: &Value, name: &str) -> bool {
    scalar_text(key).is_some_and(|key| same(&key, name))
}

/// Rule names are the same ignoring case and surrounding blanks.
pub(super) fn same(a: &str, b: &str) -> bool {
    crate::names::same_trimmed_name(a, b)
}
