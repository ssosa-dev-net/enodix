//! Automatic reconnect (#24): when a session that drops may try again by
//! itself, how long it waits before each try, and when it gives up.
//!
//! The interface only runs the countdown the core hands it and calls
//! `reconnect`. Everything that decides lives here: which closes may be
//! retried ([`CloseReason::allows_auto_reconnect`]), the waits, the count,
//! the reset once connected and the stop when the user presses a key.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::{CloseReason, SessionId};

/// How long an automatic reconnect waits before each try, one entry per
/// try: spaced further apart each time. After the last, it gives up.
pub const AUTO_RECONNECT_DELAYS: [Duration; 8] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(60),
    Duration::from_secs(60),
];

/// The next try: wait `after`, then reconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub after: Duration,
    /// Which try this is, from 1.
    pub attempt: usize,
    /// How many tries there are before it gives up.
    pub of: usize,
}

/// One session's automatic reconnect.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AutoReconnect {
    tries: usize,
    stopped: bool,
    gave_up: bool,
}

impl AutoReconnect {
    pub fn new() -> Self {
        Self::default()
    }

    /// The session closed for `reason`: the next try, or `None` when it
    /// must not try again by itself (the reason does not allow it, the user
    /// stopped it, or the tries ran out; see [`Self::gave_up`]).
    pub fn after_close(&mut self, reason: &CloseReason) -> Option<Retry> {
        if self.stopped || !reason.allows_auto_reconnect() {
            return None;
        }
        let Some(after) = AUTO_RECONNECT_DELAYS.get(self.tries) else {
            self.gave_up = true;
            return None;
        };
        self.tries += 1;
        Some(Retry {
            after: *after,
            attempt: self.tries,
            of: AUTO_RECONNECT_DELAYS.len(),
        })
    }

    /// The session connected: the count starts again, and a stop is over.
    pub fn connected(&mut self) {
        *self = Self::default();
    }

    /// The user reconnects by hand: as [`Self::connected`].
    pub fn restart(&mut self) {
        *self = Self::default();
    }

    /// The user pressed a key: no more tries until it connects again.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Whether the last close found the tries used up.
    pub fn gave_up(&self) -> bool {
        self.gave_up
    }
}

/// The automatic reconnect of every session that has one, by session id.
#[derive(Debug, Default)]
pub struct AutoReconnects(Mutex<HashMap<SessionId, Entry>>);

#[derive(Debug, Default)]
struct Entry {
    /// Whether the session's host has the `auto_reconnect` setting on.
    enabled: bool,
    state: AutoReconnect,
}

impl AutoReconnects {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, session: SessionId, f: impl FnOnce(&mut Entry) -> T) -> T {
        f(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session)
            .or_default())
    }

    /// Whether `session` reconnects by itself (its host's setting).
    pub fn enable(&self, session: SessionId, enabled: bool) {
        self.with(session, |entry| entry.enabled = enabled);
    }

    /// [`AutoReconnect::after_close`]; `None` too while not enabled.
    pub fn after_close(&self, session: SessionId, reason: &CloseReason) -> Option<Retry> {
        self.with(session, |entry| {
            entry
                .enabled
                .then(|| entry.state.after_close(reason))
                .flatten()
        })
    }

    /// Whether the tries of `session` ran out at its last close.
    pub fn gave_up(&self, session: SessionId) -> bool {
        self.with(session, |entry| entry.enabled && entry.state.gave_up())
    }

    pub fn connected(&self, session: SessionId) {
        self.with(session, |entry| entry.state.connected());
    }

    pub fn restart(&self, session: SessionId) {
        self.with(session, |entry| entry.state.restart());
    }

    pub fn stop(&self, session: SessionId) {
        self.with(session, |entry| entry.state.stop());
    }

    /// The session is gone for good.
    pub fn forget(&self, session: SessionId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session);
    }
}
