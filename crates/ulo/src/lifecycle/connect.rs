//! The connect phase (§9.2, §9.3): singletons in a stable topological order, each followed by
//! its readiness check, then every `OnModuleInit` hook in the same order with a module's own
//! hooks after its providers', then every `OnApplicationBootstrap` hook. The first failure
//! stops the walk and is returned as a `ConnectError`.

use std::sync::Arc;

use crate::app::shared::AppShared;
use crate::binding::ReadyRecord;
use crate::error::ConnectError;
use crate::graph::{BindingId, ModuleId};
use crate::hooks::HookKind;

/// Connects `singletons` (in connect order) and the hooks of `modules` (in collection order).
/// `App::connect` passes the whole graph; `load` passes what the lazy module brought.
pub(crate) async fn connect(shared: &Arc<AppShared>, singletons: &[BindingId], modules: &[ModuleId]) -> Result<(), ConnectError> {
    todo!()
}

/// Builds one singleton under its construction bound, panics caught, and stores it.
/// `Construct { key, module, reason }` on failure.
pub(crate) async fn build_singleton(shared: &Arc<AppShared>, id: BindingId) -> Result<(), ConnectError> {
    todo!()
}

/// Runs a readiness check: attempts under the attempt bound, `retries` more after an `Err` or an
/// attempt timeout with `backoff` between, the whole under the check bound. A panicking attempt
/// ends the check at once. `Readiness { key, attempts, reason }` on failure, the reason by the
/// rules of §9.3.
pub(crate) async fn run_readiness(shared: &Arc<AppShared>, id: BindingId, ready: &ReadyRecord) -> Result<(), ConnectError> {
    todo!()
}

/// Every hook of `kind` on `singletons` and `modules`, in connect order with each module's own
/// hooks after its providers'. `Hook { hook, key, reason }` on the first failure.
pub(crate) async fn run_startup_hooks(
    shared: &Arc<AppShared>,
    kind: HookKind,
    singletons: &[BindingId],
    modules: &[ModuleId],
) -> Result<(), ConnectError> {
    todo!()
}
