use std::collections::HashMap;
use std::sync::Arc;

use crate::http_helpers::{Body, HttpMethod, HttpRequest, HttpResponse, PathParams};

use super::request_handler::RequestHandler;

fn to_matchit_path(path: &str) -> String {
    if !path.contains(':') {
        return path.to_owned();
    }
    let mut out = String::with_capacity(path.len() + 4);
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek().map_or(false, |&n| n != '/') {
            out.push('{');
            for n in chars.by_ref() {
                if n == '/' {
                    out.push('}');
                    out.push('/');
                    break;
                }
                out.push(n);
            }
            if !out.ends_with('}') {
                out.push('}');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Collects route registrations before the router is sealed.
///
/// matchit requires each path to be inserted exactly once, so routes are
/// grouped by path here before `build` is called.
pub struct RouteTableBuilder {
    routes: HashMap<String, HashMap<String, Arc<dyn RequestHandler>>>,
}

impl RouteTableBuilder {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    pub fn insert(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) {
        self.routes
            .entry(path.to_owned())
            .or_default()
            .insert(method.as_str().to_uppercase(), handler);
    }

    pub fn build(self) -> RouteTable {
        let mut inner = matchit::Router::new();
        for (path, method_map) in self.routes {
            let matchit_path = to_matchit_path(&path);
            inner.insert(matchit_path, method_map).unwrap_or_else(|e| {
                tracing::error!(path, error = %e, "failed to register route");
            });
        }
        RouteTable { inner }
    }
}

impl Default for RouteTableBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Immutable path router returned by `RouteTableBuilder::build`.
///
/// Adapters that want matchit-based routing can use this directly; adapters
/// with their own router don't need it at all.
pub struct RouteTable {
    inner: matchit::Router<HashMap<String, Arc<dyn RequestHandler>>>,
}

// Safety: all values inside are Send + Sync (Arc<dyn RequestHandler: Send+Sync>).
// matchit::Router<T> doesn't auto-derive Send+Sync even when T is Send+Sync.
unsafe impl Send for RouteTable {}
unsafe impl Sync for RouteTable {}

impl RouteTable {
    pub async fn dispatch(&self, req: HttpRequest) -> HttpResponse {
        let (mut parts, body) = req.into_parts();
        let path = parts.uri.path().to_owned();
        let method = parts.method.as_str().to_uppercase();

        match self.inner.at(&path) {
            Ok(matched) => {
                let path_params: HashMap<String, String> = matched
                    .params
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                parts.extensions.insert(PathParams(path_params));

                let req = HttpRequest::from_parts(parts, body);

                if let Some(handler) = matched.value.get(&method) {
                    handler.handle(req).await
                } else {
                    let allowed: Vec<&str> =
                        matched.value.keys().map(String::as_str).collect();
                    method_not_allowed(&method, &path, &allowed)
                }
            }
            Err(_) => not_found(&method, &path),
        }
    }
}

fn not_found(method: &str, path: &str) -> HttpResponse {
    HttpResponse {
        status: 404,
        headers: vec![],
        body: Some(Body::json(serde_json::json!({
            "statusCode": 404,
            "message": format!("Cannot {} {}", method, path),
            "error": "Not Found"
        }))),
    }
}

fn method_not_allowed(method: &str, path: &str, _allowed: &[&str]) -> HttpResponse {
    HttpResponse {
        status: 405,
        headers: vec![],
        body: Some(Body::json(serde_json::json!({
            "statusCode": 405,
            "message": format!("Cannot {} {}", method, path),
            "error": "Method Not Allowed"
        }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matchit_path_conversion() {
        assert_eq!(to_matchit_path("/users"), "/users");
        assert_eq!(to_matchit_path("/users/:id"), "/users/{id}");
        assert_eq!(
            to_matchit_path("/users/:userId/posts/:postId"),
            "/users/{userId}/posts/{postId}"
        );
        assert_eq!(to_matchit_path("/users/:id/profile"), "/users/{id}/profile");
    }
}
