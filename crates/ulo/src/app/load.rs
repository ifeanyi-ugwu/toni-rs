//! `load`: a module wired against the frozen graph and connected through its own readiness
//! checks and init hooks, after startup (§8.6).
//!
//! What the frozen graph cannot take is refused with `LoadRefusal`, because the graph has
//! already been handed out: controllers and middleware (routes are bound), contributions to a
//! collection the module does not introduce (whatever reads it as `Many<T>` may already have),
//! global exports, and execution inputs (the per-handler input check ran at wiring time).

use std::sync::{Arc, PoisonError};

use crate::app::shared::AppShared;
use crate::error::{Closed, LoadError};
use crate::graph::ModuleId;
use crate::graph::wire::{self, LazyFailure, LazyWiring, WireEnv};
use crate::lifecycle::connect;
use crate::module::Module;
use crate::module::handle::ModuleRef;

/// Refused from Stopping on with `LoadError::Closed`: a module loaded then would miss the
/// before-shutdown stage already running. An identity already in the graph returns its
/// existing handle. Lazily loaded modules shut down in reverse load order.
///
/// A load that fails to connect leaves the app as it found it: the graph it extended is put
/// back and the singletons it built are dropped, so a later load of the same module wires
/// again from the start.
pub(crate) async fn load(shared: &Arc<AppShared>, module: Box<dyn Module>) -> Result<ModuleRef, LoadError> {
    if !shared.phase.allows_load() {
        return Err(LoadError::Closed(Closed::new()));
    }
    let _serial = shared.load_lock.lock().await;
    // Checked again: a shutdown may have begun while this load waited on another.
    if !shared.phase.allows_load() {
        return Err(LoadError::Closed(Closed::new()));
    }

    let base = shared.graph();
    if let Some(&existing) = base.by_identity.get(&module.identity()) {
        return Ok(shared.module_ref(existing));
    }

    let env = WireEnv {
        timer: shared.config.timer.clone(),
        runtime: shared.config.runtime.clone(),
        knobs_set: shared.config.knobs_set(),
    };
    let LazyWiring { graph, module: loaded, singletons } = match wire::wire_lazy(&base, module, &env) {
        Ok(wiring) => wiring,
        Err(LazyFailure::Wiring(errors)) => return Err(LoadError::Wiring(errors)),
        Err(LazyFailure::Refused(refusal)) => return Err(LoadError::Refused(refusal)),
    };
    // Lazily loaded modules take the ids after the base graph's, in the collection order of
    // what this load brought.
    let modules: Vec<ModuleId> = (base.modules.len()..graph.modules.len()).map(|i| ModuleId(i as u32)).collect();

    // Published before the connect walk: its constructors resolve through the app's graph. No
    // module of the base graph sees the new bindings, since a lazy module exports nothing
    // globally and contributes to no collection it does not introduce.
    *shared.graph.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(graph);

    if let Err(failure) = connect::connect(shared, &singletons, &modules).await {
        *shared.graph.write().unwrap_or_else(PoisonError::into_inner) = base;
        for id in &singletons {
            shared.singletons.remove(*id);
        }
        return Err(LoadError::Connect(failure));
    }
    Ok(shared.module_ref(loaded))
}
