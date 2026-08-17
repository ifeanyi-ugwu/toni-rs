use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use event_listener::Event;

/// A per-request cancellation primitive.
///
/// Created by the framework at the start of an execution and dropped when it
/// ends. Cheaply cloneable — handles share state via `Arc`.
///
/// Nothing in the framework signals it yet: firing on client disconnect needs a
/// producer in each adapter, since only the adapter knows the socket died.
///
/// Toni-native and runtime-agnostic on purpose: toni core does not depend on
/// any specific async runtime. Adapters that want to bridge into
/// `tokio_util::CancellationToken` can do so externally.
///
/// # Example
///
/// ```
/// use toni::context::CancellationToken;
///
/// let token = CancellationToken::new();
/// let child = token.clone();
/// child.cancel();
/// assert!(token.is_cancelled());
/// // `token.cancelled().await` resolves immediately when the flag is set.
/// ```
#[derive(Clone, Debug)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    cancelled: AtomicBool,
    event: Event,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                cancelled: AtomicBool::new(false),
                event: Event::new(),
            }),
        }
    }

    /// Signal cancellation. Idempotent — only the first call has an effect;
    /// subsequent calls return without notifying.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.event.notify(usize::MAX);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once [`cancel`](Self::cancel) has been called.
    ///
    /// Compose with `tokio::select!` to race a future against cancellation.
    pub async fn cancelled(&self) {
        loop {
            if self.inner.cancelled.load(Ordering::SeqCst) {
                return;
            }
            // Listener registration must straddle the flag re-check to avoid a
            // wakeup race with a concurrent cancel().
            let listener = self.inner.event.listen();
            if self.inner.cancelled.load(Ordering::SeqCst) {
                return;
            }
            listener.await;
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_is_observable() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        token.cancelled().await;
    }

    #[tokio::test]
    async fn cancel_wakes_pending_waiter() {
        let token = CancellationToken::new();
        let child = token.clone();
        let waiter = tokio::spawn(async move {
            child.cancelled().await;
            "done"
        });
        // Yield so the waiter registers a listener before we cancel.
        tokio::task::yield_now().await;
        token.cancel();
        assert_eq!(waiter.await.unwrap(), "done");
    }

    #[tokio::test]
    async fn cancel_is_idempotent() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn clones_share_state() {
        let a = CancellationToken::new();
        let b = a.clone();
        a.cancel();
        assert!(b.is_cancelled());
    }
}
