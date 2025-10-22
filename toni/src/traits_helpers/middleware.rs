use async_trait::async_trait;
use std::sync::Arc;

use crate::http_helpers::{HttpRequest, HttpResponse, IntoResponse};

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
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

impl MiddlewareConfiguration {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn should_apply(&self, path: &str) -> bool {
        // If no patterns specified, apply to all
        if self.include_patterns.is_empty() && self.exclude_patterns.is_empty() {
            return true;
        }

        // Check exclusions first
        for pattern in &self.exclude_patterns {
            if Self::matches_pattern(path, pattern) {
                return false;
            }
        }

        // If include patterns exist, path must match one
        if !self.include_patterns.is_empty() {
            return self
                .include_patterns
                .iter()
                .any(|pattern| Self::matches_pattern(path, pattern));
        }

        true
    }

    fn matches_pattern(path: &str, pattern: &str) -> bool {
        // Simple glob-like matching
        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            path.starts_with(prefix)
        } else {
            path == pattern
        }
    }
}
