use std::sync::Arc;

use crate::middleware::MiddlewareChain;

/// Runtime context the framework hands to an adapter at serve time.
///
/// Passed to [`HttpAdapter::create`] after all `bind`/`bind_ws` calls.
///
/// New fields can be added here without changing the trait signature —
/// adapters ignore fields they don't need.
///
/// TODO: add graceful shutdown signal.
pub struct AdapterContext {
    /// Runs before the adapter's routing on every request, including 404s.
    pub global_chain: Arc<MiddlewareChain>,
}

impl AdapterContext {
    pub fn new(global_chain: MiddlewareChain) -> Self {
        Self {
            global_chain: Arc::new(global_chain),
        }
    }
}
