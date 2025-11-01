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
    fn configure_middleware(&self) -> Option<Vec<MiddlewareConfiguration>> {
        None
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

    fn configure_middleware(&self) -> Option<Vec<MiddlewareConfiguration>> {
        self.inner.configure_middleware()
    }
}

/// Trait that modules can implement to configure middleware
pub trait ConfigureMiddleware {
    fn configure(consumer: &mut MiddlewareConsumer);
}

/// Builder for configuring middleware in modules
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
    pub fn apply(&mut self, middleware: Arc<dyn Middleware>) -> &mut Self {
        self.current_middleware.push(middleware);
        self
    }

    /// Specify routes to apply middleware to
    /// Accepts strings like "/users/*" or tuples like ("/users/*", "POST")
    pub fn for_routes<T: IntoRoutePattern>(&mut self, patterns: Vec<T>) -> &mut Self {
        self.current_includes = patterns
            .into_iter()
            .map(|p| p.into_route_pattern())
            .collect();
        self.finalize_current();
        self
    }

    /// Exclude specific routes from middleware
    /// Accepts strings like "/users/*" or tuples like ("/users/*", "POST")
    pub fn exclude<T: IntoRoutePattern>(&mut self, patterns: Vec<T>) -> &mut Self {
        self.current_excludes = patterns
            .into_iter()
            .map(|p| p.into_route_pattern())
            .collect();
        self.finalize_current();
        self
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
