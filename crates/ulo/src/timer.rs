use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::error::Limit;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// The app's one clock. Every wait the core times, the drain, a hook, a construction, a
/// readiness check, runs through it; it is set on the builder and supplied by a runtime adapter.
///
/// `sleep` returns a boxed `'static` future so the trait stays dyn-compatible and a caller can
/// spawn what it returns. `now` sits on the same trait because deadlines and sleeps have to read
/// one clock: measured with `std::time::Instant`, a deadline on a paused test clock never
/// arrives while sleeps keep resolving. When a `Timer` is configured, services read it as
/// `Dep<dyn Timer>`.
pub trait Timer: Send + Sync + 'static {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
    fn now(&self) -> Instant;
}

/// How long a hook, a construction or a readiness check may take.
///
/// `Default` takes the app's default for that kind of item: `hook_timeout` (10 s) for a hook,
/// `construct_timeout` (30 s) for a construction and for one attempt of a readiness check. The
/// defaults apply only when a `Timer` is configured; without one an item left at `Default` runs
/// unbounded. `After(d)` needs a `Timer` and is a wiring error without one. `Unbounded` needs
/// none and is still contained by `shutdown_timeout` at shutdown.
///
/// `Unbounded` is a variant rather than `After(Duration::MAX)` because `Instant + Duration::MAX`
/// panics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Bound {
    #[default]
    Default,
    After(Duration),
    Unbounded,
}

/// The kind of item a bound applies to, which picks the app default `Bound::Default` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundKind {
    Hook,
    Construction,
    /// One attempt of a readiness check.
    ReadinessAttempt,
    /// A readiness check as a whole, retries and backoff included. Its `Default` is no bound:
    /// only the attempt takes `construct_timeout`.
    ReadinessWhole,
}

/// A bound after the app defaults and the presence of a `Timer` are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// Expiry reports `TimedOut { after, limit }`.
    After { after: Duration, limit: Limit },
    Unbounded,
}

/// The app defaults a `Bound::Default` resolves against.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Defaults {
    pub(crate) hook_timeout: Duration,
    pub(crate) construct_timeout: Duration,
}

/// Applies the §3.9 rule: an explicit bound is `Limit::Item` (or `Limit::Attempt` for a
/// readiness attempt), `Default` is the app default only when a `Timer` exists, `Unbounded`
/// is none. The environment pass has already refused an explicit bound with no `Timer`.
pub(crate) fn resolve_bound(bound: Bound, kind: BoundKind, defaults: Option<&Defaults>) -> Resolved {
    todo!()
}

/// `fut` raced against `timer.sleep(d)`: `Err(())` when the sleep wins, dropping `fut` at its
/// current await.
pub(crate) async fn timeout<F: Future>(timer: &dyn Timer, d: Duration, fut: F) -> Result<F::Output, ()> {
    todo!()
}
