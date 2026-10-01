//! `load`: a module wired against the frozen graph and connected through its own readiness
//! checks and init hooks, after startup (§8.6).
//!
//! What the frozen graph cannot take is refused with `LoadRefusal`, because the graph has
//! already been handed out: controllers and middleware (routes are bound), contributions to a
//! collection the module does not introduce (whatever reads it as `Many<T>` may already have),
//! global exports, and execution inputs (the per-handler input check ran at wiring time).

use std::sync::Arc;

use crate::app::shared::AppShared;
use crate::error::LoadError;
use crate::module::Module;
use crate::module::handle::ModuleRef;

/// Refused from Stopping on with `LoadError::Closed`: a module loaded then would miss the
/// before-shutdown stage already running. An identity already in the graph returns its
/// existing handle. Lazily loaded modules shut down in reverse load order.
pub(crate) async fn load(shared: &Arc<AppShared>, module: Box<dyn Module>) -> Result<ModuleRef, LoadError> {
    todo!()
}
