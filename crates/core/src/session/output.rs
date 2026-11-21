//! The output pipeline: how session output reaches whoever displays it.
//!
//! Output is batched on its way out. After a quiet spell the first output is
//! delivered at once, so keystroke echo is never held back. While output keeps
//! coming it is gathered into batches, each delivered when it reaches
//! [`OutputOptions::batch_size`] or when [`OutputOptions::batch_interval`] has
//! passed since the previous delivery, whichever comes first.
//!
//! Flow control: the consumer acknowledges output as it finishes with it (for
//! the interface, once the terminal has consumed it). When
//! [`OutputOptions::window`] bytes are read from the device but not yet
//! acknowledged, the session stops reading from the device until
//! acknowledgements make room. Input, resizes and close keep working while
//! reading is paused.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use super::SessionEvent;

/// How a session's output is batched and how far its consumer may fall behind.
///
/// After a quiet spell (no delivery within `batch_interval`) output is
/// delivered at once; while it keeps coming it is batched. Output must be
/// acknowledged once consumed; `window` unacknowledged bytes pause reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputOptions {
    /// While output keeps arriving, the longest a batch waits after the
    /// previous delivery before it is delivered.
    pub batch_interval: Duration,
    /// A batch this large is delivered without waiting.
    pub batch_size: usize,
    /// How many bytes may be read from the device and not yet acknowledged
    /// before the session stops reading. At least 1.
    pub window: usize,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            batch_interval: Duration::from_millis(10),
            batch_size: 64 * 1024,
            window: 256 * 1024,
        }
    }
}

/// The stream of output and state changes of one session.
///
/// Output arrives batched (see [`OutputOptions`]) and must be
/// acknowledged with [`SessionEvents::acknowledge`] or
/// [`SessionManager::acknowledge`](super::SessionManager::acknowledge) once
/// consumed, or the session stops reading from the device. The stream ends
/// after the `Closed` state. Dropping it closes the session.
pub struct SessionEvents {
    events: mpsc::UnboundedReceiver<SessionEvent>,
    flow: Arc<Flow>,
    options: OutputOptions,
    /// Output gathered but not yet delivered.
    batch: Vec<u8>,
    /// A state change or prompt that arrived behind `batch`, delivered
    /// after it.
    held: Option<SessionEvent>,
    last_delivery: Option<Instant>,
}

impl SessionEvents {
    pub(super) fn new(
        events: mpsc::UnboundedReceiver<SessionEvent>,
        flow: Arc<Flow>,
        options: OutputOptions,
    ) -> Self {
        Self {
            events,
            flow,
            options,
            batch: Vec::new(),
            held: None,
            last_delivery: None,
        }
    }

    /// The next event, or `None` once the session has closed.
    ///
    /// Cancel-safe: output gathered by a call that is dropped is kept for the
    /// next one.
    pub async fn next(&mut self) -> Option<SessionEvent> {
        if let Some(event) = self.held.take() {
            return Some(event);
        }
        loop {
            // Take whatever is already waiting, up to a full batch.
            while self.batch.len() < self.options.batch_size {
                match self.events.try_recv() {
                    Ok(SessionEvent::Output(bytes)) => self.gather(bytes),
                    Ok(event) => return Some(self.deliver_before(event)),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return self.deliver(),
                }
            }

            if self.batch.is_empty() {
                match self.events.recv().await {
                    Some(SessionEvent::Output(bytes)) => self.gather(bytes),
                    other => return other,
                }
                continue;
            }

            // After a quiet spell the batch is due at once.
            let due = self
                .last_delivery
                .map_or_else(Instant::now, |last| last + self.options.batch_interval);
            if self.batch.len() >= self.options.batch_size || Instant::now() >= due {
                return self.deliver();
            }
            tokio::select! {
                event = self.events.recv() => match event {
                    Some(SessionEvent::Output(bytes)) => self.gather(bytes),
                    Some(event) => return Some(self.deliver_before(event)),
                    None => return self.deliver(),
                },
                () = tokio::time::sleep_until(due) => return self.deliver(),
            }
        }
    }

    /// Tells the session that `bytes` bytes of output have been consumed.
    pub fn acknowledge(&self, bytes: usize) {
        self.flow.acknowledge(bytes);
    }

    /// The session's flow control, for a relay that acknowledges on its
    /// consumers' behalf.
    pub(super) fn flow(&self) -> Arc<Flow> {
        self.flow.clone()
    }

    fn gather(&mut self, bytes: Vec<u8>) {
        if self.batch.is_empty() {
            self.batch = bytes;
        } else {
            self.batch.extend_from_slice(&bytes);
        }
    }

    /// The gathered output, if any.
    fn deliver(&mut self) -> Option<SessionEvent> {
        if self.batch.is_empty() {
            return None;
        }
        self.last_delivery = Some(Instant::now());
        Some(SessionEvent::Output(std::mem::take(&mut self.batch)))
    }

    /// The gathered output, keeping `event` (not output) for the next call;
    /// or `event` itself if nothing was gathered.
    fn deliver_before(&mut self, event: SessionEvent) -> SessionEvent {
        match self.deliver() {
            Some(output) => {
                self.held = Some(event);
                output
            }
            None => event,
        }
    }
}

impl fmt::Debug for SessionEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionEvents").finish_non_exhaustive()
    }
}

/// Counts output read from the device but not yet acknowledged, and lets the
/// transport wait until there is room to read more.
pub(super) struct Flow {
    unacknowledged: AtomicUsize,
    window: usize,
    room: Notify,
}

impl Flow {
    pub(super) fn new(window: usize) -> Self {
        Self {
            unacknowledged: AtomicUsize::new(0),
            window: window.max(1),
            room: Notify::new(),
        }
    }

    /// Records output read from the device.
    pub(super) fn read(&self, bytes: usize) {
        self.unacknowledged.fetch_add(bytes, Ordering::AcqRel);
    }

    /// Records output the consumer has finished with. Acknowledging more than
    /// was read is ignored.
    pub(super) fn acknowledge(&self, bytes: usize) {
        let mut current = self.unacknowledged.load(Ordering::Acquire);
        loop {
            let next = current.saturating_sub(bytes);
            match self.unacknowledged.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        self.room.notify_one();
    }

    /// Whether the consumer is less than a window behind: reading may go
    /// on now.
    pub(super) fn has_room(&self) -> bool {
        self.unacknowledged.load(Ordering::Acquire) < self.window
    }

    /// Resolves once the consumer is less than a window behind.
    pub(super) async fn room_to_read(&self) {
        // Only the session's transport waits here, so `notify_one`'s stored
        // permit covers an acknowledgement that lands between the check and
        // the wait.
        while self.unacknowledged.load(Ordering::Acquire) >= self.window {
            self.room.notified().await;
        }
    }
}
