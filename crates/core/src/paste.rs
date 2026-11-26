//! Paste safety: what a paste (or a snippet, sent the same way) needs before
//! it reaches a device.
//!
//! A paste of more than one line waits for the user to confirm it, seeing
//! the exact text and how many lines it has. The user may turn that off
//! (a preference, [`crate::preferences::Preferences::confirm_multi_line_paste`])
//! for every host except production ones, which always ask. The text is sent
//! line by line, each line with its own line break, with an optional delay
//! between lines for slow devices (the inherited `paste_line_delay` setting).
//!
//! The environment a host is marked as (production, staging, lab) also gives
//! its sessions their tab and border colour; the interface draws them from
//! [`PasteSafety::environment`].

use serde::{Deserialize, Serialize};

use crate::workspace::{EffectiveSettings, Environment};

/// The longest delay between pasted lines, in milliseconds.
pub const MAX_PASTE_LINE_DELAY_MS: u32 = 10_000;

/// What applies to pastes into one session: its host's resolved
/// environment and paste line delay. The default (no environment, no delay)
/// is for sessions without an inventory host, such as quick connects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteSafety {
    pub environment: Option<Environment>,
    /// Milliseconds to wait after each pasted line before the next one.
    pub line_delay_ms: u32,
}

impl PasteSafety {
    /// From a host's (or folder's) resolved settings.
    pub fn of(settings: &EffectiveSettings) -> Self {
        Self {
            environment: settings.environment.as_ref().map(|e| e.value),
            line_delay_ms: settings.paste_line_delay.value,
        }
    }

    /// Whether a multi-line paste asks even with the confirmation turned
    /// off: on production hosts it cannot be turned off.
    pub fn always_confirms(&self) -> bool {
        self.environment == Some(Environment::Production)
    }

    /// Whether a session with this safety is confirmed by the user before
    /// it joins the multi-send group: production ones, whatever the paste
    /// preference (joining is not a paste).
    pub fn confirms_joining(&self) -> bool {
        self.always_confirms()
    }
}

/// How a paste goes out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PastePlan {
    /// The text split into lines, each with its own line break (`\r\n`,
    /// `\n` or `\r`, as pasted); the last one may have none.
    pub lines: Vec<String>,
    /// Whether the user must confirm it, shown the text, before any line
    /// is sent.
    pub confirm: bool,
    /// Milliseconds to wait between two lines.
    pub line_delay_ms: u32,
}

/// How `text` is pasted into a session with `safety`: confirmed first if it
/// has more than one line and either the user wants multi-line pastes
/// confirmed (`confirm_multi_line`) or the host is a production one. One
/// line, with or without its line break, never asks.
pub fn plan_paste(text: &str, safety: &PasteSafety, confirm_multi_line: bool) -> PastePlan {
    let lines = paste_lines(text);
    let confirm = lines.len() > 1 && (confirm_multi_line || safety.always_confirms());
    PastePlan {
        lines,
        confirm,
        line_delay_ms: safety.line_delay_ms.min(MAX_PASTE_LINE_DELAY_MS),
    }
}

/// Whether a snippet's steps (#23) wait for the user's yes, shown what they
/// will do, before they run on a session with `safety`: as a paste of
/// several lines does (unless the user turned that off), and always on a
/// production host.
pub fn confirms_steps(safety: &PasteSafety, confirm_multi_line: bool) -> bool {
    confirm_multi_line || safety.always_confirms()
}

/// `text` split after each line break (`\r\n`, `\n` or a lone `\r`),
/// keeping the breaks; text after the last break is a line of its own.
pub fn paste_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let end = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
            b'\r' | b'\n' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(text[start..end].to_string());
        start = end;
        index = end;
    }
    if start < text.len() {
        lines.push(text[start..].to_string());
    }
    lines
}

/// Which of the sessions `joining` (each an id of the caller's, with its
/// safety) must be confirmed before they join the multi-send group, in
/// their order. Adding a whole pane asks once, naming these.
pub fn confirm_joining<T: Clone>(joining: &[(T, PasteSafety)]) -> Vec<T> {
    joining
        .iter()
        .filter(|(_, safety)| safety.confirms_joining())
        .map(|(id, _)| id.clone())
        .collect()
}

/// How one paste (or snippet) goes out to every member of the multi-send
/// group: confirmed once for all of them, then each member's lines with its
/// own delay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPastePlan {
    /// Each member's own plan, in the order the members were given.
    pub members: Vec<PastePlan>,
    /// Whether the user confirms it, once, before any member gets a line:
    /// if any member's own plan would ask.
    pub confirm: bool,
    /// The strictest member environment (production, then staging, then
    /// lab), for how the confirmation is presented.
    pub environment: Option<Environment>,
}

/// How a paste goes out to the group's `members`, each the text it is to
/// receive (the same for a paste; a snippet is filled per member) and its
/// safety. See [`plan_paste`] for each member's own rules.
pub fn plan_group_paste(
    members: &[(&str, PasteSafety)],
    confirm_multi_line: bool,
) -> GroupPastePlan {
    let plans: Vec<PastePlan> = members
        .iter()
        .map(|(text, safety)| plan_paste(text, safety, confirm_multi_line))
        .collect();
    GroupPastePlan {
        confirm: plans.iter().any(|plan| plan.confirm),
        environment: members
            .iter()
            .filter_map(|(_, safety)| safety.environment)
            .max_by_key(|&environment| strictness(environment)),
        members: plans,
    }
}

fn strictness(environment: Environment) -> u8 {
    match environment {
        Environment::Lab => 1,
        Environment::Staging => 2,
        Environment::Production => 3,
    }
}
