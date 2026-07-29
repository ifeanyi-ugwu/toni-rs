use std::{pin::Pin, sync::Arc};

use crate::{
    http_helpers::{Body, HttpRequest, HttpResponse, trim_trailing_slashes},
    middleware::MiddlewareChain,
};

/// Runtime context the framework hands to an adapter at serve time.
///
/// Passed to [`HttpAdapter::into_lifecycle`](crate::adapter::HttpAdapter::into_lifecycle)
/// after all `register_route`/`register_ws_route` calls.
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
    ///
    /// Trailing slashes are trimmed from the request path before the chain
    /// runs, so middleware and routing both see the canonical form — `/app/`
    /// matches a route registered at `/app`. Trimming before the chain (not
    /// between chain and routing) keeps path checks in middleware and route
    /// matching consistent: an auth middleware guarding `/admin` cannot be
    /// sidestepped by requesting `/admin/`.
    pub async fn execute<F>(&self, req: HttpRequest, routing: F) -> HttpResponse
    where
        F: FnOnce(HttpRequest) -> Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>>
            + Send
            + 'static,
    {
        self.global_chain
            .execute(normalize_request_path(req), routing)
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

/// Trim trailing slashes from the request path, preserving the root `/` and
/// the query string. Registered route paths never carry a trailing slash
/// ([`join_route`](crate::http_helpers::join_route) trims them), so this is
/// the request-side half of trailing-slash-insensitive matching.
///
/// A URI that fails to reparse after trimming (never a path-form URI in
/// practice) passes through untouched rather than erroring.
fn normalize_request_path(mut req: HttpRequest) -> HttpRequest {
    let uri = req.uri();
    let path = uri.path();
    if path.len() > 1 && path.ends_with('/') {
        let trimmed = trim_trailing_slashes(path);
        let path_and_query = match uri.query() {
            Some(q) => format!("{trimmed}?{q}"),
            None => trimmed.to_string(),
        };
        let mut parts = uri.clone().into_parts();
        if let Ok(pq) = path_and_query.parse() {
            parts.path_and_query = Some(pq);
            if let Ok(new_uri) = http::Uri::from_parts(parts) {
                *req.uri_mut() = new_uri;
            }
        }
    }
    req
}

#[cfg(test)]
mod tests {
    use super::normalize_request_path;
    use crate::http_helpers::{HttpRequest, RequestBody};

    fn request(uri: &str) -> HttpRequest {
        HttpRequest(
            http::Request::builder()
                .uri(uri)
                .body(RequestBody::Buffered(bytes::Bytes::new()))
                .unwrap(),
        )
    }

    #[test]
    fn trims_trailing_slash_and_keeps_query() {
        let req = normalize_request_path(request("/app/"));
        assert_eq!(req.uri().path(), "/app");

        let req = normalize_request_path(request("/app/?page=2"));
        assert_eq!(req.uri().path(), "/app");
        assert_eq!(req.uri().query(), Some("page=2"));

        let req = normalize_request_path(request("/app/user/5///"));
        assert_eq!(req.uri().path(), "/app/user/5");
    }

    #[test]
    fn root_and_slashless_paths_pass_through() {
        let req = normalize_request_path(request("/"));
        assert_eq!(req.uri().path(), "/");

        let req = normalize_request_path(request("/app/user/5"));
        assert_eq!(req.uri().path(), "/app/user/5");
    }
}
