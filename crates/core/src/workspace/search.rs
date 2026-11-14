//! The search index: every host's searchable text, held in memory and
//! rebuilt from the files each time the workspace loads (the files stay the
//! only source of truth).
//!
//! A query is split into words; a host matches when every word is part of
//! one of its fields, ignoring case: its name, management address, tags
//! (those inherited from its folders included), built-in fields (the
//! platform that applies, perhaps a folder's), role (id or name) and custom
//! field values. Each word scores by the best field it
//! is part of: the name before the address, then tags, then the other
//! fields; within each, a whole match before the start of a field, the
//! start of a word, and anywhere else. Hosts are ordered by the sum of
//! their words' scores (so for one word, name matches come first), then by
//! name.
//!
//! Saved filters ([`SavedFilter`]) are applied over the same index.

use std::borrow::Cow;
use std::collections::BTreeSet;

use serde::Serialize;

use super::filters::{Condition, SavedFilter, CUSTOM_PREFIX, TAG, TEXT};
use super::{Folder, HostId, ADDRESS, NAME, PLATFORM};

/// What a search found: the best hits, up to the limit asked for, and how
/// many hosts matched in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total: usize,
}

/// A host a search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub id: HostId,
    pub name: String,
    /// The management address.
    pub address: String,
    /// The fields the query's words matched, in the order of the words.
    pub matches: Vec<FieldMatch>,
}

/// A field of a host that matched: its key (`name`, `address`, `tag`, a
/// built-in field such as `vendor`, `role`, or `custom.<key>`) and value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldMatch {
    pub field: String,
    pub value: String,
}

/// The part of the tree a saved filter shows: the hosts it matches, ordered
/// by id, and every folder holding one of them (the root aside), by path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilterView {
    pub hosts: Vec<HostId>,
    pub folders: Vec<String>,
}

/// How strongly a field counts: lower is better.
const NAME_WEIGHT: u32 = 0;
const ADDRESS_WEIGHT: u32 = 1;
const TAG_WEIGHT: u32 = 2;
const FIELD_WEIGHT: u32 = 3;
/// How many kinds of match there are within one field (see [`kind`]).
const KINDS: u32 = 4;

#[derive(Debug, Clone, Default)]
pub(super) struct SearchIndex {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    id: HostId,
    name: String,
    address: String,
    /// The name in lower case, to order hits.
    sort_key: String,
    fields: Vec<Field>,
}

#[derive(Debug, Clone)]
struct Field {
    key: Cow<'static, str>,
    value: String,
    lower: String,
    weight: u32,
}

impl Entry {
    fn push(&mut self, key: impl Into<Cow<'static, str>>, value: &str, weight: u32) {
        let lower = value.to_lowercase();
        let key = key.into();
        if value.is_empty()
            || self
                .fields
                .iter()
                .any(|field| field.key == key && field.lower == lower)
        {
            return;
        }
        self.fields.push(Field {
            key,
            value: value.to_string(),
            lower,
            weight,
        });
    }

    /// The best field `term` (in lower case) is part of: its score and index.
    fn best(&self, term: &str) -> Option<(u32, usize)> {
        self.fields
            .iter()
            .enumerate()
            .filter_map(|(index, field)| {
                kind(&field.lower, term).map(|kind| (field.weight * KINDS + kind, index))
            })
            .min()
    }

    /// The score of a host matching every term, and the fields they matched.
    fn score(&self, terms: &[String]) -> Option<(u32, Vec<usize>)> {
        let mut score = 0;
        let mut fields = Vec::new();
        for term in terms {
            let (points, field) = self.best(term)?;
            score += points;
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
        Some((score, fields))
    }

    fn satisfies(&self, rule: &Rule) -> bool {
        match rule {
            Rule::Text(terms) => terms.iter().all(|term| self.best(term).is_some()),
            Rule::Field { key, pattern } => self
                .fields
                .iter()
                .any(|field| field.key == *key && glob(pattern, &field.lower)),
        }
    }
}

/// How `term` is part of `text`, both in lower case: the whole of it (0),
/// its start (1), the start of a word in it (2), elsewhere (3), or not.
fn kind(text: &str, term: &str) -> Option<u32> {
    if text == term {
        return Some(0);
    }
    if text.starts_with(term) {
        return Some(1);
    }
    let mut found = None;
    for (at, _) in text.match_indices(term) {
        let after_separator = text[..at]
            .chars()
            .next_back()
            .is_some_and(|before| !before.is_alphanumeric());
        if after_separator {
            return Some(2);
        }
        found = Some(3);
    }
    found
}

/// A condition of a saved filter, ready to test.
enum Rule {
    /// Every word is part of some field, as in a search.
    Text(Vec<String>),
    /// A field with this key matches the pattern (lower case, `*` for any
    /// text).
    Field { key: String, pattern: String },
}

impl Rule {
    fn of(condition: &Condition) -> Self {
        let value = condition.value.trim().to_lowercase();
        if condition.field == TEXT {
            Rule::Text(value.split_whitespace().map(str::to_string).collect())
        } else {
            Rule::Field {
                key: condition.field.trim().to_string(),
                pattern: value,
            }
        }
    }
}

/// Whether `text` matches `pattern`, where `*` stands for any text.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // The last `*` seen, and where in the text it started matching.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

impl SearchIndex {
    /// Indexes every host of `tree`. `role_name` gives the name of a role
    /// by id, when the workspace defines it.
    pub(super) fn build(tree: &Folder, role_name: impl Fn(&str) -> Option<String>) -> Self {
        let mut index = Self::default();
        index.add(tree, &[], None, &role_name);
        index
    }

    fn add(
        &mut self,
        folder: &Folder,
        inherited: &[String],
        inherited_platform: Option<&str>,
        role_name: &impl Fn(&str) -> Option<String>,
    ) {
        let platform = folder.settings.platform.as_deref().or(inherited_platform);
        let mut tags = inherited.to_vec();
        for tag in &folder.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        for host in &folder.hosts {
            let mut entry = Entry {
                id: host.id.clone(),
                name: host.name.clone(),
                address: host.address.clone(),
                sort_key: host.name.to_lowercase(),
                fields: Vec::new(),
            };
            entry.push(NAME, &host.name, NAME_WEIGHT);
            entry.push(ADDRESS, &host.address, ADDRESS_WEIGHT);
            for tag in host.tags.iter().chain(&tags) {
                entry.push(TAG, tag, TAG_WEIGHT);
            }
            // The role's name before its id, so that a word matching both
            // shows the name.
            if let Some(name) = host.role.as_deref().and_then(role_name) {
                entry.push(super::ROLE, &name, FIELD_WEIGHT);
            }
            // The platform that applies: the host's own, else its folders'.
            if let Some(platform) = host.settings.platform.as_deref().or(platform) {
                entry.push(PLATFORM, platform, FIELD_WEIGHT);
            }
            for (key, value) in host.text_fields() {
                if let Some(value) = value {
                    entry.push(key, value, FIELD_WEIGHT);
                }
            }
            for field in &host.custom {
                entry.push(
                    format!("{CUSTOM_PREFIX}{}", field.key),
                    &field.value,
                    FIELD_WEIGHT,
                );
            }
            self.entries.push(entry);
        }
        for child in &folder.folders {
            self.add(child, &tags, platform, role_name);
        }
    }

    /// The hosts matching every word of `query`, best first, at most
    /// `limit` of them. An empty query matches every host.
    pub(super) fn search(&self, query: &str, limit: usize) -> SearchResults {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut scored: Vec<(u32, &Entry, Vec<usize>)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                entry
                    .score(&terms)
                    .map(|(score, fields)| (score, entry, fields))
            })
            .collect();
        let total = scored.len();
        scored.sort_by(|(a_score, a, _), (b_score, b, _)| {
            a_score
                .cmp(b_score)
                .then_with(|| a.sort_key.cmp(&b.sort_key))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        scored.truncate(limit);
        let hits = scored
            .into_iter()
            .map(|(_, entry, fields)| SearchHit {
                id: entry.id.clone(),
                name: entry.name.clone(),
                address: entry.address.clone(),
                matches: fields
                    .into_iter()
                    .map(|index| {
                        let field = &entry.fields[index];
                        FieldMatch {
                            field: field.key.to_string(),
                            value: field.value.clone(),
                        }
                    })
                    .collect(),
            })
            .collect();
        SearchResults { hits, total }
    }

    /// The hosts matching every condition of `filter`, and their folders.
    pub(super) fn apply(&self, filter: &SavedFilter) -> FilterView {
        let rules: Vec<Rule> = filter.conditions.iter().map(Rule::of).collect();
        let mut hosts: Vec<HostId> = if rules.is_empty() {
            Vec::new()
        } else {
            self.entries
                .iter()
                .filter(|entry| rules.iter().all(|rule| entry.satisfies(rule)))
                .map(|entry| entry.id.clone())
                .collect()
        };
        hosts.sort();
        let mut folders = BTreeSet::new();
        for id in &hosts {
            let mut path = id.split().0;
            while !path.is_empty() {
                if !folders.insert(path.to_string()) {
                    break;
                }
                path = path.rsplit_once('/').map_or("", |(parent, _)| parent);
            }
        }
        FilterView {
            hosts,
            folders: folders.into_iter().collect(),
        }
    }
}
