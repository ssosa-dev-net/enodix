//! Keyword highlighting: which parts of a line of terminal output stand
//! out, and in which colour.
//!
//! Highlighting is decoration only. It works on the text a terminal shows
//! (the interface asks for the lines on screen), never on the session's
//! bytes: what is sent, received and logged is the same with it on or off.
//!
//! The app ships [`BUILT_IN_RULES`] (states such as down, up and
//! err-disabled, IPv4 and IPv6 addresses, interface names). A workspace adds
//! its own rules and changes or turns off the built-in ones in its
//! highlights file (see `Workspace::highlight_rules`). A [`Highlighter`]
//! holds the rules that apply, compiled, and finds them in a line
//! ([`Highlighter::spans`]).
//!
//! Patterns are regular expressions in the syntax of the `regex` crate
//! (Perl-like, without look-around or back-references; `(?i)` ignores
//! case). Where matches of two rules overlap, the earlier rule wins.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// A highlight rule: text matching `pattern` shows in `color`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightRule {
    /// What the rule is called; unique in a workspace, ignoring case.
    pub name: String,
    /// A regular expression (see the module documentation).
    pub pattern: String,
    /// `#rrggbb` (lower case), or one of [`COLOR_NAMES`], a colour of the
    /// terminal's palette, which follows its theme.
    pub color: String,
    /// A rule turned off is kept, but highlights nothing.
    pub enabled: bool,
}

/// Where a rule of a workspace comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HighlightSource {
    /// One of [`BUILT_IN_RULES`], as built in.
    BuiltIn,
    /// A built-in rule the workspace changes (its pattern, colour, or
    /// turning it off).
    Changed,
    /// The workspace's own.
    Workspace,
}

/// A rule as a workspace lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedRule {
    pub rule: HighlightRule,
    pub source: HighlightSource,
    /// Why its pattern cannot be used, if it cannot: such a rule
    /// highlights nothing, and the other rules are not affected.
    pub error: Option<String>,
}

/// A highlighted part of a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Span {
    /// Where it starts and ends, in characters (Unicode scalar values) from
    /// the start of the line, `end` exclusive.
    pub start: usize,
    pub end: usize,
    /// The rule's colour (see [`HighlightRule::color`]).
    pub color: String,
    /// The rule's name.
    pub rule: String,
}

/// The names of the terminal palette's colours a rule can use.
pub const COLOR_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

/// A rule the app ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltInRule {
    pub name: &'static str,
    pub pattern: &'static str,
    pub color: &'static str,
}

impl BuiltInRule {
    pub fn rule(&self) -> HighlightRule {
        HighlightRule {
            name: self.name.into(),
            pattern: self.pattern.into(),
            color: self.color.into(),
            enabled: true,
        }
    }
}

/// The rules the app ships, in order (earlier wins where matches overlap).
pub const BUILT_IN_RULES: [BuiltInRule; 6] = [
    BuiltInRule {
        name: "state-down",
        pattern: r"(?i)\b(?:administratively\s+down|err-disabled|errdisabled?|down|notconnect|not\s+connected|failed|failure|fail|error|unreachable|denied|deny|critical|inactive|disabled|suspended|shutdown)\b",
        color: "red",
    },
    BuiltInRule {
        name: "state-warning",
        pattern: r"(?i)\b(?:warning|warn|standby|blocking|listening|learning|pending|flapping|degraded|half-duplex|init)\b",
        color: "yellow",
    },
    BuiltInRule {
        name: "state-up",
        pattern: r"(?i)\b(?:up|connected|established|forwarding|full-duplex|active|enabled|permit|online|running|ok)\b",
        color: "green",
    },
    BuiltInRule {
        name: "ipv4",
        // Candidates; `Check::Ipv4` keeps the real addresses.
        pattern: r"\d+(?:\.\d+)+(?:/\d+)?",
        color: "cyan",
    },
    BuiltInRule {
        name: "ipv6",
        // Candidates; `Check::Ipv6` keeps the real addresses.
        pattern: r"(?:[0-9A-Fa-f]{0,4}:){2,8}(?:(?:\d{1,3}\.){3}\d{1,3}|[0-9A-Fa-f]{1,4})?(?:/\d+)?",
        color: "cyan",
    },
    BuiltInRule {
        name: "interface",
        pattern: r"(?i)\b(?:(?:(?:Ten|TwentyFive|Forty|Fifty|Hundred|TwoHundred|FourHundred)?Gig(?:abit)?E(?:thernet)?|FastEthernet|Ethernet|Eth|Port-channel|Bundle-Ether|Vlan|Loopback|Tunnel|Serial|Management|mgmt|Gi|Fa|Te|Twe|Tw|Fo|Hu|Et|Po|Lo|Tu|Se|Vl|BE|ae|reth|fxp|em)\d+(?:[/:.]\d+)*|(?:ge|xe|et|fe|so|gr|lt)-\d+/\d+/\d+(?:[:.]\d+)*|irb\.\d+)\b",
        color: "bright-blue",
    },
];

/// The built-in rules as a workspace without changes lists them.
pub fn built_in_rules() -> Vec<ListedRule> {
    BUILT_IN_RULES
        .iter()
        .map(|rule| ListedRule {
            rule: rule.rule(),
            source: HighlightSource::BuiltIn,
            error: None,
        })
        .collect()
}

/// The built-in rule called `name` (ignoring case), if there is one.
pub fn built_in(name: &str) -> Option<&'static BuiltInRule> {
    let name = name.trim();
    BUILT_IN_RULES
        .iter()
        .find(|rule| rule.name.eq_ignore_ascii_case(name))
}

/// Why `pattern` cannot be used as a rule's pattern, if it cannot.
pub fn check_pattern(pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err("the pattern is empty".into());
    }
    compile(pattern).map(|_| ())
}

fn compile(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .build()
        .map_err(|error| format!("the pattern cannot be used: {error}"))
}

/// `text` as a rule colour: `#rrggbb` in lower case (from `#rgb` or
/// `#rrggbb`) or one of [`COLOR_NAMES`] (ignoring case), if it is one.
pub fn parse_color(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    if COLOR_NAMES.contains(&lower.as_str()) {
        return Some(lower);
    }
    let digits = lower.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        6 => Some(lower),
        3 => Some(digits.chars().fold(String::from("#"), |mut color, c| {
            color.push(c);
            color.push(c);
            color
        })),
        _ => None,
    }
}

/// Extra checks of a built-in rule's matches that a regular expression of
/// the `regex` crate cannot make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    None,
    Ipv4,
    Ipv6,
}

impl Check {
    fn of(rule: &HighlightRule) -> Self {
        match built_in(&rule.name) {
            Some(built) if built.pattern == rule.pattern => match built.name {
                "ipv4" => Self::Ipv4,
                "ipv6" => Self::Ipv6,
                _ => Self::None,
            },
            _ => Self::None,
        }
    }

    /// Whether `line[start..end]` counts as a match.
    fn accepts(self, line: &str, start: usize, end: usize) -> bool {
        match self {
            Self::None => true,
            Self::Ipv4 => standalone(line, start, end) && address(&line[start..end], 32, ipv4),
            Self::Ipv6 => standalone(line, start, end) && address(&line[start..end], 128, ipv6),
        }
    }
}

fn ipv4(text: &str) -> bool {
    text.parse::<Ipv4Addr>().is_ok()
}

fn ipv6(text: &str) -> bool {
    // A bare `::` is more often text than the unspecified address.
    text.bytes().any(|b| b.is_ascii_hexdigit()) && text.parse::<Ipv6Addr>().is_ok()
}

/// Whether `text` is an address (by `valid`) with an optional prefix
/// length of at most `max_prefix`.
fn address(text: &str, max_prefix: u32, valid: fn(&str) -> bool) -> bool {
    let (address, prefix) = match text.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (text, None),
    };
    valid(address)
        && prefix.is_none_or(|prefix| prefix.parse::<u32>().is_ok_and(|p| p <= max_prefix))
}

/// Whether the address at `line[start..end]` stands on its own: not part of
/// a word, a longer dotted number or a longer colon-separated one.
fn standalone(line: &str, start: usize, end: usize) -> bool {
    let joined = |c: char| c.is_alphanumeric() || c == '_' || c == ':';
    let before = line[..start].chars().next_back();
    let mut after = line[end..].chars();
    let next = after.next();
    let after_next = after.next();
    !before.is_some_and(|c| joined(c) || c == '.')
        && !next.is_some_and(|c| {
            joined(c) || (c == '.' && after_next.is_some_and(char::is_alphanumeric))
        })
}

#[derive(Debug, Clone)]
struct Compiled {
    name: String,
    color: String,
    regex: Regex,
    check: Check,
}

/// Compiled rules, ready to find in lines.
#[derive(Debug, Clone)]
pub struct Highlighter {
    rules: Vec<Compiled>,
}

impl Default for Highlighter {
    /// The built-in rules.
    fn default() -> Self {
        static BUILT_IN: LazyLock<Highlighter> = LazyLock::new(|| {
            let rules: Vec<HighlightRule> = BUILT_IN_RULES.iter().map(BuiltInRule::rule).collect();
            Highlighter::new(&rules)
        });
        BUILT_IN.clone()
    }
}

impl Highlighter {
    /// The rules that are on and whose pattern can be used, in order
    /// (earlier wins where matches overlap). The others are left out
    /// without affecting these; [`check_pattern`] says what is wrong.
    pub fn new(rules: &[HighlightRule]) -> Self {
        let rules = rules
            .iter()
            .filter(|rule| rule.enabled && !rule.pattern.trim().is_empty())
            .filter_map(|rule| {
                Some(Compiled {
                    name: rule.name.clone(),
                    color: rule.color.clone(),
                    regex: compile(&rule.pattern).ok()?,
                    check: Check::of(rule),
                })
            })
            .collect();
        Self { rules }
    }

    /// What to highlight in `line`, in line order, never overlapping.
    /// Empty matches highlight nothing.
    pub fn spans(&self, line: &str) -> Vec<Span> {
        // Byte ranges and the rule, kept in line order (they never overlap).
        let mut taken: Vec<(usize, usize, usize)> = Vec::new();
        for (index, rule) in self.rules.iter().enumerate() {
            for found in rule.regex.find_iter(line) {
                let (start, end) = (found.start(), found.end());
                if start == end || !rule.check.accepts(line, start, end) {
                    continue;
                }
                // Only the neighbours in line order can overlap it.
                let at = taken.partition_point(|&(s, _, _)| s < start);
                let free = (at == 0 || taken[at - 1].1 <= start)
                    && taken.get(at).is_none_or(|&(s, _, _)| end <= s);
                if free {
                    taken.insert(at, (start, end, index));
                }
            }
        }

        // Byte offsets to character offsets, in one pass along the line.
        let mut spans = Vec::with_capacity(taken.len());
        let mut byte = 0;
        let mut chars = 0;
        let mut advance = |to: usize| {
            chars += line[byte..to].chars().count();
            byte = to;
            chars
        };
        for (start, end, index) in taken {
            let rule = &self.rules[index];
            let start = advance(start);
            let end = advance(end);
            spans.push(Span {
                start,
                end,
                color: rule.color.clone(),
                rule: rule.name.clone(),
            });
        }
        spans
    }
}
