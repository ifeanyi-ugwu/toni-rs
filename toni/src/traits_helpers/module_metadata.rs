use super::{Controller, Provider};
use crate::middleware::{IntoRoutePattern, RoutePattern};
use crate::traits_helpers::middleware::{Middleware, MiddlewareConfiguration};
use std::sync::Arc;

pub trait ModuleMetadata {
    fn get_id(&self) -> String;
    fn get_name(&self) -> String;
    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>>;
    fn controllers(&self) -> Option<Vec<Box<dyn Controller>>>;
    fn providers(&self) -> Option<Vec<Box<dyn Provider>>>;
    fn exports(&self) -> Option<Vec<String>>;

    /// Returns true if this module is global (exports available everywhere)
    fn is_global(&self) -> bool {
        false // Default: non-global
    }

    /// Configure middleware for this module
    fn configure_middleware(&self, _consumer: &mut MiddlewareConsumer) {
        // Default: do nothing
    }

    /// Mark this module as global, making its exports available everywhere
    fn global(self) -> GlobalModuleWrapper<Self>
    where
        Self: Sized,
    {
        GlobalModuleWrapper { inner: self }
    }
}

/// Wrapper that makes any module global by overriding is_global()
pub struct GlobalModuleWrapper<T: ModuleMetadata> {
    inner: T,
}

impl<T: ModuleMetadata> ModuleMetadata for GlobalModuleWrapper<T> {
    fn get_id(&self) -> String {
        self.inner.get_id()
    }

    fn get_name(&self) -> String {
        self.inner.get_name()
    }

    fn is_global(&self) -> bool {
        true // Always return true for global wrapper
    }

    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
        self.inner.imports()
    }

    fn controllers(&self) -> Option<Vec<Box<dyn Controller>>> {
        self.inner.controllers()
    }

    fn providers(&self) -> Option<Vec<Box<dyn Provider>>> {
        self.inner.providers()
    }

    fn exports(&self) -> Option<Vec<String>> {
        self.inner.exports()
    }

    fn configure_middleware(&self, consumer: &mut MiddlewareConsumer) {
        self.inner.configure_middleware(consumer)
    }
}

/// Builder for configuring middleware in modules
///
/// This provides a fluent API for configuring middleware with route patterns.
/// Used within the `configure_middleware` method of your modules.
///
/// # Example
/// ```ignore
/// #[module(controllers: [UserController])]
/// impl UserModule {
///     fn configure_middleware(&self, consumer: &mut MiddlewareConsumer) {
///         // Apply logger to all routes
///         consumer
///             .apply(LoggerMiddleware::new())
///             .for_routes(vec!["/users/*"]);
///
///         // Apply auth to specific routes, excluding public endpoints
///         consumer
///             .apply(AuthMiddleware::new())
///             .for_routes(vec!["/users/*"])
///             .exclude(vec!["/users/public/*"]);
///
///         // Multiple middleware can be applied to the same routes
///         consumer
///             .apply(RateLimitMiddleware::new(100, 60000))
///             .for_routes(vec![("/users/create", "POST")]);
///     }
/// }
/// ```
///
/// # Route Patterns
///
/// Supports glob-like patterns and HTTP method filtering:
/// - `/users` - Exact match, all HTTP methods
/// - `/api/*` - All routes starting with /api/, all HTTP methods
/// - `("/users", "POST")` - Only POST requests to /users
/// - `("/api/*", ["GET", "POST"])` - Only GET and POST to /api/*
pub struct MiddlewareConsumer {
    configurations: Vec<MiddlewareConfiguration>,
    current_middleware: Vec<Arc<dyn Middleware>>,
    current_includes: Vec<RoutePattern>,
    current_excludes: Vec<RoutePattern>,
}

impl MiddlewareConsumer {
    pub fn new() -> Self {
        Self {
            configurations: Vec::new(),
            current_middleware: Vec::new(),
            current_includes: Vec::new(),
            current_excludes: Vec::new(),
        }
    }

    /// Apply middleware to routes
    ///
    /// Returns a proxy that requires you to specify routes via `.for_routes()` or `.for_route()`.
    ///
    /// # Example
    /// ```ignore
    /// // Single middleware
    /// consumer
    ///     .apply(LoggerMiddleware::new())
    ///     .for_routes(vec!["/api/*"]);
    ///
    /// // Multiple middleware on same routes
    /// consumer
    ///     .apply(LoggerMiddleware::new())
    ///     .apply_also(AuthMiddleware::new())
    ///     .for_routes(vec!["/api/*"]);
    /// ```
    pub fn apply<M>(&mut self, middleware: M) -> MiddlewareConfigProxy<'_>
    where
        M: Middleware + 'static,
    {
        self.current_middleware.push(Arc::new(middleware));
        MiddlewareConfigProxy { consumer: self }
    }

    /// Finalize current middleware configuration
    fn finalize_current(&mut self) {
        if !self.current_middleware.is_empty() {
            let config = MiddlewareConfiguration {
                middleware: std::mem::take(&mut self.current_middleware),
                include_patterns: std::mem::take(&mut self.current_includes),
                exclude_patterns: std::mem::take(&mut self.current_excludes),
            };
            self.configurations.push(config);
        }
    }

    /// Get all configurations
    pub fn build(mut self) -> Vec<MiddlewareConfiguration> {
        self.finalize_current();
        self.configurations
    }
}

impl Default for MiddlewareConsumer {
    fn default() -> Self {
        Self::new()
    }
}

/// Proxy type returned by `.apply()` that enforces route specification
///
/// This type-state pattern ensures you cannot forget to call `.for_routes()` or `.for_route()`
/// after applying middleware.
///
/// # Methods
/// - `.apply_also()` - Add another middleware to the same configuration
/// - `.for_routes()` / `.for_route()` - Specify routes (required, returns consumer)
/// - `.exclude()` / `.exclude_route()` - Exclude routes (optional, returns proxy)
#[must_use = "Middleware proxy must call .for_routes() or .for_route() to complete configuration"]
pub struct MiddlewareConfigProxy<'a> {
    consumer: &'a mut MiddlewareConsumer,
}

impl<'a> MiddlewareConfigProxy<'a> {
    /// Add another middleware to the same configuration
    ///
    /// This allows you to group multiple middleware that should apply to the same routes.
    ///
    /// # Example
    /// ```ignore
    /// consumer
    ///     .apply(LoggerMiddleware::new())
    ///     .apply_also(AuthMiddleware::new())
    ///     .apply_also(CorsMiddleware::new())
    ///     .for_routes(vec!["/api/*"]);
    /// ```
    pub fn apply_also<M>(self, middleware: M) -> Self
    where
        M: Middleware + 'static,
    {
        self.consumer.current_middleware.push(Arc::new(middleware));
        self
    }

    /// Specify a single route to apply middleware to
    ///
    /// Finalizes the middleware configuration and returns the consumer,
    /// allowing you to chain another `.apply()` call.
    ///
    /// # Example
    /// ```ignore
    /// consumer
    ///     .apply(LoggerMiddleware::new())
    ///     .for_route("/api/*")              // Returns consumer
    ///     .apply(AuthMiddleware::new())      // Can chain another config
    ///     .for_route("/admin/*");
    /// ```
    pub fn for_route<T: IntoRoutePattern>(self, pattern: T) -> &'a mut MiddlewareConsumer {
        self.consumer
            .current_includes
            .push(pattern.into_route_pattern());

        // Finalize the configuration now that routes are specified
        self.consumer.finalize_current();
        self.consumer
    }

    /// Specify multiple routes to apply middleware to
    ///
    /// Finalizes the middleware configuration and returns the consumer,
    /// allowing you to chain another `.apply()` call.
    ///
    /// # Example
    /// ```ignore
    /// consumer
    ///     .apply(LoggerMiddleware::new())
    ///     .for_routes(vec!["/api/*", "/admin/*"]);
    /// ```
    pub fn for_routes<T: IntoRoutePattern>(self, patterns: Vec<T>) -> &'a mut MiddlewareConsumer {
        let mut new_patterns: Vec<RoutePattern> = patterns
            .into_iter()
            .map(|p| p.into_route_pattern())
            .collect();

        self.consumer.current_includes.append(&mut new_patterns);

        // Finalize the configuration now that routes are specified
        self.consumer.finalize_current();
        self.consumer
    }

    /// Exclude a single route from middleware
    ///
    /// Returns the proxy, so you can continue chaining exclusions or call `.for_routes()`.
    ///
    /// # Example
    /// ```ignore
    /// consumer
    ///     .apply(AuthMiddleware::new())
    ///     .exclude_route("/api/public/*")
    ///     .exclude_route("/api/health")
    ///     .for_routes(vec!["/api/*"]);
    /// ```
    pub fn exclude_route<T: IntoRoutePattern>(self, pattern: T) -> Self {
        self.consumer
            .current_excludes
            .push(pattern.into_route_pattern());
        self
    }

    /// Exclude multiple routes from middleware
    ///
    /// Returns the proxy, so you can continue chaining exclusions or call `.for_routes()`.
    ///
    /// # Example
    /// ```ignore
    /// consumer
    ///     .apply(AuthMiddleware::new())
    ///     .exclude(vec!["/api/public/*", "/api/health"])
    ///     .for_routes(vec!["/api/*"]);
    /// ```
    pub fn exclude<T: IntoRoutePattern>(self, patterns: Vec<T>) -> Self {
        let mut new_patterns: Vec<RoutePattern> = patterns
            .into_iter()
            .map(|p| p.into_route_pattern())
            .collect();

        self.consumer.current_excludes.append(&mut new_patterns);
        self
    }
}
