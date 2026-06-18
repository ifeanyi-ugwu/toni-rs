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

mod extensions;
pub use self::extensions::Extensions;

mod route_metadata;
pub use self::route_metadata::RouteMetadata;

mod sse;
pub use self::sse::{Sse, SseEvent, sse};

mod execution_result;
pub use self::execution_result::ExecutionResult;

/// Join a controller's route prefix with a handler's sub-path, normalizing slashes.
///
/// The `#[controller]` prefix lives on the struct and the sub-path on the `#[routes]` handler, so the
/// full path is composed at route-registration time rather than baked in by the macro.
///
/// `"/api" + "/users"` → `"/api/users"`; `"/" + "/x"` → `"/x"`; `"/api" + ""` → `"/api"`.
pub fn join_route(prefix: &str, sub_path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let sub_path = sub_path.trim_start_matches('/');
    if prefix.is_empty() {
        format!("/{}", sub_path)
    } else if sub_path.is_empty() {
        prefix.to_string()
    } else {
        format!("{}/{}", prefix, sub_path)
    }
}
