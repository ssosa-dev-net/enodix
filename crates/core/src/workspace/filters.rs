//! Saved filters: named views of the inventory, kept in [`FILTERS_FILE`] at
//! the workspace root as a mapping of filter names to their conditions:
//!
//! ```yaml
//! Arista in Madrid:
//!   vendor: arista      # a built-in field, matched whole, ignoring case
//!   site: Madrid
//! Circuits:
//!   custom:             # custom fields by key
//!     circuit: CID-*    # `*` stands for any text
//! Spanish switches:
//!   tags: [spain]       # one tag or a list; inherited tags count
//!   name: "*-sw*"
//! Core:
//!   text: core north    # free text, searched as the palette does
//! ```
//!
//! A host is in a filter's view when it meets every condition. A filter
//! defined badly is a problem on the file and is left out.

use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use super::{
    scalar_text, ADDRESS, BUILDING, CUSTOM, DESCRIPTION, MODEL, NAME, PLATFORM, RACK, RACK_UNIT,
    ROLE, ROOM, SERIAL_NUMBER, SITE, TAGS, VENDOR,
};

/// The file at the workspace root holding the saved filters.
pub const FILTERS_FILE: &str = "_filters.yaml";

/// The condition field of a tag (`tags` in the file).
pub(super) const TAG: &str = "tag";
/// The condition field of free text, searched as the palette does.
pub(super) const TEXT: &str = "text";
/// Condition fields of custom fields are `custom.<key>`.
pub(super) const CUSTOM_PREFIX: &str = "custom.";

/// The built-in host fields a condition can test, as named in files.
const FIELDS: [&str; 13] = [
    NAME,
    ADDRESS,
    VENDOR,
    PLATFORM,
    MODEL,
    SERIAL_NUMBER,
    SITE,
    BUILDING,
    ROOM,
    RACK,
    RACK_UNIT,
    ROLE,
    DESCRIPTION,
];

/// A named view of the inventory: the hosts meeting every condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFilter {
    pub name: String,
    pub conditions: Vec<Condition>,
}

/// One condition of a filter.
///
/// `field` is a built-in host field (`name`, `address`, `vendor`, ...,
/// `role`, matched by role id or name), `tag`, `custom.<key>`, or `text`.
/// `value` matches the field's whole value ignoring case, `*` standing for
/// any text; for `text` it is words that must each be part of some field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub value: String,
}

impl Condition {
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
        }
    }
}

/// The filters of the filters file (`document`), in file order, and a
/// message for each one defined badly (it is left out).
pub(super) fn load(document: &Mapping) -> (Vec<SavedFilter>, Vec<String>) {
    let mut filters = Vec::new();
    let mut errors = Vec::new();
    for (key, definition) in document {
        let Some(name) = scalar_text(key) else {
            errors.push("filter names must be text".into());
            continue;
        };
        match parse(&name, definition).and_then(|filter| clean(&filter)) {
            Ok(filter) => filters.push(filter),
            Err(message) => errors.push(format!("filter `{}`: {message}", name.trim())),
        }
    }
    (filters, errors)
}

fn parse(name: &str, definition: &Value) -> Result<SavedFilter, String> {
    let fields = match definition {
        Value::Mapping(fields) => fields,
        Value::Null => return Err("it has no conditions".into()),
        _ => return Err("expected a mapping of fields to values".into()),
    };
    let text = |key: &str, value: &Value| {
        scalar_text(value).ok_or_else(|| format!("`{key}` must be text"))
    };
    let mut conditions = Vec::new();
    for (key, value) in fields {
        let key = scalar_text(key).ok_or("field names must be text")?;
        match key.as_str() {
            TAGS => match value {
                Value::Sequence(tags) => {
                    for tag in tags {
                        conditions.push(Condition::new(TAG, text(TAGS, tag)?));
                    }
                }
                tag => conditions.push(Condition::new(TAG, text(TAGS, tag)?)),
            },
            CUSTOM => {
                let custom = value
                    .as_mapping()
                    .ok_or_else(|| format!("`{CUSTOM}` must map field names to values"))?;
                for (field, value) in custom {
                    let field = scalar_text(field).ok_or("field names must be text")?;
                    let value = text(&field, value)?;
                    conditions.push(Condition::new(format!("{CUSTOM_PREFIX}{field}"), value));
                }
            }
            field if field == TEXT || FIELDS.contains(&field) => {
                conditions.push(Condition::new(field, text(field, value)?));
            }
            other => return Err(format!("unknown field `{other}`")),
        }
    }
    Ok(SavedFilter {
        name: name.to_string(),
        conditions,
    })
}

/// `filter` with its name and values trimmed and repeated tags dropped, if
/// it is a valid filter; else why not.
pub(super) fn clean(filter: &SavedFilter) -> Result<SavedFilter, String> {
    let name = filter.name.trim();
    if name.is_empty() {
        return Err("the filter has no name".into());
    }
    let mut conditions: Vec<Condition> = Vec::new();
    for condition in &filter.conditions {
        let field = match condition.field.trim() {
            field if field == TAG || field == TEXT || FIELDS.contains(&field) => field.to_string(),
            field => match field.strip_prefix(CUSTOM_PREFIX).map(str::trim) {
                Some(key) if !key.is_empty() => format!("{CUSTOM_PREFIX}{key}"),
                _ => return Err(format!("unknown field `{field}`")),
            },
        };
        let field = field.as_str();
        let value = condition.value.trim();
        if value.is_empty() {
            return Err(format!("`{field}` has no value"));
        }
        let condition = Condition::new(field, value);
        if field == TAG {
            if !conditions.contains(&condition) {
                conditions.push(condition);
            }
        } else if conditions.iter().any(|seen| seen.field == field) {
            return Err(format!("`{field}` appears twice"));
        } else {
            conditions.push(condition);
        }
    }
    if conditions.is_empty() {
        return Err("it has no conditions".into());
    }
    Ok(SavedFilter {
        name: name.to_string(),
        conditions,
    })
}

/// Writes `filter` into the filters file's `document`: in place of the
/// filter of the same name (ignoring case), else after the others.
pub(super) fn upsert(document: &mut Mapping, filter: &SavedFilter) {
    let name = Value::from(filter.name.as_str());
    let definition = to_value(filter);
    if !document.keys().any(|key| same_name(key, &filter.name)) {
        document.insert(name, definition);
        return;
    }
    *document = std::mem::take(document)
        .into_iter()
        .map(|(key, value)| {
            if same_name(&key, &filter.name) {
                (name.clone(), definition.clone())
            } else {
                (key, value)
            }
        })
        .collect();
}

/// Removes the filter called `name` (ignoring case) from the filters
/// file's `document`; false if it has none.
pub(super) fn remove(document: &mut Mapping, name: &str) -> bool {
    let key = document.keys().find(|key| same_name(key, name)).cloned();
    key.is_some_and(|key| document.shift_remove(&key).is_some())
}

/// A filter's definition as written in the filters file: tags go in one
/// `tags` list and custom fields in one `custom` mapping, each where its
/// first condition is.
fn to_value(filter: &SavedFilter) -> Value {
    let mut document = Mapping::new();
    for condition in &filter.conditions {
        let value = Value::from(condition.value.as_str());
        if condition.field == TAG {
            let tags = document
                .entry(TAGS.into())
                .or_insert_with(|| Value::Sequence(Vec::new()));
            if let Value::Sequence(tags) = tags {
                tags.push(value);
            }
        } else if let Some(key) = condition.field.strip_prefix(CUSTOM_PREFIX) {
            let custom = document
                .entry(CUSTOM.into())
                .or_insert_with(|| Value::Mapping(Mapping::new()));
            if let Value::Mapping(custom) = custom {
                custom.insert(key.into(), value);
            }
        } else {
            document.insert(condition.field.as_str().into(), value);
        }
    }
    Value::Mapping(document)
}

/// Whether the filters file key `key` names the filter `name`, ignoring case.
fn same_name(key: &Value, name: &str) -> bool {
    scalar_text(key).is_some_and(|key| key.trim().to_lowercase() == name.trim().to_lowercase())
}
