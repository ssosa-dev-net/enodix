//! Handing a session's events from one consumer to another (#82).
//!
//! A session's events go to exactly one consumer at a time: in the app, the
//! window showing it. A [`Relay`] owns the session's [`SessionEvents`] and
//! delivers them to its consumer; moving the session to another window is a
//! handover: [`Relay::release`] stops delivery to the consumer (from then on
//! it receives nothing more) and says how much output it was given in all,
//! and [`Relay::attach`] names the next one. Events that arrive in between
//! wait for it, in order: nothing is delivered twice, nothing skipped.
//!
//! Flow control spans the handover: output a consumer was given and had not
//! acknowledged counts as acknowledged once it lets go (it will never
//! acknowledge it now), and acknowledgements must name the consumer
//! ([`ConsumerKey`]), so a late one from a window the session left cannot
//! make room the current window has not. While nobody is shown the session,
//! flow control still bounds what waits for the next one.
//!
//! A consumer that finds it can no longer deliver ([`ConsumerGone`]: its
//! window was closed or died) lets go the same way, and the session stays
//! open for another consumer to take. The relay learns of it only from a
//! failed delivery: a consumer that dies while the session sends nothing
//! (or is paused by flow control on its account) stays attached until its
//! owner calls [`Relay::release`]. Dropping every handle to the relay drops
//! the session's events at once, whoever is shown it, which closes the
//! session as dropping [`SessionEvents`] does: when it next has something
//! to report.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

use super::output::Flow;
use super::{SessionEvent, SessionEvents, SessionState};

/// Where a relay delivers a session's events: in the app, one window.
pub trait Consumer: Send + 'static {
    /// Delivers one event, in order. An error means the consumer is gone
    /// for good; the event is kept for the next consumer.
    ///
    /// Called with the relay locked: it must not block, and must not call
    /// back into the relay.
    fn deliver(&mut self, event: SessionEvent) -> Result<(), ConsumerGone>;

    /// Called once the relay has let go of a consumer that went away
    /// (after [`Self::deliver`] failed), before the consumer is dropped.
    fn gone(self: Box<Self>) {}
}

/// The consumer can no longer receive anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerGone;

/// Identifies one consumer of one relay, for acknowledging and releasing.
/// No two consumers get the same key, of the same relay or of any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConsumerKey(u64);

impl ConsumerKey {
    /// The key as a number, for passing to an interface and back.
    pub fn get(self) -> u64 {
        self.0
    }

    /// The key a number from [`Self::get`] stands for.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// A consumer just attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attached {
    pub key: ConsumerKey,
    /// The output (in bytes) delivered to earlier consumers: the new one's
    /// share starts right after it.
    pub offset: u64,
}

/// Why a handover was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoverError {
    /// Another consumer is shown the session: it must be released first.
    Shown,
    /// The consumer named is not the one shown the session (it was
    /// released, went away or never was).
    NotShownThere,
    /// There is no such session (or it has no relay).
    NoSuchSession,
}

impl fmt::Display for HandoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shown => f.write_str("the session is shown elsewhere"),
            Self::NotShownThere => f.write_str("the session is not shown there"),
            Self::NoSuchSession => f.write_str("there is no such session"),
        }
    }
}

impl std::error::Error for HandoverError {}

/// Delivers a session's events to one consumer at a time, handing them over
/// without losing, doubling or reordering any (see the module docs).
///
/// Cloning gives another handle to the same relay.
#[derive(Clone)]
pub struct Relay {
    handle: Arc<Handle>,
}

/// What every [`Relay`] handle shares; dropping the last abandons the
/// relay, which closes the session.
struct Handle {
    shared: Arc<Shared>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shared.state().abandoned = true;
        self.shared.wake.notify_one();
    }
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the delivering task: a consumer attached, or the relay was
    /// abandoned. Only that task waits, on one wait at a time.
    wake: Notify,
    flow: Arc<Flow>,
}

/// Where every relay takes its consumers' keys from: one count for the
/// process, so no two relays (a session's relay before and after a
/// reconnect, say) ever give out the same key.
static NEXT_KEY: AtomicU64 = AtomicU64::new(0);

fn new_key() -> ConsumerKey {
    ConsumerKey(NEXT_KEY.fetch_add(1, Ordering::Relaxed))
}

struct State {
    consumer: Option<Current>,
    /// Output delivered so far, to every consumer, in bytes.
    delivered: u64,
    /// The last state delivered, to whichever consumer.
    shown_state: Option<SessionState>,
    abandoned: bool,
}

struct Current {
    key: ConsumerKey,
    consumer: Box<dyn Consumer>,
    /// Output delivered to it and not yet acknowledged, in bytes.
    unacknowledged: usize,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lets go of the current consumer: what it was given and never
    /// acknowledged counts as acknowledged.
    fn let_go(&self, state: &mut State) -> Option<Current> {
        let current = state.consumer.take()?;
        self.flow.acknowledge(current.unacknowledged);
        Some(current)
    }
}

impl Relay {
    /// Starts delivering `events` to `consumer`, and returns the relay and
    /// that consumer's key. The session's events go nowhere else from now
    /// on.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn start(events: SessionEvents, consumer: impl Consumer) -> (Self, ConsumerKey) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                consumer: None,
                delivered: 0,
                shown_state: None,
                abandoned: false,
            }),
            wake: Notify::new(),
            flow: events.flow(),
        });
        let key = new_key();
        shared.state().consumer = Some(Current {
            key,
            consumer: Box::new(consumer),
            unacknowledged: 0,
        });
        tokio::spawn(deliver(events, shared.clone()));
        (
            Self {
                handle: Arc::new(Handle { shared }),
            },
            key,
        )
    }

    fn shared(&self) -> &Shared {
        &self.handle.shared
    }

    /// Stops delivering to the consumer `key`: once this returns it is
    /// given nothing more. Returns the output delivered so far in all, in
    /// bytes: the end of its share, which it may still be receiving.
    /// Output it was given and has not acknowledged counts as acknowledged.
    pub fn release(&self, key: ConsumerKey) -> Result<u64, HandoverError> {
        let shared = self.shared();
        let mut state = shared.state();
        if state.consumer.as_ref().map(|current| current.key) != Some(key) {
            return Err(HandoverError::NotShownThere);
        }
        shared.let_go(&mut state);
        Ok(state.delivered)
    }

    /// Delivers the session's events to `consumer` from now on, starting
    /// with those that waited for it. Refused while another consumer is
    /// shown the session.
    pub fn attach(&self, consumer: impl Consumer) -> Result<Attached, HandoverError> {
        let shared = self.shared();
        let mut state = shared.state();
        if state.consumer.is_some() {
            return Err(HandoverError::Shown);
        }
        let key = new_key();
        state.consumer = Some(Current {
            key,
            consumer: Box::new(consumer),
            unacknowledged: 0,
        });
        let offset = state.delivered;
        drop(state);
        shared.wake.notify_one();
        Ok(Attached { key, offset })
    }

    /// Tells the session that the consumer `key` has finished with `bytes`
    /// bytes of the output it was given. Ignored from a consumer the
    /// session has left (what it had not acknowledged then already counts
    /// as acknowledged), and beyond what it was given.
    pub fn acknowledge(&self, key: ConsumerKey, bytes: usize) {
        let shared = self.shared();
        let mut state = shared.state();
        let Some(current) = state.consumer.as_mut().filter(|current| current.key == key) else {
            return;
        };
        let bytes = bytes.min(current.unacknowledged);
        current.unacknowledged -= bytes;
        shared.flow.acknowledge(bytes);
    }

    /// Lets go of whichever consumer is shown the session (one whose
    /// window died without releasing it, #84), as [`Self::release`] does;
    /// returns the output delivered so far in all, in bytes.
    pub fn let_go(&self) -> u64 {
        let shared = self.shared();
        let mut state = shared.state();
        shared.let_go(&mut state);
        state.delivered
    }

    /// The last state delivered to a consumer (any of them): where the
    /// session stands for one taking it now, before the events that wait
    /// for it. `None` until one was delivered.
    pub fn shown_state(&self) -> Option<SessionState> {
        self.shared().state().shown_state.clone()
    }

    /// The consumer shown the session now, if any.
    pub fn consumer(&self) -> Option<ConsumerKey> {
        self.shared()
            .state()
            .consumer
            .as_ref()
            .map(|current| current.key)
    }
}

impl fmt::Debug for Relay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Relay")
            .field("consumer", &self.consumer())
            .finish_non_exhaustive()
    }
}

/// The relay's task: takes each event and delivers it to whoever is shown
/// the session, waiting for somebody when nobody is.
/// Returning drops the events, which closes the session.
async fn deliver(mut events: SessionEvents, shared: Arc<Shared>) {
    loop {
        // `next` is cancel-safe: output gathered when abandoned is dropped
        // with the session.
        let event = tokio::select! {
            event = events.next() => event,
            () = abandoned(&shared) => return,
        };
        let Some(mut pending) = event else {
            return;
        };
        loop {
            match offer(&shared, pending) {
                Offered::Delivered => break,
                Offered::Abandoned => return,
                Offered::Waiting(event) => {
                    pending = event;
                    shared.wake.notified().await;
                }
                Offered::Gone(event, gone) => {
                    pending = event;
                    gone.consumer.gone();
                }
            }
        }
    }
}

/// Resolves once every handle to the relay has been dropped.
async fn abandoned(shared: &Shared) {
    loop {
        if shared.state().abandoned {
            return;
        }
        shared.wake.notified().await;
    }
}

/// What became of an event offered to the consumer.
enum Offered {
    Delivered,
    /// Nobody is shown the session: the event waits.
    Waiting(SessionEvent),
    /// The consumer has gone; the event waits for the next one.
    Gone(SessionEvent, Current),
    /// Every handle to the relay was dropped: nobody can acknowledge or
    /// attach anymore.
    Abandoned,
}

/// Offers `event` to the consumer shown the session, if any. Delivered
/// under the lock, so a release that returns has seen every delivery to the
/// consumer it released.
fn offer(shared: &Shared, event: SessionEvent) -> Offered {
    let mut state = shared.state();
    if state.abandoned {
        return Offered::Abandoned;
    }
    let Some(current) = state.consumer.as_mut() else {
        return Offered::Waiting(event);
    };
    let bytes = match &event {
        SessionEvent::Output(bytes) => bytes.len(),
        _ => 0,
    };
    let shown_state = match &event {
        SessionEvent::State(session_state) => Some(session_state.clone()),
        _ => None,
    };
    match current.consumer.deliver(event.clone()) {
        Ok(()) => {
            current.unacknowledged += bytes;
            state.delivered += bytes as u64;
            if shown_state.is_some() {
                state.shown_state = shown_state;
            }
            Offered::Delivered
        }
        Err(ConsumerGone) => {
            let gone = shared.let_go(&mut state).expect("just found");
            Offered::Gone(event, gone)
        }
    }
}
