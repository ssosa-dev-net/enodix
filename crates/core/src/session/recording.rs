//! A stream that keeps a copy of the first bytes it reads: what the device
//! sent first (its version line and key-exchange init), so that a failed
//! negotiation can report what the device offered. `russh` reports only the
//! one list that had nothing in common.

use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// How many bytes are kept: the version line and the key-exchange init
/// fit in a few kilobytes; the limit is a packet's maximum size and more.
const KEPT: usize = 64 * 1024;

/// The first bytes read, shared with whoever wants them afterwards.
#[derive(Clone)]
pub(super) struct FirstBytes(Arc<Mutex<Option<Vec<u8>>>>);

impl FirstBytes {
    pub(super) fn new() -> Self {
        Self(Arc::new(Mutex::new(Some(Vec::new()))))
    }

    /// Whether nothing has been read yet (or the bytes were taken).
    pub(super) fn is_empty(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_none_or(Vec::is_empty)
    }

    /// The bytes kept so far; nothing more is kept after this.
    pub(super) fn take(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_default()
    }
}

pub(super) struct Recording<S> {
    inner: S,
    first: FirstBytes,
}

impl<S> Recording<S> {
    pub(super) fn new(inner: S, first: FirstBytes) -> Self {
        Self { inner, first }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Recording<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            let mut kept = self.first.0.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(kept) = kept.as_mut() {
                let room = KEPT.saturating_sub(kept.len());
                let new = &buf.filled()[before..];
                kept.extend_from_slice(&new[..new.len().min(room)]);
            }
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Recording<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
