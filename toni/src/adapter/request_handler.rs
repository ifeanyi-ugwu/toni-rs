use std::{pin::Pin, sync::Arc};

use crate::{
    http_helpers::{Body, HttpRequest, HttpResponse},
    middleware::MiddlewareChain,
};

pub type BoxFuture<T> = Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// The single entry point the adapter calls for every incoming HTTP request.
///
/// The framework passes an `Arc<dyn RequestHandler>` to `HttpAdapter::create`.
/// What's behind that pointer is opaque to the adapter — at runtime it is a
/// `MiddlewareAdapter` that runs the global middleware chain and then delegates
/// to the adapter's own routing via a second `RequestHandler` the adapter
/// produced in `route_handler`.
pub trait RequestHandler: Send + Sync + 'static {
    fn handle(&self, req: HttpRequest) -> BoxFuture<HttpResponse>;
}

/// Framework-internal wrapper: runs global middleware, then calls the adapter's
/// routing handler.  Adapter authors never interact with this type.
pub(crate) struct MiddlewareAdapter {
    pub(crate) inner: Arc<dyn RequestHandler>,
    pub(crate) chain: Arc<MiddlewareChain>,
}

impl RequestHandler for MiddlewareAdapter {
    fn handle(&self, req: HttpRequest) -> BoxFuture<HttpResponse> {
        let inner = self.inner.clone();
        let chain = self.chain.clone();
        Box::pin(async move {
            chain
                .execute(req, move |req| inner.handle(req))
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
        })
    }
}
