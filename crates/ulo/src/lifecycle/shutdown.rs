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

use std::future::poll_fn;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant};

use async_lock::OnceCell;

use crate::app::shared::AppShared;
use crate::error::{FailureReason, Shutdown, ShutdownError, ShutdownFailure};
use crate::execution::notify::{Listen, Notify};
use crate::graph::{BindingId, Graph, ModuleId};
use crate::hooks::HookKind;
use crate::lifecycle::connect::{HookSite, hook_plan, run_hook, site_hooks, site_key};
use crate::lifecycle::phase::Phase;
use crate::lifecycle::run::{Cap, Outcome, select};
use crate::redact::redact;
use crate::signal::Signal;
use crate::timer::{BoxFuture, Timer};
use crate::transport::server::{DrainToken, ErasedServer};

/// The one shutdown of an app: the winning trigger's signal and the outcome every `serve` and
/// `close` caller receives.
pub(crate) struct ShutdownCell {
    signal: Mutex<Option<Signal>>,
    triggered: Notify,
    outcome: OnceCell<Result<Shutdown, ShutdownError>>,
    /// Where the sequence stands. A caller whose future is dropped mid-sequence leaves it here,
    /// and the next `serve` or `close` caller to take over the outcome resumes from it, so no
    /// hook runs twice.
    progress: async_lock::Mutex<Progress>,
}

impl ShutdownCell {
    pub(crate) fn new() -> Self {
        ShutdownCell {
            signal: Mutex::new(None),
            triggered: Notify::new(),
            outcome: OnceCell::new(),
            progress: async_lock::Mutex::new(Progress::default()),
        }
    }

    /// Records `signal` if no trigger has arrived yet; `true` when this call won.
    pub(crate) fn trigger(&self, signal: Signal) -> bool {
        {
            let mut slot = self.signal.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.is_some() {
                return false;
            }
            *slot = Some(signal);
        }
        self.triggered.fire();
        true
    }

    /// Resolves once any trigger has arrived; `serve` races it against its own signal.
    pub(crate) fn triggered(&self) -> Listen<'_> {
        self.triggered.listen()
    }

    pub(crate) fn winning_signal(&self) -> Option<Signal> {
        self.signal.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The sequence's position, kept across a dropped runner.
#[derive(Default)]
pub(crate) struct Progress {
    stage: Stage,
    cap: Option<Cap>,
    /// The step under way, as the hooks it runs in order, or nothing between hook steps.
    plan: Option<Vec<(HookSite, usize)>>,
    /// How many hooks of `plan`, or transports of the close step, have started. One that started
    /// is never started again.
    cursor: usize,
    accepting_stopped: bool,
    drain_end: Option<DrainEnd>,
    failures: Vec<ShutdownFailure>,
    abandoned: usize,
    terminal_abandoned: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Trigger,
    BeforeShutdown,
    Drain,
    Destroy,
    Close,
    Shutdown,
    Done,
}

#[derive(Clone, Copy, Debug)]
enum DrainEnd {
    Now,
    At(Instant),
    /// A drain window too long to put on the clock, with no cap to end it sooner.
    Open,
}

/// A `close` on the app or on any handle: triggers the shutdown if none has started, and either
/// way waits for the one outcome.
pub(crate) async fn close(shared: &Arc<AppShared>, signal: Signal) -> Result<Shutdown, ShutdownError> {
    shared.shutdown.trigger(signal.clone());
    run_sequence(shared, signal).await
}

/// Runs the seven steps once, for the winning signal, and stores the outcome. Whoever runs it
/// is the trigger's caller: `serve` when its signal won, otherwise the first `close`. A caller
/// arriving while it runs waits for the same outcome.
pub(crate) async fn run_sequence(shared: &Arc<AppShared>, signal: Signal) -> Result<Shutdown, ShutdownError> {
    shared.shutdown.trigger(signal.clone());
    let signal = shared.shutdown.winning_signal().unwrap_or(signal);
    shared.shutdown.outcome.get_or_init(|| sequence(shared, signal)).await.clone()
}

async fn sequence(shared: &Arc<AppShared>, signal: Signal) -> Result<Shutdown, ShutdownError> {
    let mut progress = shared.shutdown.progress.lock().await;
    let p = &mut *progress;

    if p.stage == Stage::Trigger {
        // The cap starts at the trigger, where the orchestrator's grace period starts too.
        p.cap = cap_from_now(shared.config.timer.as_deref(), shared.config.shutdown_timeout);
        shared.phase.advance(Phase::Stopping);
        p.stage = Stage::BeforeShutdown;
    }
    if p.stage == Stage::BeforeShutdown {
        run_hook_step(shared, HookKind::BeforeApplicationShutdown, &signal, p).await;
        p.stage = Stage::Drain;
    }
    if p.stage == Stage::Drain {
        let (abandoned, terminal) = drain(shared, p).await;
        p.abandoned = abandoned;
        p.terminal_abandoned = terminal;
        p.stage = Stage::Destroy;
    }
    if p.stage == Stage::Destroy {
        shared.phase.advance(Phase::Destroying);
        run_hook_step(shared, HookKind::OnModuleDestroy, &signal, p).await;
        p.stage = Stage::Close;
    }
    if p.stage == Stage::Close {
        close_servers(shared, p).await;
        p.stage = Stage::Shutdown;
    }
    if p.stage == Stage::Shutdown {
        run_hook_step(shared, HookKind::OnApplicationShutdown, &signal, p).await;
        shared.phase.advance(Phase::Closed);
        p.stage = Stage::Done;
    }

    // Read last: a terminal execution refused during the destroy, close or shutdown steps
    // counts too.
    let terminal_skipped = p.terminal_abandoned.saturating_add(shared.phase.terminal_refused());
    let report = Shutdown { signal, abandoned: p.abandoned, terminal_skipped };
    if p.failures.is_empty() {
        Ok(report)
    } else {
        Err(ShutdownError { report, failures: Arc::from(std::mem::take(&mut p.failures)) })
    }
}

/// One shutdown hook step over every binding and module, in reverse connect order, each hook
/// under its own bound and the cap. Failures are appended; the step never stops early.
pub(crate) async fn run_hook_step(shared: &Arc<AppShared>, kind: HookKind, signal: &Signal, progress: &mut Progress) {
    let graph = shared.graph();
    let timer = shared.config.timer.as_deref();
    let Progress { plan: slot, cursor, failures, cap, .. } = progress;
    let cap = *cap;
    let signal = matches!(kind, HookKind::BeforeApplicationShutdown | HookKind::OnApplicationShutdown).then_some(signal);
    let plan = slot.get_or_insert_with(|| shutdown_plan(&graph, kind));

    while let Some(&(site, index)) = plan.get(*cursor) {
        *cursor += 1;
        let Some(hook) = site_hooks(&graph, site).get(index) else { continue };
        let expired = match (cap, timer) {
            (Some(cap), Some(timer)) => cap.expired(timer),
            _ => false,
        };
        let reason = if expired {
            FailureReason::Skipped
        } else {
            match run_hook(shared, &graph, site, hook, signal, cap).await {
                None | Some(Outcome::Done(Ok(()))) => continue,
                // A closure hook whose site read failed: the hook itself returns `()`.
                Some(Outcome::Done(Err(e))) => FailureReason::Errored(redact(&graph.secrets, e)),
                Some(Outcome::Panicked(p)) => FailureReason::Panicked(p),
                Some(Outcome::TimedOut { after, limit }) => FailureReason::TimedOut { after, limit },
            }
        };
        failures.push(ShutdownFailure::Hook { hook: kind, key: site_key(&graph, site), reason });
    }
    *slot = None;
    *cursor = 0;
}

/// Steps 2 to 4: stop accepting, the drain, then cancel and abandon. Answers
/// `(abandoned, terminal abandoned)` for the report.
///
/// Each transport's `Server::drain` future runs inside the drain window beside the wait for live
/// executions: the window ends once every one of them has completed and no execution is live,
/// or at its deadline, where a transport's unfinished drain future is dropped.
pub(crate) async fn drain(shared: &Arc<AppShared>, progress: &mut Progress) -> (usize, usize) {
    let timer = shared.config.timer.as_deref();
    let servers = servers_of(shared);
    let mut drains: Vec<BoxFuture<'_, ()>> = Vec::new();
    if !progress.accepting_stopped {
        progress.accepting_stopped = true;
        shared.phase.advance(Phase::Draining);
        shared.draining.fire();
        progress.drain_end = Some(drain_end(timer, shared.config.drain(), progress.cap));
        let token = DrainToken::new();
        drains = servers.iter().map(|server| server.drain(token.clone())).collect();
    }

    let settled = async {
        settle(drains).await;
        shared.live.until_empty().await;
    };
    match (progress.drain_end.unwrap_or(DrainEnd::Now), timer) {
        (DrainEnd::Open, _) => settled.await,
        (DrainEnd::At(at), Some(timer)) => {
            let left = at.saturating_duration_since(timer.now());
            if !left.is_zero() {
                select(settled, timer.sleep(left)).await;
            }
        }
        _ => {}
    }

    // Ending the drain before cancelling refuses a terminal execution that would otherwise open
    // after the cancellation and outlive it uncounted.
    shared.phase.end_drain();
    shared.live.cancel_all()
}

async fn settle(mut pending: Vec<BoxFuture<'_, ()>>) {
    poll_fn(|cx| {
        pending.retain_mut(|f| f.as_mut().poll(cx).is_pending());
        if pending.is_empty() { Poll::Ready(()) } else { Poll::Pending }
    })
    .await
}

/// Step 6, in reverse bind order. Neither the cap nor a bound applies: closing runs no user code.
async fn close_servers(shared: &Arc<AppShared>, progress: &mut Progress) {
    let servers = servers_of(shared);
    while progress.cursor < servers.len() {
        let server = &servers[servers.len() - 1 - progress.cursor];
        progress.cursor += 1;
        if let Err(e) = server.close().await {
            let source = redact(&shared.graph().secrets, e);
            progress.failures.push(ShutdownFailure::Close { transport: server.transport_name(), source });
        }
    }
    progress.cursor = 0;
}

fn servers_of(shared: &AppShared) -> Vec<Arc<dyn ErasedServer>> {
    shared.servers.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The hooks of `kind` in the order shutdown runs them: each group of lazily loaded modules,
/// latest load first, then the base graph, each group in the exact reverse of its startup order.
fn shutdown_plan(graph: &Graph, kind: HookKind) -> Vec<(HookSite, usize)> {
    let mut loads: Vec<Option<u32>> = graph.modules.iter().map(|m| m.loaded).collect();
    loads.sort_unstable();
    loads.dedup();

    let mut entries = Vec::new();
    // `None`, the base graph, sorts first, so the reverse visits it last.
    for load in loads.into_iter().rev() {
        let singletons: Vec<BindingId> = graph
            .connect_order
            .iter()
            .copied()
            .filter(|&id| graph.module(graph.binding(id).origin).loaded == load)
            .collect();
        let modules: Vec<ModuleId> = graph.modules.iter().filter(|m| m.loaded == load).map(|m| m.id).collect();
        for site in hook_plan(graph, &singletons, &modules).into_iter().rev() {
            for (index, hook) in site_hooks(graph, site).iter().enumerate().rev() {
                if hook.kind == kind {
                    entries.push((site, index));
                }
            }
        }
    }
    entries
}

fn cap_from_now(timer: Option<&dyn Timer>, limit: Option<Duration>) -> Option<Cap> {
    let (timer, after) = (timer?, limit?);
    Some(Cap { at: timer.now().checked_add(after)?, after })
}

fn drain_end(timer: Option<&dyn Timer>, window: Duration, cap: Option<Cap>) -> DrainEnd {
    let Some(timer) = timer else { return DrainEnd::Now };
    match (timer.now().checked_add(window), cap) {
        (Some(at), Some(cap)) => DrainEnd::At(at.min(cap.at)),
        (Some(at), None) => DrainEnd::At(at),
        (None, Some(cap)) => DrainEnd::At(cap.at),
        (None, None) => DrainEnd::Open,
    }
}
