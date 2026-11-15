//! Every session of the app through a relay (#83).
//!
//! [`SessionRelays`] keeps each session's [`Relay`] and the window it is
//! shown in. Starting one for a session that has one replaces it: a
//! reconnect gives the session new events, and so a new relay, whose
//! consumer's key no earlier relay gave out. Acknowledgements are passed on
//! to the session's relay, which ignores any but the current consumer's.
//!
//! A session moves to another window (#84) by [`SessionRelays::release`]
//! from the window showing it and [`SessionRelays::attach`] from the next;
//! a window that died holding it is made to let go with
//! [`SessionRelays::let_go`].
//!
//! When a main window's consumer is found gone on delivery (its page
//! reloaded or closed), its relay is dropped, which closes the session as
//! dropping its [`SessionEvents`] does; never a relay a reconnect has
//! started since. A session window's found gone leaves the session open,
//! held by nobody, for the main window to take.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use super::{
    Attached, Consumer, ConsumerGone, ConsumerKey, HandoverError, Relay, Reported, SessionEvent,
    SessionEvents, SessionId,
};
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
        // Only the main window's sessions go with its page.
        let removed = match shown.get(&session) {
            Some(current) if current.number == number && current.window == WindowId::MAIN => {
                shown.remove(&session)
            }
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

    /// Stops showing `session` to the consumer `key`, which is moving it to
    /// another window: see [`Relay::release`]. Returns the end of its share
    /// of the output, in bytes.
    pub fn release(&self, session: SessionId, key: ConsumerKey) -> Result<u64, HandoverError> {
        self.relay(session)?.release(key)
    }

    /// Shows `session` in `window` from now on, to `consumer`: see
    /// [`Relay::attach`]. Refused while another consumer holds it.
    pub fn attach(
        &self,
        session: SessionId,
        window: WindowId,
        consumer: impl Consumer,
    ) -> Result<Attached, HandoverError> {
        let mut shown = self.shared.shown();
        let current = shown
            .get_mut(&session)
            .ok_or(HandoverError::NoSuchSession)?;
        let attached = current.relay.attach(Registered {
            consumer,
            relays: Arc::downgrade(&self.shared),
            session,
            number: current.number,
        })?;
        current.window = window;
        Ok(attached)
    }

    /// Lets go of whoever holds `session` (a window that died holding
    /// it); returns the output delivered so far, or `None` for a session
    /// without a relay.
    pub fn let_go(&self, session: SessionId) -> Option<u64> {
        Some(self.relay(session).ok()?.let_go())
    }

    /// What `session` has reported to the windows shown it (#86): its
    /// last state, agreed algorithms, log failure and backup outcome,
    /// while it has a relay.
    pub fn reported(&self, session: SessionId) -> Option<Reported> {
        Some(self.relay(session).ok()?.reported())
    }

    fn relay(&self, session: SessionId) -> Result<Relay, HandoverError> {
        self.shared
            .shown()
            .get(&session)
            .map(|shown| shown.relay.clone())
            .ok_or(HandoverError::NoSuchSession)
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
