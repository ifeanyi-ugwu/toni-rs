//! The tracing span each transport wraps `dispatch` in, named and attributed by the
//! OpenTelemetry semantic conventions (transports DESIGN §2.9).
//!
//! A `tracing` span declares its fields where it is created, so [`call`] declares every field any
//! transport records, empty, and each transport records its own: HTTP `http.request.method`,
//! `http.route`, `url.path`, `url.scheme`, `http.response.status_code`; RPC `rpc.system` (`"ulo"`),
//! `rpc.method` and, on a broker, `messaging.system`; gRPC `rpc.system = "grpc"`, `rpc.service`,
//! `rpc.method`, `rpc.grpc.status_code`. The span's display name, `GET /users/{id}` or
//! `users.v1.UserService/GetUser`, is the `otel.name` field. Every span carries `ulo.transport`
//! (the transport's key) and `ulo.handler`.

use tracing::Span;
use tracing::field::Empty;

pub const OTEL_NAME: &str = "otel.name";
pub const TRANSPORT: &str = "ulo.transport";
pub const HANDLER: &str = "ulo.handler";
pub const HTTP_REQUEST_METHOD: &str = "http.request.method";
pub const HTTP_ROUTE: &str = "http.route";
pub const URL_PATH: &str = "url.path";
pub const URL_SCHEME: &str = "url.scheme";
pub const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";
pub const RPC_SYSTEM: &str = "rpc.system";
pub const RPC_SERVICE: &str = "rpc.service";
pub const RPC_METHOD: &str = "rpc.method";
pub const RPC_GRPC_STATUS_CODE: &str = "rpc.grpc.status_code";
pub const MESSAGING_SYSTEM: &str = "messaging.system";

/// The span for one call: `name` as `otel.name`, `transport` as `ulo.transport`, `handler` as
/// `ulo.handler` when a handler matched, every other field above declared empty for the transport
/// to record.
pub fn call(transport: &'static str, name: &str, handler: Option<&str>) -> Span {
    // Each field name spells the constant of the same text above; `span.record` finds a field by
    // that text, and the macro takes names as written, not as constants.
    tracing::info_span!(
        "call",
        otel.name = name,
        ulo.transport = transport,
        ulo.handler = handler,
        http.request.method = Empty,
        http.route = Empty,
        url.path = Empty,
        url.scheme = Empty,
        http.response.status_code = Empty,
        rpc.system = Empty,
        rpc.service = Empty,
        rpc.method = Empty,
        rpc.grpc.status_code = Empty,
        messaging.system = Empty,
    )
}
