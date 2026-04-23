use std::sync::Arc;

use crate::http_helpers::{HttpRequest, HttpResponse};
use crate::middleware::MiddlewareChain;

use super::framework_router::FrameworkRouter;

/// Owns the full pre-routing pipeline.
///
/// Every HTTP request (matched or not) flows through:
/// ```text
/// global middleware → framework path match → route-scoped middleware → guards → interceptors → pipes → controller
///                                          ↘ 404 / 405 if no match
/// ```
///
/// This is the single object the HTTP adapter receives via
/// [`HttpAdapter::set_dispatcher`]. The adapter calls [`dispatch`] for every
/// incoming request, including those that would otherwise produce a 404.
///
/// [`HttpAdapter::set_dispatcher`]: crate::http_adapter::HttpAdapter::set_dispatcher
/// [`dispatch`]: RequestDispatcher::dispatch
pub struct RequestDispatcher {
    router: Arc<FrameworkRouter>,
    global_middleware: MiddlewareChain,
}

impl RequestDispatcher {
    pub fn new(router: FrameworkRouter, global_middleware: MiddlewareChain) -> Self {
        Self {
            router: Arc::new(router),
            global_middleware,
        }
    }

    pub async fn dispatch(&self, req: HttpRequest) -> HttpResponse {
        let router = self.router.clone();
        self.global_middleware
            .execute(req, move |req| {
                let router = router.clone();
                Box::pin(async move { router.dispatch(req).await })
            })
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, "unhandled error in global middleware chain");
                HttpResponse {
                    status: 500,
                    headers: vec![],
                    body: Some(crate::http_helpers::Body::json(serde_json::json!({
                        "statusCode": 500,
                        "message": "Internal Server Error",
                        "error": "Internal Server Error"
                    }))),
                }
            })
    }
}
