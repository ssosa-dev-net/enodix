//! Whether each session window's page still answers (#120).
//!
//! A page that hangs without crashing is not reported by the webview
//! engine (WebView2 raised no `ProcessFailed` for a page stopped for 48 s
//! with a key and a click sent to it over DevTools), and a channel into it
//! still says `Ok`. So the shell
//! asks every session window's page a question every [`PAGE_ASK_EVERY`];
//! the page answers as soon as it runs. One whose oldest unanswered
//! question is [`PAGE_ANSWER_WAIT`] old does not answer: the user is told
//! and decides (it may yet recover). An answer after that says it answers
//! again.
//!
//! The question is an event run in the page as soon as it is free, not a
//! timer of the page's own, so a page whose timers are throttled
//! (minimised, hidden) still answers at once; a page drawing a flood
//! answers between two pieces of it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::WindowId;

/// How often the shell asks each session window's page.
pub const PAGE_ASK_EVERY: Duration = Duration::from_secs(2);

/// How long a page may leave a question unanswered before it is taken
/// for not answering: far beyond what a page busy for a moment takes (a
/// flood drawn, a large paste, a reload), short enough that a user who
/// finds the window frozen hears of it soon.
pub const PAGE_ANSWER_WAIT: Duration = Duration::from_secs(10);

/// What a page's answer changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answered {
    /// It was answering: nothing to say.
    AsBefore,
    /// It had been taken for not answering, and answers again.
    Again,
}

#[derive(Debug, Default)]
struct Page {
    /// When the oldest question it has not answered was asked.
    waiting_since: Option<Instant>,
    /// Whether it was taken for not answering.
    silent: bool,
}

/// The questions asked of the session windows' pages and their answers.
#[derive(Debug, Default)]
pub struct PageAnswers {
    pages: HashMap<WindowId, Page>,
}

impl PageAnswers {
    pub fn new() -> Self {
        Self::default()
    }

    /// A question is asked of `window`'s page at `now`.
    pub fn ask(&mut self, window: WindowId, now: Instant) {
        self.pages
            .entry(window)
            .or_default()
            .waiting_since
            .get_or_insert(now);
    }

    /// `window`'s page answered: every question asked so far is answered
    /// (it runs).
    pub fn answered(&mut self, window: WindowId) -> Answered {
        let page = self.pages.entry(window).or_default();
        page.waiting_since = None;
        if std::mem::take(&mut page.silent) {
            Answered::Again
        } else {
            Answered::AsBefore
        }
    }

    /// The windows whose pages, at `now`, have left a question unanswered
    /// for [`PAGE_ANSWER_WAIT`] or longer and were not said to be so yet;
    /// each is said once, until it answers again.
    pub fn not_answering(&mut self, now: Instant) -> Vec<WindowId> {
        let mut found: Vec<WindowId> = self
            .pages
            .iter_mut()
            .filter(|(_, page)| !page.silent)
            .filter(|(_, page)| {
                page.waiting_since
                    .is_some_and(|since| now.saturating_duration_since(since) >= PAGE_ANSWER_WAIT)
            })
            .map(|(&window, page)| {
                page.silent = true;
                window
            })
            .collect();
        found.sort_by_key(|window| window.0);
        found
    }

    /// Whether `window`'s page is taken for not answering now.
    pub fn is_silent(&self, window: WindowId) -> bool {
        self.pages.get(&window).is_some_and(|page| page.silent)
    }

    /// Forgets every window not in `open` (closed, docked back, taken
    /// over).
    pub fn keep_only(&mut self, open: &[WindowId]) {
        self.pages.retain(|window, _| open.contains(window));
    }
}
