use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::{
    middleware::route_pattern::IntoRoutePattern,
    traits_helpers::middleware::{Middleware, MiddlewareConfiguration},
};

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
    /// that matches the route pattern and HTTP method
    ///
    /// # Arguments
    /// * `module_token` - The token of the module the route belongs to
    /// * `route_path` - The path of the route (e.g., "/api/users")
    /// * `method` - The HTTP method (e.g., "GET", "POST")
    ///
    /// # Returns
    /// A vector of middleware that should be applied to this route
    pub fn get_middleware_for_route(
        &self,
        module_token: &str,
        route_path: &str,
        method: &str,
    ) -> Vec<Arc<dyn Middleware>> {
        let mut middleware = Vec::new();

        // Add global middleware first (executes first in chain)
        middleware.extend(self.global_middleware.iter().cloned());

        // Add module-specific middleware if applicable
        if let Some(config) = self.module_middleware.get(module_token) {
            if config.should_apply(route_path, method) {
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
    /// Supports glob-like patterns and HTTP method filtering:
    /// - `/users` - Exact match, all HTTP methods
    /// - `/api/*` - All routes starting with /api/, all HTTP methods
    /// - `("/users", "POST")` - Only POST requests to /users
    /// - `("/api/*", ["GET", "POST"])` - Only GET and POST to /api/*
    ///
    /// # Example
    /// ```rust
    /// // String patterns (all HTTP methods)
    /// configurer.for_routes(vec!["/api/*", "/admin/*"]);
    ///
    /// // With HTTP method filtering
    /// configurer.for_routes(vec![
    ///     ("/api/users", "POST"),
    ///     ("/api/products/*", ["GET", "PUT"]),
    /// ]);
    /// ```
    pub fn for_routes<T: IntoRoutePattern>(mut self, patterns: Vec<T>) -> Self {
        self.config.include_patterns = patterns
            .into_iter()
            .map(|p| p.into_route_pattern())
            .collect();
        self
    }

    /// Add a single route pattern
    ///
    /// # Example
    /// ```rust
    /// configurer
    ///     .for_route("/api/*")
    ///     .for_route(("/users", "POST"));
    /// ```
    pub fn for_route<T>(mut self, route: T) -> Self
    where
        T: IntoRoutePattern,
    {
        self.config
            .include_patterns
            .push(route.into_route_pattern());
        self
    }

    /// Specify which routes to exclude from this middleware
    ///
    /// Supports glob-like patterns and HTTP method filtering:
    /// - `/api/public/*` - Exclude all public API routes
    /// - `("/users", "DELETE")` - Exclude only DELETE requests to /users
    ///
    /// # Example
    /// ```rust
    /// configurer
    ///     .for_routes(vec!["/api/*"])
    ///     .exclude(vec![
    ///         "/api/public/*",
    ///         ("/api/users", "DELETE"),
    ///     ]);
    /// ```
    pub fn exclude<T: IntoRoutePattern>(mut self, patterns: Vec<T>) -> Self {
        self.config.exclude_patterns = patterns
            .into_iter()
            .map(|p| p.into_route_pattern())
            .collect();
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

        let middleware = manager.get_middleware_for_route("TestModule", "/api/test", "GET");
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
        assert_eq!(config.include_patterns[0].path, "/api/*");
        assert!(config.include_patterns[0].methods.is_none());
    }

    #[test]
    fn test_middleware_configurer_with_excludes() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .exclude(vec!["/api/public/*"])
            .build();

        assert_eq!(config.middleware.len(), 1);
        assert_eq!(config.exclude_patterns.len(), 1);
        assert_eq!(config.exclude_patterns[0].path, "/api/public/*");
        assert!(config.exclude_patterns[0].methods.is_none());
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
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users", "GET");
        assert_eq!(middleware.len(), 1);

        // Should not apply to /other/route
        let middleware = manager.get_middleware_for_route("TestModule", "/other/route", "GET");
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
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users", "GET");
        assert_eq!(middleware.len(), 1);

        // Should not apply to /api/public/data (excluded)
        let middleware = manager.get_middleware_for_route("TestModule", "/api/public/data", "GET");
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
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users", "GET");

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

        // With no patterns, should apply to any route and method
        assert!(config.should_apply("/any/route", "GET"));
        assert!(config.should_apply("/another/path", "POST"));
    }

    #[test]
    fn test_pattern_matching_exact() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/users"])
            .build();

        assert!(config.should_apply("/api/users", "GET"));
        assert!(!config.should_apply("/api/users/123", "GET"));
        assert!(!config.should_apply("/api/posts", "GET"));
    }

    #[test]
    fn test_pattern_matching_wildcard() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/*"])
            .build();

        assert!(config.should_apply("/api/users", "GET"));
        assert!(config.should_apply("/api/users/123", "POST"));
        assert!(config.should_apply("/api/posts", "PUT"));
        assert!(!config.should_apply("/admin/users", "GET"));
    }

    #[test]
    fn test_exclusion_takes_precedence() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/*"])
            .exclude(vec!["/api/public/*"])
            .build();

        assert!(config.should_apply("/api/users", "GET"));
        assert!(!config.should_apply("/api/public/data", "GET"));
        assert!(!config.should_apply("/api/public/info", "POST"));
    }

    // ===== HTTP Method Filtering Tests =====

    #[test]
    fn test_method_filtering_single_method() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec![("/api/users", "POST")])
            .build();

        // Should apply to POST
        assert!(config.should_apply("/api/users", "POST"));

        // Should NOT apply to other methods
        assert!(!config.should_apply("/api/users", "GET"));
        assert!(!config.should_apply("/api/users", "PUT"));
        assert!(!config.should_apply("/api/users", "DELETE"));
    }

    #[test]
    fn test_method_filtering_multiple_methods() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec![("/api/users", ["GET", "POST"])])
            .build();

        // Should apply to GET and POST
        assert!(config.should_apply("/api/users", "GET"));
        assert!(config.should_apply("/api/users", "POST"));

        // Should NOT apply to other methods
        assert!(!config.should_apply("/api/users", "PUT"));
        assert!(!config.should_apply("/api/users", "DELETE"));
        assert!(!config.should_apply("/api/users", "PATCH"));
    }

    #[test]
    fn test_method_filtering_with_wildcard() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec![("/api/*", "POST")])
            .build();

        // Should apply to POST on any /api/* route
        assert!(config.should_apply("/api/users", "POST"));
        assert!(config.should_apply("/api/products", "POST"));
        assert!(config.should_apply("/api/orders/123", "POST"));

        // Should NOT apply to other methods
        assert!(!config.should_apply("/api/users", "GET"));
        assert!(!config.should_apply("/api/products", "PUT"));
    }

    #[test]
    fn test_mixed_patterns_with_and_without_methods() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_route("/api/public/*") // All methods
            .for_route(("/api/users", "POST")) // Only POST
            .build();

        // /api/public/* should match all methods
        assert!(config.should_apply("/api/public/data", "GET"));
        assert!(config.should_apply("/api/public/info", "POST"));
        assert!(config.should_apply("/api/public/status", "DELETE"));

        // /api/users should only match POST
        assert!(config.should_apply("/api/users", "POST"));
        assert!(!config.should_apply("/api/users", "GET"));
    }

    #[test]
    fn test_exclude_with_method_filtering() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec!["/api/*"])
            .exclude(vec![("/api/users", "DELETE")])
            .build();

        // Should apply to all methods on /api/products
        assert!(config.should_apply("/api/products", "GET"));
        assert!(config.should_apply("/api/products", "POST"));
        assert!(config.should_apply("/api/products", "DELETE"));

        // Should apply to non-DELETE methods on /api/users
        assert!(config.should_apply("/api/users", "GET"));
        assert!(config.should_apply("/api/users", "POST"));
        assert!(config.should_apply("/api/users", "PUT"));

        // Should NOT apply to DELETE on /api/users
        assert!(!config.should_apply("/api/users", "DELETE"));
    }

    #[test]
    fn test_case_insensitive_method_matching() {
        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("test")))
            .for_routes(vec![("/api/users", "POST")])
            .build();

        // Should match regardless of case
        assert!(config.should_apply("/api/users", "POST"));
        assert!(config.should_apply("/api/users", "post"));
        assert!(config.should_apply("/api/users", "Post"));
        assert!(config.should_apply("/api/users", "pOsT"));
    }

    #[test]
    fn test_method_filtering_with_manager() {
        let mut manager = MiddlewareManager::new();

        let config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("post-only")))
            .for_routes(vec![("/api/users", "POST")])
            .build();

        manager.add_for_module("TestModule".to_string(), config);

        // Should return middleware for POST
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users", "POST");
        assert_eq!(middleware.len(), 1);

        // Should NOT return middleware for GET
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users", "GET");
        assert_eq!(middleware.len(), 0);
    }

    #[test]
    fn test_complex_method_filtering_scenario() {
        let mut manager = MiddlewareManager::new();

        // Auth middleware: all methods on /api/*, except GET on /api/public/*
        let auth_config = MiddlewareConfigurer::new()
            .apply(Arc::new(DummyMiddleware::new("auth")))
            .for_routes(vec!["/api/*"])
            .exclude(vec![("/api/public/*", "GET")])
            .build();

        manager.add_for_module("TestModule".to_string(), auth_config);

        // Auth required for POST to /api/public/data
        let middleware = manager.get_middleware_for_route("TestModule", "/api/public/data", "POST");
        assert_eq!(middleware.len(), 1);

        // Auth NOT required for GET to /api/public/data
        let middleware = manager.get_middleware_for_route("TestModule", "/api/public/data", "GET");
        assert_eq!(middleware.len(), 0);

        // Auth required for GET to /api/users (not in public)
        let middleware = manager.get_middleware_for_route("TestModule", "/api/users", "GET");
        assert_eq!(middleware.len(), 1);
    }
}
