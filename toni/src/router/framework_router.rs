use std::collections::HashMap;
use std::sync::Arc;

use crate::http_helpers::{Body, HttpMethod, HttpRequest, HttpResponse, PathParams};
use crate::injector::InstanceWrapper;

/// Convert a toni path (`:param` style) to matchit's `{param}` style.
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
            // param ran to end of string
            if !out.ends_with('}') {
                out.push('}');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Builds a [`FrameworkRouter`].
///
/// matchit requires each path to be inserted exactly once, so routes are
/// collected first and grouped by path before the router is sealed.
pub struct FrameworkRouterBuilder {
    /// path → (method_uppercase → wrapper)
    routes: HashMap<String, HashMap<String, Arc<InstanceWrapper>>>,
}

impl FrameworkRouterBuilder {
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    pub fn insert(&mut self, method: HttpMethod, path: &str, wrapper: Arc<InstanceWrapper>) {
        self.routes
            .entry(path.to_owned())
            .or_default()
            .insert(method.as_str().to_uppercase(), wrapper);
    }

    pub fn build(self) -> FrameworkRouter {
        let mut inner = matchit::Router::new();
        for (path, method_map) in self.routes {
            let matchit_path = to_matchit_path(&path);
            // matchit panics on duplicate inserts — each path is unique here.
            inner.insert(matchit_path, method_map).unwrap_or_else(|e| {
                tracing::error!(path, error = %e, "failed to register route in framework router");
            });
        }
        FrameworkRouter { inner }
    }
}

impl Default for FrameworkRouterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// The framework's own path router.
///
/// Immutable after construction. Stored inside [`RequestDispatcher`] behind an
/// `Arc` so it can be shared across async tasks without copying.
pub struct FrameworkRouter {
    inner: matchit::Router<HashMap<String, Arc<InstanceWrapper>>>,
}

// Safety: InstanceWrapper is Send + Sync (all its fields require Send + Sync).
// matchit::Router<T> is Send + Sync when T: Send + Sync.
unsafe impl Send for FrameworkRouter {}
unsafe impl Sync for FrameworkRouter {}

impl FrameworkRouter {
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

                if let Some(wrapper) = matched.value.get(&method) {
                    wrapper.handle_request(req).await
                } else {
                    let allowed: Vec<&str> = matched.value.keys().map(String::as_str).collect();
                    method_not_allowed_response(&method, &path, &allowed)
                }
            }
            Err(_) => not_found_response(&method, &path),
        }
    }
}

fn not_found_response(method: &str, path: &str) -> HttpResponse {
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

fn method_not_allowed_response(method: &str, path: &str, _allowed: &[&str]) -> HttpResponse {
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
    fn test_to_matchit_path_no_params() {
        assert_eq!(to_matchit_path("/users"), "/users");
        assert_eq!(to_matchit_path("/"), "/");
    }

    #[test]
    fn test_to_matchit_path_single_param() {
        assert_eq!(to_matchit_path("/users/:id"), "/users/{id}");
    }

    #[test]
    fn test_to_matchit_path_multiple_params() {
        assert_eq!(
            to_matchit_path("/users/:userId/posts/:postId"),
            "/users/{userId}/posts/{postId}"
        );
    }

    #[test]
    fn test_to_matchit_path_trailing_param() {
        assert_eq!(to_matchit_path("/users/:id/profile"), "/users/{id}/profile");
    }
}
