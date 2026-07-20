use std::{pin::Pin, sync::Arc};

use crate::{
    http_helpers::{Body, HttpRequest, HttpResponse},
    middleware::MiddlewareChain,
};

/// Runtime context the framework hands to an adapter at serve time.
///
/// Passed to [`HttpAdapter::into_lifecycle`](crate::adapter::HttpAdapter::into_lifecycle)
/// after all `bind`/`bind_ws` calls.
///
/// New fields can be added here without changing the trait signature —
/// adapters ignore fields they don't need.
///
/// TODO: add graceful shutdown signal.
pub struct AdapterContext {
    /// Runs before the adapter's routing on every request — including
    /// unknown paths (404) and method mismatches (405).
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
    /// Call this once per incoming request, before route resolution:
    /// `routing` must be the adapter's entire match-and-dispatch step, not a
    /// single matched handler. The request the chain hands to `routing` is
    /// the one the router must match on — middleware may have rewritten it —
    /// and middleware that never calls `routing` has short-circuited the
    /// request (auth rejections, CORS preflight). Unhandled middleware
    /// errors produce a 500 response.
    pub async fn execute<F>(&self, req: HttpRequest, routing: F) -> HttpResponse
    where
        F: FnOnce(HttpRequest) -> Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>>
            + Send
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
