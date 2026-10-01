//! Shutdown: one event per app, two triggers, one outcome for every receiver (§9.5).
//!
//! The sequence:
//! 1. `BeforeApplicationShutdown(signal)` hooks, reverse connect order, while the app serves.
//! 2. Stop accepting: phase Draining, `draining()` fires, each transport's `Server::drain`
//!    receives a `DrainToken`.
//! 3. Drain: wait for live executions to end on their own, up to `drain_timeout` (zero without a
//!    `Timer`) or the earlier expiry of the cap.
//! 4. Cancel and abandon the rest, counting them in `Shutdown::abandoned`.
//! 5. `OnModuleDestroy` hooks, reverse connect order; phase Destroying from the first.
//! 6. Transports close their sockets.
//! 7. `OnApplicationShutdown(signal)` hooks, reverse connect order.
//!
//! Lazily loaded modules come first in every hook step, in reverse load order. A failing step
//! never stops the sequence. When `shutdown_timeout` expires, hooks not yet run are `Skipped`, a
//! hook mid-run is dropped as `TimedOut { limit: ShutdownCap }`, and an open drain ends at once;
//! stop accepting and the socket close still run.

use std::sync::{Arc, Mutex};

use async_lock::OnceCell;

use crate::app::shared::AppShared;
use crate::error::{Shutdown, ShutdownError, ShutdownFailure};
use crate::execution::notify::{Listen, Notify};
use crate::hooks::HookKind;
use crate::lifecycle::run::Cap;
use crate::signal::Signal;

/// The one shutdown of an app: the winning trigger's signal and the outcome every `serve` and
/// `close` caller receives.
pub(crate) struct ShutdownCell {
    signal: Mutex<Option<Signal>>,
    triggered: Notify,
    outcome: OnceCell<Result<Shutdown, ShutdownError>>,
}

impl ShutdownCell {
    pub(crate) fn new() -> Self {
        ShutdownCell { signal: Mutex::new(None), triggered: Notify::new(), outcome: OnceCell::new() }
    }

    /// Records `signal` if no trigger has arrived yet; `true` when this call won.
    pub(crate) fn trigger(&self, signal: Signal) -> bool {
        todo!()
    }

    /// Resolves once any trigger has arrived; `serve` races it against its own signal.
    pub(crate) fn triggered(&self) -> Listen<'_> {
        self.triggered.listen()
    }

    pub(crate) fn winning_signal(&self) -> Option<Signal> {
        todo!()
    }
}

/// A `close` on the app or on any handle: triggers the shutdown if none has started, and either
/// way waits for the one outcome.
pub(crate) async fn close(shared: &Arc<AppShared>, signal: Signal) -> Result<Shutdown, ShutdownError> {
    todo!()
}

/// Runs the seven steps once, for the winning signal, and stores the outcome. Whoever runs it
/// is the trigger's caller: `serve` when its signal won, otherwise the first `close`.
pub(crate) async fn run_sequence(shared: &Arc<AppShared>, signal: Signal) -> Result<Shutdown, ShutdownError> {
    todo!()
}

/// One shutdown hook step over every binding and module, in reverse connect order, each hook
/// under its own bound and the cap. Failures are appended; the step never stops early.
pub(crate) async fn run_hook_step(
    shared: &Arc<AppShared>,
    kind: HookKind,
    signal: &Signal,
    cap: Option<Cap>,
    failures: &mut Vec<ShutdownFailure>,
) {
    todo!()
}

/// Steps 2 to 4: stop accepting, the drain, then cancel and abandon. Answers
/// `(abandoned, terminal_skipped)` for the report.
pub(crate) async fn drain(shared: &Arc<AppShared>, cap: Option<Cap>) -> (usize, usize) {
    todo!()
}
