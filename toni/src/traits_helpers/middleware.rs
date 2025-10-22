use async_trait::async_trait;
use std::sync::Arc;

use crate::http_helpers::{HttpRequest, HttpResponse, IntoResponse};
use crate::middleware::RoutePattern;

/// Result type for middleware chain execution
pub type MiddlewareResult = Result<
    Box<dyn IntoResponse<Response = HttpResponse> + Send>,
    Box<dyn std::error::Error + Send + Sync>,
>;

/// Next function in the middleware chain
#[async_trait]
pub trait Next: Send + Sync {
    async fn run(self: Box<Self>, req: HttpRequest) -> MiddlewareResult;
}

/// Core middleware trait
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Process the request and optionally call next
    async fn handle(&self, req: HttpRequest, next: Box<dyn Next>) -> MiddlewareResult;
}

/// Middleware consumer trait for applying middleware to routes
pub trait MiddlewareConsumer {
    /// Apply middleware to specific routes
    fn apply(&mut self, middleware: Arc<dyn Middleware>) -> &mut Self;

    /// Exclude specific routes from middleware
    fn exclude(&mut self, paths: Vec<String>) -> &mut Self;

    /// Apply to routes matching pattern
    fn for_routes(&mut self, patterns: Vec<String>) -> &mut Self;
}

/// Functional middleware - simpler alternative using closures
pub type MiddlewareFn = Arc<
    dyn Fn(
            HttpRequest,
            Box<dyn Next>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = MiddlewareResult> + Send>>
        + Send
        + Sync,
>;

/// Wrapper to convert functional middleware to trait
pub struct FunctionalMiddleware {
    handler: MiddlewareFn,
}

impl FunctionalMiddleware {
    pub fn new(handler: MiddlewareFn) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl Middleware for FunctionalMiddleware {
    async fn handle(&self, req: HttpRequest, next: Box<dyn Next>) -> MiddlewareResult {
        (self.handler)(req, next).await
    }
}

/// Middleware configuration for a module
#[derive(Default)]
pub struct MiddlewareConfiguration {
    pub middleware: Vec<Arc<dyn Middleware>>,
    pub include_patterns: Vec<RoutePattern>,
    pub exclude_patterns: Vec<RoutePattern>,
}

impl MiddlewareConfiguration {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this middleware should apply to the given path and HTTP method
    pub fn should_apply(&self, path: &str, method: &str) -> bool {
        // If no patterns specified, apply to all
        if self.include_patterns.is_empty() && self.exclude_patterns.is_empty() {
            return true;
        }

        // Check exclusions first - if excluded, don't apply
        for pattern in &self.exclude_patterns {
            if pattern.matches(path, method) {
                return false;
            }
        }

        // If include patterns exist, path must match one
        if !self.include_patterns.is_empty() {
            return self
                .include_patterns
                .iter()
                .any(|pattern| pattern.matches(path, method));
        }

        true
    }
}
