//! The one runner for user code the core waits on: a hook, a construction, a readiness attempt.
//! It applies the item's resolved bound and the shutdown cap through the app's `Timer`, and
//! catches panics at the poll boundary (§3.9, §10.2).
//!
//! Expiry drops the future: the item stops at its current await with its own state as it was,
//! and anything it spawned keeps running. Under `panic = "abort"` nothing can be caught.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::error::{FailureReason, Limit};
use crate::redact::{Redacted, SecretRegistry};
use crate::timer::{Resolved, Timer};

/// The `shutdown_timeout` cap, counted from the trigger. A hook still running when it expires
/// reports `TimedOut { limit: ShutdownCap, after }`, whatever its own bound was.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cap {
    pub(crate) at: Instant,
    pub(crate) after: Duration,
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
pub(crate) async fn run<F: Future>(
    timer: Option<&dyn Timer>,
    bound: Resolved,
    cap: Option<Cap>,
    secrets: &SecretRegistry,
    fut: F,
) -> Outcome<F::Output> {
    todo!()
}

/// Polls the inner future inside `catch_unwind`, answering the payload on a panic.
pub(crate) struct CatchUnwind<F> {
    inner: F,
}

impl<F> CatchUnwind<F> {
    pub(crate) fn new(inner: F) -> Self {
        CatchUnwind { inner }
    }
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

/// Polls both, answering whichever completes first; the other is dropped.
pub(crate) async fn select<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    todo!()
}

pub(crate) enum Either<A, B> {
    Left(A),
    Right(B),
}
