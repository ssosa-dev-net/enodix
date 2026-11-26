//! Watching a session's output beside its consumer (#23).
//!
//! A session's output goes to one consumer (the window showing it), which
//! acknowledges it for flow control. Something that must also see it, such
//! as a snippet's steps waiting for the device's prompt, takes an
//! [`OutputWatch`]: a copy of every byte read from the device from then on,
//! in order, until the session's connection ends. A watch takes no part in
//! flow control: it never acknowledges, and never holds reading back; if it
//! falls more than [`WATCH_LIMIT`] behind, the oldest bytes it has not taken
//! are dropped (it only ever looks at the latest output).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use tokio::sync::Notify;

/// The most output a watch holds that it has not taken yet.
pub const WATCH_LIMIT: usize = 1024 * 1024;

/// A copy of a session's output, from when it was taken until the
/// connection ends. Made by [`super::SessionManager::watch_output`].
pub struct OutputWatch {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Raised on output and on the end.
    changed: Notify,
}

#[derive(Default)]
struct State {
    pending: Vec<u8>,
    closed: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl OutputWatch {
    /// The output read since the last call (at least one byte), waiting
    /// for some; `None` once the connection has ended and everything read
    /// before was taken. Cancel-safe.
    pub async fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            {
                let mut state = self.shared.lock();
                if !state.pending.is_empty() {
                    return Some(std::mem::take(&mut state.pending));
                }
                if state.closed {
                    return None;
                }
            }
            // One waiter per watch: a raise with nobody waiting is kept for
            // this wait.
            self.shared.changed.notified().await;
        }
    }
}

impl std::fmt::Debug for OutputWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputWatch").finish_non_exhaustive()
    }
}

/// The watches of one connection of a session.
pub(super) struct Watchers {
    /// `None` once the connection has ended.
    watches: Mutex<Option<Vec<Weak<Shared>>>>,
}

impl Watchers {
    pub(super) fn new() -> Self {
        Self {
            watches: Mutex::new(Some(Vec::new())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Vec<Weak<Shared>>>> {
        self.watches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new watch; one taken after the connection ended ends at once.
    pub(super) fn watch(&self) -> OutputWatch {
        let shared = Arc::new(Shared::default());
        match &mut *self.lock() {
            Some(watches) => watches.push(Arc::downgrade(&shared)),
            None => shared.lock().closed = true,
        }
        OutputWatch { shared }
    }

    /// Gives each watch still held a copy of `bytes`.
    pub(super) fn output(&self, bytes: &[u8]) {
        let mut watches = self.lock();
        let Some(watches) = watches.as_mut().filter(|watches| !watches.is_empty()) else {
            return;
        };
        watches.retain(|watch| {
            let Some(shared) = watch.upgrade() else {
                return false;
            };
            {
                let mut state = shared.lock();
                state.pending.extend_from_slice(bytes);
                let over = state.pending.len().saturating_sub(WATCH_LIMIT);
                if over > 0 {
                    state.pending.drain(..over);
                }
            }
            shared.changed.notify_one();
            true
        });
    }

    /// The connection ended: every watch ends once it has taken what it was
    /// given.
    pub(super) fn close(&self) {
        let watches = self.lock().take().unwrap_or_default();
        for watch in watches {
            if let Some(shared) = watch.upgrade() {
                shared.lock().closed = true;
                shared.changed.notify_one();
            }
        }
    }
}
