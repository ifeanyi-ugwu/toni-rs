//! The connect phase (§9.2, §9.3): singletons in a stable topological order, each followed by
//! its readiness check, then every `OnModuleInit` hook in the same order with a module's own
//! hooks after its providers', then every `OnApplicationBootstrap` hook. The first failure
//! stops the walk and is returned as a `ConnectError`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::app::shared::AppShared;
use crate::binding::{Qualifier, ReadyRecord, Recipe};
use crate::construct::ConstructError;
use crate::error::{ConnectError, FailureReason, LookupError};
use crate::graph::{BindingId, FrozenBinding, Graph, ModuleId, Visible};
use crate::hooks::{HookCx, HookKind, HookRecord};
use crate::key::{BindingKind, Key, KeyName};
use crate::lifecycle::run::{Cap, Outcome, run};
use crate::module::ModuleName;
use crate::redact::{Redacted, SecretRegistry, redact};
use crate::resolver::{Purpose, Resolver};
use crate::signal::Signal;
use crate::timer::{BoxError, BoundKind, resolve_bound};

/// Connects `singletons` (in connect order) and the hooks of `modules` (in collection order).
/// `App::connect` passes the whole graph; `load` passes what the lazy module brought, after
/// swapping the extended graph in, since every step reads `shared.graph()`. The phase is the
/// caller's: `App::connect` moves it around this call, and `load` runs while the app is Running.
pub(crate) async fn connect(shared: &Arc<AppShared>, singletons: &[BindingId], modules: &[ModuleId]) -> Result<(), ConnectError> {
    for &id in singletons {
        if shared.singletons.get(id).is_none() {
            build_singleton(shared, id).await?;
        }
        let graph = shared.graph();
        if let Some(ready) = &graph.binding(id).record.ready {
            run_readiness(shared, id, ready).await?;
        }
    }
    run_startup_hooks(shared, HookKind::OnModuleInit, singletons, modules).await?;
    run_startup_hooks(shared, HookKind::OnApplicationBootstrap, singletons, modules).await?;
    Ok(())
}

/// Builds one singleton under its construction bound, panics caught, and stores it.
/// `Construct { key, module, reason }` on failure.
///
/// The store holds the instance as the recipe built it; a contribution's `into_primary` and the
/// `also_as` coercions are applied by whoever reads it under another key.
pub(crate) async fn build_singleton(shared: &Arc<AppShared>, id: BindingId) -> Result<(), ConnectError> {
    let graph = shared.graph();
    let binding = graph.binding(id);
    let ctor = match &binding.record.recipe {
        Recipe::Construct(ctor) | Recipe::Factory(ctor) => ctor,
        Recipe::Value(instance) => {
            shared.singletons.insert(id, Arc::clone(instance));
            return Ok(());
        }
        // An alias is read through its target and builds nothing; a failed `try_value` is
        // reported by `wire()`, and a graph holding one never reaches `connect`.
        Recipe::Alias { .. } | Recipe::Failed => return Ok(()),
    };

    let timer = shared.config.timer.as_deref();
    let defaults = shared.config.defaults();
    let bound = resolve_bound(binding.record.construct_bound, BoundKind::Construction, defaults.as_ref());
    let resolver = Resolver::new(shared, binding.origin, None, Purpose::Lookup);
    let reason = match run(timer, bound, None, &graph.secrets, (**ctor)(&resolver)).await {
        Outcome::Done(Ok(instance)) => {
            shared.singletons.insert(id, instance);
            return Ok(());
        }
        Outcome::Done(Err(ConstructError::Failed(e))) => FailureReason::Errored(redact(&graph.secrets, e)),
        Outcome::Done(Err(ConstructError::Site(e))) => return Err(site_failure(&graph, binding, e)),
        Outcome::Panicked(p) => FailureReason::Panicked(p),
        Outcome::TimedOut { after, limit } => FailureReason::TimedOut { after, limit },
    };
    Err(ConnectError::Construct { key: binding_key(binding), module: module_name(&graph, binding.origin), reason })
}

/// Runs a readiness check: attempts under the attempt bound, `retries` more after an `Err` or an
/// attempt timeout with `backoff` between, the whole under the check bound. A panicking attempt
/// ends the check at once. `Readiness { key, attempts, reason }` on failure, the reason by the
/// rules of §9.3.
pub(crate) async fn run_readiness(shared: &Arc<AppShared>, id: BindingId, ready: &ReadyRecord) -> Result<(), ConnectError> {
    let graph = shared.graph();
    let binding = graph.binding(id);
    let secrets = &graph.secrets;
    let timer = shared.config.timer.as_deref();
    let defaults = shared.config.defaults();
    // The record already carries §9.3's opt-out: a check that wrote `.timeout(..)` alone holds
    // `Unbounded` as its attempt bound.
    let whole = resolve_bound(ready.whole, BoundKind::ReadinessWhole, defaults.as_ref());
    let attempt = resolve_bound(ready.attempt, BoundKind::ReadinessAttempt, defaults.as_ref());
    let resolver = Resolver::new(shared, binding.origin, None, Purpose::Lookup);
    // Read after the whole-check bound may have dropped the loop mid-attempt.
    let attempts = AtomicU32::new(0);

    let checks = async {
        loop {
            let made = attempts.fetch_add(1, Ordering::Relaxed).saturating_add(1);
            let reason = match run(timer, attempt, None, secrets, (*ready.check)(&resolver)).await {
                Outcome::Done(Ok(())) => return Ok(()),
                Outcome::Done(Err(e)) => FailureReason::Errored(construct_error(secrets, e)),
                Outcome::Panicked(p) => return Err(FailureReason::Panicked(p)),
                Outcome::TimedOut { after, limit } => FailureReason::TimedOut { after, limit },
            };
            if made > ready.retries {
                return Err(reason);
            }
            // Without a `Timer` there is no clock to back off on, and the retry follows at once.
            if let Some(timer) = timer {
                if !ready.backoff.is_zero() {
                    timer.sleep(ready.backoff).await;
                }
            }
        }
    };

    let reason = match run(timer, whole, None, secrets, checks).await {
        Outcome::Done(Ok(())) => return Ok(()),
        Outcome::Done(Err(reason)) => reason,
        Outcome::Panicked(p) => FailureReason::Panicked(p),
        Outcome::TimedOut { after, limit } => FailureReason::TimedOut { after, limit },
    };
    Err(ConnectError::Readiness { key: binding_key(binding), attempts: attempts.load(Ordering::Relaxed), reason })
}

/// Every hook of `kind` on `singletons` and `modules`, in connect order with each module's own
/// hooks after its providers'. `Hook { hook, key, reason }` on the first failure.
pub(crate) async fn run_startup_hooks(
    shared: &Arc<AppShared>,
    kind: HookKind,
    singletons: &[BindingId],
    modules: &[ModuleId],
) -> Result<(), ConnectError> {
    let graph = shared.graph();
    for site in hook_plan(&graph, singletons, modules) {
        for hook in site_hooks(&graph, site).iter().filter(|h| h.kind == kind) {
            let reason = match run_hook(shared, &graph, site, hook, None, None).await {
                None | Some(Outcome::Done(Ok(()))) => continue,
                Some(Outcome::Done(Err(e))) => FailureReason::Errored(redact(&graph.secrets, e)),
                Some(Outcome::Panicked(p)) => FailureReason::Panicked(p),
                Some(Outcome::TimedOut { after, limit }) => FailureReason::TimedOut { after, limit },
            };
            return Err(ConnectError::Hook { hook: kind, key: site_key(&graph, site), reason });
        }
    }
    Ok(())
}

/// What owns a hook: a singleton binding, or a module through its own `on_*` calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookSite {
    Binding(BindingId),
    Module(ModuleId),
}

/// The order hooks run in at startup; shutdown runs its reverse. Bindings keep connect order,
/// and each module's own hooks follow the last of its singletons. A module with no singleton
/// follows the modules before it in collection order.
pub(crate) fn hook_plan(graph: &Graph, singletons: &[BindingId], modules: &[ModuleId]) -> Vec<HookSite> {
    let mut after_last: HashMap<ModuleId, usize> = HashMap::new();
    for (i, &id) in singletons.iter().enumerate() {
        after_last.insert(graph.binding(id).origin, i + 1);
    }
    let mut floor = 0;
    let mut placed: Vec<(usize, ModuleId)> = modules
        .iter()
        .map(|&m| {
            let at = after_last.get(&m).copied().unwrap_or(floor);
            floor = floor.max(at);
            (at, m)
        })
        .collect();
    // Stable, so modules placed at one position keep collection order.
    placed.sort_by_key(|&(at, _)| at);

    let mut plan = Vec::with_capacity(singletons.len() + placed.len());
    let mut next = placed.iter().peekable();
    for (i, &id) in singletons.iter().enumerate() {
        while let Some(&&(at, m)) = next.peek() {
            if at > i {
                break;
            }
            plan.push(HookSite::Module(m));
            next.next();
        }
        plan.push(HookSite::Binding(id));
    }
    plan.extend(next.map(|&(_, m)| HookSite::Module(m)));
    plan
}

pub(crate) fn site_hooks(graph: &Graph, site: HookSite) -> &[HookRecord] {
    match site {
        HookSite::Binding(id) => &graph.binding(id).record.hooks,
        HookSite::Module(id) => &graph.module(id).hooks,
    }
}

/// The key a hook failure names: the binding's, or the module's own type for a module hook.
pub(crate) fn site_key(graph: &Graph, site: HookSite) -> KeyName {
    match site {
        HookSite::Binding(id) => binding_key(graph.binding(id)),
        HookSite::Module(id) => {
            let identity = &graph.module(id).identity;
            let q = identity.qualifier().unwrap_or_else(Qualifier::none);
            Key::from_parts(identity.type_id(), identity.type_name(), q.id, q.name).name(BindingKind::Single)
        }
    }
}

/// Runs one hook under its bound and `cap`, reading with `Purpose::Lifecycle` in the module the
/// hook belongs to. `None` for a binding hook whose singleton was never built, which has no
/// instance to run on.
pub(crate) async fn run_hook(
    shared: &Arc<AppShared>,
    graph: &Graph,
    site: HookSite,
    hook: &HookRecord,
    signal: Option<&Signal>,
    cap: Option<Cap>,
) -> Option<Outcome<Result<(), BoxError>>> {
    let (module, instance) = match site {
        HookSite::Binding(id) => (graph.binding(id).origin, Some(shared.singletons.get(id)?)),
        HookSite::Module(id) => (id, None),
    };
    let timer = shared.config.timer.as_deref();
    let defaults = shared.config.defaults();
    let bound = resolve_bound(hook.bound, BoundKind::Hook, defaults.as_ref());
    let resolver = Resolver::new(shared, module, None, Purpose::Lifecycle);
    let cx = HookCx { resolver: &resolver, instance: instance.as_ref(), signal };
    Some(run(timer, bound, cap, &graph.secrets, (*hook.run)(cx)).await)
}

pub(crate) fn binding_key(binding: &FrozenBinding) -> KeyName {
    let key = binding.record.keys().next().unwrap_or(binding.record.primary);
    key.name(binding.record.kind)
}

fn module_name(graph: &Graph, id: ModuleId) -> ModuleName {
    graph.module(id).name.clone()
}

/// A constructor's failed site read during `connect`. A nested construction's failure keeps the
/// deeper key and its module; any other lookup error is the constructor's failure, since
/// `ConnectError` has no lookup variant to carry it unchanged.
fn site_failure(graph: &Graph, binding: &FrozenBinding, error: LookupError) -> ConnectError {
    match error {
        LookupError::Construct { key, reason } => {
            let module = owner_of(graph, binding.origin, key.key()).unwrap_or(binding.origin);
            ConnectError::Construct { key, module: module_name(graph, module), reason }
        }
        other => ConnectError::Construct {
            key: binding_key(binding),
            module: module_name(graph, binding.origin),
            reason: FailureReason::Errored(core_error(other)),
        },
    }
}

fn owner_of(graph: &Graph, from: ModuleId, key: Key) -> Option<ModuleId> {
    if let Some(Visible::Binding(id)) = graph.lookup(from, key) {
        return Some(graph.binding(*id).origin);
    }
    graph.bindings.iter().find(|b| b.record.keys().any(|k| k == key)).map(|b| b.origin)
}

/// A readiness attempt's error. Only the constructor's own error is redacted; a `LookupError`
/// is the core's, and any outside error inside it was redacted where it was stored.
fn construct_error(secrets: &SecretRegistry, error: ConstructError) -> Redacted {
    match error {
        ConstructError::Failed(e) => redact(secrets, e),
        ConstructError::Site(e) => core_error(e),
    }
}

fn core_error(error: LookupError) -> Redacted {
    let text = error.to_string();
    Redacted::from_parts(Box::new(error), text)
}
