//! The one runner for user code the core waits on: a hook, a construction, a readiness attempt.
//! It applies the item's resolved bound and the shutdown cap through the app's `Timer`, and
//! catches panics at the poll boundary (§3.9, §10.2).
//!
//! Expiry drops the future: the item stops at its current await with its own state as it was,
//! and anything it spawned keeps running. Under `panic = "abort"` nothing can be caught.

use std::any::Any;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::error::{FailureReason, Limit};
use crate::redact::{Redacted, SecretRegistry, redact_panic};
use crate::timer::{BoxFuture, Resolved, Timer};

/// The `shutdown_timeout` cap, counted from the trigger. A hook still running when it expires
/// reports `TimedOut { limit: ShutdownCap, after }`, whatever its own bound was.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cap {
    pub(crate) at: Instant,
    pub(crate) after: Duration,
}

impl Cap {
    /// Whether the cap has run out on `timer`'s clock.
    pub(crate) fn expired(&self, timer: &dyn Timer) -> bool {
        timer.now() >= self.at
    }
}

pub(crate) enum Outcome<T> {
    Done(T),
    Panicked(Redacted),
    TimedOut { after: Duration, limit: Limit },
}

impl<T> Outcome<T> {
    /// The reason a non-`Done` outcome reports.
    pub(crate) fn into_failure(self) -> Result<T, FailureReason> {
        match self {
            Outcome::Done(v) => Ok(v),
            Outcome::Panicked(p) => Err(FailureReason::Panicked(p)),
            Outcome::TimedOut { after, limit } => Err(FailureReason::TimedOut { after, limit }),
        }
    }
}

/// Runs `fut` under `bound` and, at shutdown, `cap`; whichever limit fires first is the one
/// reported. Without a `Timer` only `Resolved::Unbounded` reaches here, the environment check
/// having refused every explicit bound.
///
/// The item is polled before either limit, so an item that completes on the poll where a limit
/// fires counts as done; the cap is polled before the item's own bound, so a tie reports
/// `ShutdownCap`.
pub(crate) async fn run<F: Future>(
    timer: Option<&dyn Timer>,
    bound: Resolved,
    cap: Option<Cap>,
    secrets: &SecretRegistry,
    fut: F,
) -> Outcome<F::Output> {
    let mut item = pin!(CatchUnwind::new(fut));
    let mut own: Option<(BoxFuture<'static, ()>, Duration, Limit)> = match (timer, bound) {
        (Some(t), Resolved::After { after, limit }) => Some((t.sleep(after), after, limit)),
        _ => None,
    };
    let mut capped: Option<(BoxFuture<'static, ()>, Duration)> = match (timer, cap) {
        (Some(t), Some(c)) => Some((t.sleep(c.at.saturating_duration_since(t.now())), c.after)),
        _ => None,
    };

    let ended = poll_fn(|cx| {
        if let Poll::Ready(out) = item.as_mut().poll(cx) {
            return Poll::Ready(Ok(out));
        }
        if let Some((sleep, after)) = capped.as_mut() {
            if sleep.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err((*after, Limit::ShutdownCap)));
            }
        }
        if let Some((sleep, after, limit)) = own.as_mut() {
            if sleep.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err((*after, *limit)));
            }
        }
        Poll::Pending
    })
    .await;

    match ended {
        Ok(Ok(value)) => Outcome::Done(value),
        Ok(Err(payload)) => Outcome::Panicked(redact_panic(secrets, payload)),
        Err((after, limit)) => Outcome::TimedOut { after, limit },
    }
}

/// Polls the inner future inside `catch_unwind`, answering the payload on a panic. A future that
/// panicked must not be polled again; every caller drops it on the payload.
///
/// The inner future is boxed so the wrapper is `Unpin` and needs no pin projection.
pub(crate) struct CatchUnwind<F> {
    inner: Pin<Box<F>>,
}

impl<F> CatchUnwind<F> {
    pub(crate) fn new(inner: F) -> Self {
        CatchUnwind { inner: Box::pin(inner) }
    }
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // A future that panicked is never polled again, so no broken invariant inside it is
        // observed afterwards; only its drop runs.
        match catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

/// Polls both, answering whichever completes first; the other is dropped. `a` is polled first,
/// so it wins a tie.
pub(crate) async fn select<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let mut a = pin!(a);
    let mut b = pin!(b);
    poll_fn(|cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(v));
        }
        if let Poll::Ready(v) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(v));
        }
        Poll::Pending
    })
    .await
}

pub(crate) enum Either<A, B> {
    Left(A),
    Right(B),
}
