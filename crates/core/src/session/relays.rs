//! Every session of the app through a relay (#83).
//!
//! [`SessionRelays`] keeps each session's [`Relay`] and the window it is
//! shown in. Starting one for a session that has one replaces it: a
//! reconnect gives the session new events, and so a new relay, whose
//! consumer's key no earlier relay gave out. Acknowledgements are passed on
//! to the session's relay, which ignores any but the current consumer's.
//!
//! When a relay's consumer is found gone on delivery (its window closed or
//! its page reloaded), its relay is dropped, which closes the session as
//! dropping its [`SessionEvents`] does; never a relay a reconnect has
//! started since. Session windows change this rule (#84).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use super::{Consumer, ConsumerGone, ConsumerKey, Relay, SessionEvent, SessionEvents, SessionId};
use crate::layout::WindowId;

/// Each session's relay and the window it is shown in.
#[derive(Default)]
pub struct SessionRelays {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    shown: Mutex<HashMap<SessionId, Shown>>,
    /// Tells a session's relays apart, so a consumer found gone drops its
    /// own relay only.
    next: AtomicU64,
}

/// One of a session's relays, as numbered by [`Shared::next`].
#[derive(Clone, Copy, PartialEq, Eq)]
struct RelayNumber(u64);

struct Shown {
    relay: Relay,
    window: WindowId,
    number: RelayNumber,
}

impl Shared {
    fn shown(&self) -> MutexGuard<'_, HashMap<SessionId, Shown>> {
        self.shown.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A consumer that, once found gone, drops its relay from the registry.
struct Registered<C> {
    consumer: C,
    relays: Weak<Shared>,
    session: SessionId,
    number: RelayNumber,
}

impl<C: Consumer> Consumer for Registered<C> {
    fn deliver(&mut self, event: SessionEvent) -> Result<(), ConsumerGone> {
        self.consumer.deliver(event)
    }

    fn gone(self: Box<Self>) {
        let Self {
            consumer,
            relays,
            session,
            number,
        } = *self;
        Box::new(consumer).gone();
        let Some(relays) = relays.upgrade() else {
            return;
        };
        let mut shown = relays.shown();
        let removed = match shown.get(&session) {
            Some(current) if current.number == number => shown.remove(&session),
            _ => None,
        };
        drop(shown);
        // Dropped unlocked: dropping a relay wakes its task.
        drop(removed);
    }
}

impl SessionRelays {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts relaying `session`'s `events` to `consumer`, shown in
    /// `window`, in place of the relay the session had. Returns the key
    /// that acknowledges the output.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn start(
        &self,
        session: SessionId,
        window: WindowId,
        events: SessionEvents,
        consumer: impl Consumer,
    ) -> ConsumerKey {
        let number = RelayNumber(self.shared.next.fetch_add(1, Ordering::Relaxed));
        let consumer = Registered {
            consumer,
            relays: Arc::downgrade(&self.shared),
            session,
            number,
        };
        // Started and recorded under the lock, so a consumer found gone
        // straight away finds its relay to drop.
        let mut shown = self.shared.shown();
        let (relay, key) = Relay::start(events, consumer);
        let replaced = shown.insert(
            session,
            Shown {
                relay,
                window,
                number,
            },
        );
        drop(shown);
        drop(replaced);
        key
    }

    /// Tells `session` that the consumer `key` has finished with `bytes`
    /// bytes of its output; ignored unless `key` names the consumer shown
    /// the session now (see [`Relay::acknowledge`]), or for a session
    /// without a relay.
    pub fn acknowledge(&self, session: SessionId, key: ConsumerKey, bytes: usize) {
        if let Some(shown) = self.shared.shown().get(&session) {
            shown.relay.acknowledge(key, bytes);
        }
    }

    /// The window `session` is shown in, while it has a relay.
    pub fn shown_in(&self, session: SessionId) -> Option<WindowId> {
        self.shared.shown().get(&session).map(|shown| shown.window)
    }

    /// Drops `session`'s relay, which closes the session: it is discarded.
    pub fn forget(&self, session: SessionId) {
        let removed = self.shared.shown().remove(&session);
        drop(removed);
    }
}
