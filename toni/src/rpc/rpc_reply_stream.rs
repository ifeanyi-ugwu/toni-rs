use futures::channel::mpsc;

use super::{RpcClientError, RpcData};

/// The transport half of an [`RpcReplyStream`]: the reply router feeds items
/// in and drops it at the end marker so the stream finishes.
pub type ReplySink = mpsc::Sender<Result<RpcData, RpcClientError>>;

/// The replies of one streaming call, as returned by
/// [`RpcClient::stream`](super::RpcClient::stream).
///
/// Items arrive until the server's end marker; an error end arrives as one
/// `Err` item and finishes the stream. Dropping this before the end sends the
/// call's cancel notice upstream, so the server drops the producing stream
/// and the handler's execution hears its cancellation token.
///
/// The per-frame deadline is the transport's: its `with_timeout` bounds the
/// gap to the next frame, the first included, delivered as an `Err` item.
pub struct RpcReplyStream {
    rx: mpsc::Receiver<Result<RpcData, RpcClientError>>,
    /// Set on exhaustion or on an `Err` item — either is terminal, and a
    /// terminal the transport produced or relayed needs no cancel notice.
    ended: bool,
    on_cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl RpcReplyStream {
    /// Open a stream and its feeding sink. `on_cancel` runs when the stream
    /// is dropped before its end — the transport sends the cancel notice
    /// there.
    pub fn channel(
        capacity: usize,
        on_cancel: impl FnOnce() + Send + 'static,
    ) -> (ReplySink, Self) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            tx,
            Self {
                rx,
                ended: false,
                on_cancel: Some(Box::new(on_cancel)),
            },
        )
    }
}

impl futures::Stream for RpcReplyStream {
    type Item = Result<RpcData, RpcClientError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.rx).poll_next(cx);
        if matches!(
            polled,
            std::task::Poll::Ready(None) | std::task::Poll::Ready(Some(Err(_)))
        ) {
            this.ended = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.rx.size_hint()
    }
}

impl std::fmt::Debug for RpcReplyStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RpcReplyStream(ended: {})", self.ended)
    }
}

/// Dropped with frames still owed, the caller has walked away from a live
/// stream — the one case the server cannot see from its side.
impl Drop for RpcReplyStream {
    fn drop(&mut self) {
        if !self.ended {
            if let Some(on_cancel) = self.on_cancel.take() {
                on_cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::{SinkExt, StreamExt};

    use super::*;

    fn counted_channel() -> (ReplySink, RpcReplyStream, Arc<AtomicUsize>) {
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = fired.clone();
        let (tx, stream) = RpcReplyStream::channel(8, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (tx, stream, fired)
    }

    #[test]
    fn drop_before_the_end_sends_the_cancel_notice_once() {
        let (mut tx, stream, fired) = counted_channel();
        futures_executor::block_on(async {
            tx.send(Ok(RpcData::text("one"))).await.unwrap();
        });
        drop(stream);
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_drained_stream_sends_no_notice() {
        let (mut tx, mut stream, fired) = counted_channel();
        futures_executor::block_on(async {
            tx.send(Ok(RpcData::text("one"))).await.unwrap();
            drop(tx);
            assert!(stream.next().await.is_some());
            assert!(stream.next().await.is_none());
        });
        drop(stream);
        assert_eq!(fired.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_err_item_is_terminal() {
        let (mut tx, mut stream, fired) = counted_channel();
        futures_executor::block_on(async {
            tx.send(Err(RpcClientError::Remote {
                message: "boom".into(),
                status: "error".into(),
            }))
            .await
            .unwrap();
            assert!(matches!(stream.next().await, Some(Err(_))));
        });
        drop(stream);
        assert_eq!(fired.load(Ordering::SeqCst), 0);
    }
}
