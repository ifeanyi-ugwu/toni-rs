use anyhow::{Result, anyhow};
use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::traits_helpers::middleware::{Middleware, MiddlewareConfiguration};

/// Middleware manager for organizing middleware by module
///
/// This manages both global middleware (applies to all routes) and
/// module-specific middleware (scoped to certain routes)
pub struct MiddlewareManager {
    /// Global middleware that applies to all routes
    global_middleware: Vec<Arc<dyn Middleware>>,

    /// Module-specific middleware configurations
    /// Key: module token, Value: list of middleware configurations for that module
    module_middleware: FxHashMap<String, Vec<MiddlewareConfiguration>>,
}

impl MiddlewareManager {
    /// Create a new middleware manager
    pub fn new() -> Self {
        Self {
            global_middleware: Vec::new(),
            module_middleware: FxHashMap::default(),
        }
    }

    /// Add global middleware that applies to all routes
    ///
    /// # Example
    /// ```ignore
    /// manager.add_global(Arc::new(MyLoggerMiddleware::new()));
    /// ```
    pub fn add_global(&mut self, middleware: Arc<dyn Middleware>) {
        self.global_middleware.push(middleware);
    }

    /// Add middleware configuration for a specific module
    ///
    /// This is called internally by the framework when modules configure their middleware.
    /// Users typically don't call this directly - instead use `configure_middleware` in your module.
    pub fn add_for_module(&mut self, module_token: String, config: MiddlewareConfiguration) {
        self.module_middleware
            .entry(module_token)
            .or_insert_with(Vec::new)
            .push(config);
    }

    /// Get route-scoped middleware for a specific route.
    ///
    /// Returns only module-level middleware whose patterns match this route.
    /// Global middleware is intentionally excluded — it is applied at the
    /// dispatcher level (pre-routing) so it runs on all requests, including
    /// those that match no route.
    pub fn get_middleware_for_route(
        &self,
        module_token: &str,
        route_path: &str,
        method: &str,
    ) -> Vec<Arc<dyn Middleware>> {
        let mut middleware = Vec::new();

        if let Some(configs) = self.module_middleware.get(module_token) {
            for config in configs {
                if config.should_apply(route_path, method) {
                    middleware.extend(config.middleware.iter().cloned());
                }
            }
        }

        middleware
    }

    /// Get reference to global middleware
    pub fn get_global_middleware(&self) -> &[Arc<dyn Middleware>] {
        &self.global_middleware
    }

    /// Get reference to module middleware map
    pub fn get_module_middleware(&self) -> &FxHashMap<String, Vec<MiddlewareConfiguration>> {
        &self.module_middleware
    }

    /// Resolve middleware tokens against the role registry.
    ///
    /// Called after the DI container is fully built so the registry is populated.
    pub fn resolve_middleware_tokens(
        &mut self,
        module_token: &str,
        middleware_registry: &FxHashMap<String, Arc<dyn Middleware>>,
    ) -> Result<()> {
        if let Some(configs) = self.module_middleware.get_mut(module_token) {
            for config in configs {
                for token in &config.middleware_tokens {
                    let middleware =
                        middleware_registry.get(token).cloned().ok_or_else(|| {
                            anyhow!(
                                "Middleware '{}' not found in role registry for module '{}'. \
                                 Ensure the provider implements the Middleware trait and is registered in the module's providers.",
                                token,
                                module_token
                            )
                        })?;
                    config.middleware.push(middleware);
                }
                config.middleware_tokens.clear();
            }
        }
        Ok(())
    }
}

impl Default for MiddlewareManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits_helpers::middleware::{Middleware, MiddlewareResult, NextHandle};
    use async_trait::async_trait;

    // Dummy middleware for testing
    struct DummyMiddleware {
        name: String,
    }

    impl DummyMiddleware {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl Middleware for DummyMiddleware {
        async fn handle(&self, next: NextHandle) -> MiddlewareResult {
            println!("DummyMiddleware {} executed", self.name);
            next.run().await
        }
    }

    #[test]
    fn test_middleware_manager_creation() {
        let manager = MiddlewareManager::new();
        assert_eq!(manager.get_global_middleware().len(), 0);
    }

    #[test]
    fn test_add_global_middleware() {
        let mut manager = MiddlewareManager::new();
        manager.add_global(Arc::new(DummyMiddleware::new("global")));

        assert_eq!(manager.get_global_middleware().len(), 1);
    }

    #[test]
    fn test_get_middleware_for_route_with_global_only() {
        let mut manager = MiddlewareManager::new();
        manager.add_global(Arc::new(DummyMiddleware::new("global")));

        // Global middleware is excluded from get_middleware_for_route — it runs
        // pre-routing via AdapterContext::execute, not per-route.
        let middleware = manager.get_middleware_for_route("TestModule", "/api/test", "GET");
        assert_eq!(middleware.len(), 0);
    }
}
