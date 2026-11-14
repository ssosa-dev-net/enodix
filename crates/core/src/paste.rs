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
