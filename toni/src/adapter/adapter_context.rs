use std::{pin::Pin, sync::Arc};

use crate::{
    http_helpers::{Body, HttpRequest, HttpResponse},
    middleware::MiddlewareChain,
};

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

    /// Run `routing` through the global middleware chain.
    ///
    /// Call this once per incoming request. `routing` is the adapter's own
    /// path-matching logic — it runs after all global middleware has executed.
    /// Unhandled middleware errors produce a 500 response.
    pub async fn execute<F>(&self, req: HttpRequest, routing: F) -> HttpResponse
    where
        F: Fn(HttpRequest) -> Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>>
            + Send
            + Sync
            + 'static,
    {
        self.global_chain
            .execute(req, routing)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "unhandled error in global middleware chain");
                HttpResponse {
                    status: 500,
                    headers: vec![],
                    body: Some(Body::json(serde_json::json!({
                        "statusCode": 500,
                        "message": "Internal Server Error",
                        "error": "Internal Server Error"
                    }))),
                }
            })
    }
}
