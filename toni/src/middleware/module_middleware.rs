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
    /// Key: module token, Value: middleware configuration for that module
    module_middleware: FxHashMap<String, MiddlewareConfiguration>,
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
    /// ```rust
    /// manager.add_global(Arc::new(LoggerMiddleware::new()));
    /// ```
    pub fn add_global(&mut self, middleware: Arc<dyn Middleware>) {
        self.global_middleware.push(middleware);
    }

    /// Add middleware configuration for a specific module
    ///
    /// # Example
    /// ```rust
    /// let config = MiddlewareConfigurer::new()
    ///     .apply(Arc::new(AuthMiddleware::new()))
    ///     .for_routes(vec!["/users/*"])
    ///     .build();
    ///
    /// manager.add_for_module("UserModule".to_string(), config);
    /// ```
    pub fn add_for_module(&mut self, module_token: String, config: MiddlewareConfiguration) {
        self.module_middleware.insert(module_token, config);
    }

    /// Get all middleware that should apply to a specific route
    ///
    /// This combines global middleware with module-specific middleware
    /// that matches the route pattern
    ///
    /// # Arguments
    /// * `module_token` - The token of the module the route belongs to
    /// * `route_path` - The path of the route (e.g., "/api/users")
    ///
    /// # Returns
    /// A vector of middleware that should be applied to this route
    pub fn get_middleware_for_route(
        &self,
        module_token: &str,
        route_path: &str,
    ) -> Vec<Arc<dyn Middleware>> {
        let mut middleware = Vec::new();

        // Add global middleware first (executes first in chain)
        middleware.extend(self.global_middleware.iter().cloned());

        // Add module-specific middleware if applicable
        if let Some(config) = self.module_middleware.get(module_token) {
            if config.should_apply(route_path) {
                middleware.extend(config.middleware.iter().cloned());
            }
        }

        middleware
    }

    /// Get reference to global middleware
    pub fn get_global_middleware(&self) -> &[Arc<dyn Middleware>] {
        &self.global_middleware
    }

    /// Get reference to module middleware map
    pub fn get_module_middleware(&self) -> &FxHashMap<String, MiddlewareConfiguration> {
        &self.module_middleware
    }
}

impl Default for MiddlewareManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for configuring middleware in a module
///
/// This provides a fluent API for configuring middleware with route patterns
///
/// # Example
/// ```rust
/// let config = MiddlewareConfigurer::new()
///     .apply(Arc::new(LoggerMiddleware::new()))
///     .apply(Arc::new(AuthMiddleware::new()))
///     .for_routes(vec!["/api/*"])
///     .exclude(vec!["/api/public/*"])
///     .build();
/// ```
pub struct MiddlewareConfigurer {
    config: MiddlewareConfiguration,
}

impl MiddlewareConfigurer {
    /// Create a new middleware configurer
    pub fn new() -> Self {
        Self {
            config: MiddlewareConfiguration::new(),
        }
    }

    /// Apply middleware to the configuration
    ///
    /// Multiple middleware can be added by calling this method multiple times
    ///
    /// # Example
    /// ```rust
    /// configurer
    ///     .apply(Arc::new(LoggerMiddleware::new()))
    ///     .apply(Arc::new(AuthMiddleware::new()));
    /// ```
    pub fn apply(mut self, middleware: Arc<dyn Middleware>) -> Self {
        self.config.middleware.push(middleware);
        self
    }

    /// Specify which routes this middleware should apply to
    ///
    /// Supports glob-like patterns:
    /// - `/users` - Exact match
    /// - `/api/*` - All routes starting with /api/
    ///
    /// # Example
    /// ```rust
    /// configurer.for_routes(vec!["/api/*", "/admin/*"]);
    /// ```
    pub fn for_routes(mut self, patterns: Vec<&str>) -> Self {
        self.config.include_patterns = patterns.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Specify which routes to exclude from this middleware
    ///
    /// # Example
    /// ```rust
    /// configurer
    ///     .for_routes(vec!["/api/*"])
    ///     .exclude(vec!["/api/public/*", "/api/health"]);
    /// ```
    pub fn exclude(mut self, patterns: Vec<&str>) -> Self {
        self.config.exclude_patterns = patterns.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Build the final middleware configuration
    pub fn build(self) -> MiddlewareConfiguration {
        self.config
    }
}

impl Default for MiddlewareConfigurer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http_helpers::{Body, HttpRequest},
        traits_helpers::middleware::{Middleware, MiddlewareResult, Next},
    };
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
        async fn handle(&self, req: HttpRequest, next: Box<dyn Next>) -> MiddlewareResult {
            println!("DummyMiddleware {} executed", self.name);
            next.run(req).await
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

        let middleware = manager.get_middleware_for_route("TestModule", "/api/test");
        assert_eq!(middleware.len(), 1);
    }

    #[test]
    fn test_middleware_configurer_with_includes() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/*"])
            .build();

        assert_eq!(config.middleware.len(), 1);
        assert_eq!(config.include_patterns.len(), 1);
        assert_eq!(config.include_patterns[0], "/api/*");
    }

    #[test]
    fn test_middleware_configurer_with_excludes() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .exclude(vec!["/api/public/*"])
            .build();

        assert_eq!(config.middleware.len(), 1);
        assert_eq!(config.exclude_patterns.len(), 1);
        assert_eq!(config.exclude_patterns[0], "/api/public/*");
    }

    #[test]
    fn test_module_middleware_with_includes() {
        let mut manager = MiddlewareManager::new();

        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("module")))
            .for_routes(vec!["/api/*"])
            .build();

        manager.add_for_module("TestModule".to_string(), config);

        // Should apply to /api/users
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users");
        assert_eq!(middleware.len(), 1);

        // Should not apply to /other/route
        let middleware = manager.get_middleware_for_route("TestModule", "/other/route");
        assert_eq!(middleware.len(), 0);
    }

    #[test]
    fn test_module_middleware_with_excludes() {
        let mut manager = MiddlewareManager::new();

        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("module")))
            .exclude(vec!["/api/public/*"])
            .build();

        manager.add_for_module("TestModule".to_string(), config);

        // Should apply to /api/users (not excluded)
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users");
        assert_eq!(middleware.len(), 1);

        // Should not apply to /api/public/data (excluded)
        let middleware = manager.get_middleware_for_route("TestModule", "/api/public/data");
        assert_eq!(middleware.len(), 0);
    }

    #[test]
    fn test_global_and_module_middleware_combined() {
        let mut manager = MiddlewareManager::new();

        // Add global middleware
        manager.add_global(Arc::new(DummyMiddleware::new("global1")));
        manager.add_global(Arc::new(DummyMiddleware::new("global2")));

        // Add module-specific middleware
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("module")))
            .for_routes(vec!["/api/*"])
            .build();

        manager.add_for_module("TestModule".to_string(), config);

        // Get middleware for matching route
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users");

        // Should have 3 middleware: 2 global + 1 module
        assert_eq!(middleware.len(), 3);
    }

    #[test]
    fn test_multiple_middleware_in_config() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("first")))
            .apply(Arc::new(DummyMiddleware::new("second")))
            .apply(Arc::new(DummyMiddleware::new("third")))
            .for_routes(vec!["/api/*"])
            .build();

        assert_eq!(config.middleware.len(), 3);
    }

    #[test]
    fn test_middleware_order() {
        let mut manager = MiddlewareManager::new();

        // Add in specific order
        manager.add_global(Arc::new(DummyMiddleware::new("first")));
        manager.add_global(Arc::new(DummyMiddleware::new("second")));

        let global = manager.get_global_middleware();
        assert_eq!(global.len(), 2);

        // Order should be preserved
        // (Note: We can't easily test the actual order without downcasting,
        // but the Vec preserves insertion order)
    }

    #[test]
    fn test_empty_patterns_applies_to_all() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .build();

        // With no patterns, should apply to any route
        assert!(config.should_apply("/any/route"));
        assert!(config.should_apply("/another/path"));
    }

    #[test]
    fn test_pattern_matching_exact() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/users"])
            .build();

        assert!(config.should_apply("/api/users"));
        assert!(!config.should_apply("/api/users/123"));
        assert!(!config.should_apply("/api/posts"));
    }

    #[test]
    fn test_pattern_matching_wildcard() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/*"])
            .build();

        assert!(config.should_apply("/api/users"));
        assert!(config.should_apply("/api/users/123"));
        assert!(config.should_apply("/api/posts"));
        assert!(!config.should_apply("/admin/users"));
    }

    #[test]
    fn test_exclusion_takes_precedence() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/*"])
            .exclude(vec!["/api/public/*"])
            .build();

        assert!(config.should_apply("/api/users"));
        assert!(!config.should_apply("/api/public/data"));
        assert!(!config.should_apply("/api/public/info"));
    }
}
