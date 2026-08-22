#[path = "body.enum.rs"]
mod body;
pub use self::body::{Body, BoxBody};
pub use bytes::Bytes;

#[path = "http_response.enum.rs"]
mod http_response;
pub use self::http_response::{HttpResponse, HttpResponseBuilder, HttpResponseDefault};

mod path_params;
pub use self::path_params::PathParams;

mod request_body;
pub use self::request_body::{RequestBody, RequestBoxBody};

#[path = "http_request.struct.rs"]
mod http_request;
pub use self::http_request::{HttpRequest, RequestPart};

#[path = "http_method.enum.rs"]
mod http_method;
pub use self::http_method::HttpMethod;

#[path = "into_response.rs"]
mod into_response;
pub use self::into_response::IntoResponse;

mod sse;
pub use self::sse::{Sse, SseEvent, sse};

mod execution_result;
pub use self::execution_result::ExecutionResult;

/// Join a controller's route prefix with a handler's sub-path, normalizing slashes.
///
/// The `#[controller]` prefix lives on the struct and the sub-path on the `#[routes]` handler, so the
/// full path is composed at route-registration time rather than baked in by the macro.
///
/// Trailing slashes are insignificant: the joined path never carries one (except the root `/`),
/// and [`AdapterContext`](crate::AdapterContext) trims them from incoming request paths, so
/// `/app` and `/app/` address the same route.
///
/// `"/api" + "/users"` → `"/api/users"`; `"/" + "/x"` → `"/x"`; `"/api" + ""` → `"/api"`;
/// `"/api" + "/users/"` → `"/api/users"`.
pub fn join_route(prefix: &str, sub_path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let sub_path = sub_path.trim_matches('/');
    if prefix.is_empty() {
        format!("/{}", sub_path)
    } else if sub_path.is_empty() {
        prefix.to_string()
    } else {
        format!("{}/{}", prefix, sub_path)
    }
}

/// Trim trailing slashes from a path, preserving the root `/`.
pub fn trim_trailing_slashes(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}

#[cfg(test)]
mod join_route_tests {
    use super::{join_route, trim_trailing_slashes};

    #[test]
    fn joined_paths_never_carry_a_trailing_slash() {
        assert_eq!(join_route("/app", "/"), "/app");
        assert_eq!(join_route("/app", ""), "/app");
        assert_eq!(join_route("/app", "/users/"), "/app/users");
        assert_eq!(join_route("/app/", "/users"), "/app/users");
        assert_eq!(join_route("/", "/x"), "/x");
        assert_eq!(join_route("/", "/"), "/");
        assert_eq!(join_route("", ""), "/");
    }

    #[test]
    fn trim_preserves_root_and_inner_slashes() {
        assert_eq!(trim_trailing_slashes("/app/"), "/app");
        assert_eq!(trim_trailing_slashes("/app///"), "/app");
        assert_eq!(trim_trailing_slashes("/a/b"), "/a/b");
        assert_eq!(trim_trailing_slashes("/"), "/");
        assert_eq!(trim_trailing_slashes("///"), "/");
    }
}
