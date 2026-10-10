//! A count of the connections a host's server has read from, for [`Host::connections_read`]: an
//! embedding host wraps each stream its listener accepts with [`ReadCount::wrap`] before handing
//! it to the host framework, as `ulo-hyper-serve` counts for the hyper backend.
//!
//! [`Host::connections_read`]: crate::Host::connections_read

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// How many wrapped streams have been read from, each counted at its first read that yields
/// bytes. Cheap to clone; every clone reads the same count.
#[derive(Clone, Debug, Default)]
pub struct ReadCount(Arc<AtomicUsize>);

impl ReadCount {
    /// The streams read from so far.
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }

    /// `stream`, counted here at its first read that yields bytes.
    pub fn wrap<S>(&self, stream: S) -> Counted<S> {
        Counted { stream, unread: Some(self.clone()) }
    }
}

/// A stream [`ReadCount::wrap`] counts, otherwise the stream it wraps.
pub struct Counted<S> {
    stream: S,
    unread: Option<ReadCount>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Counted<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let read = Pin::new(&mut this.stream).poll_read(cx, buf);
        if matches!(read, Poll::Ready(Ok(()))) && buf.filled().len() > before && let Some(count) = this.unread.take() {
            count.0.fetch_add(1, Ordering::AcqRel);
        }
        read
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counted<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
