//! Kinds to canonical codes, `grpc-status-details-bin`, and the HTTP-status table for a response a
//! pre-dispatch entry answered (transports DESIGN §2.4, §6.2).

use http::StatusCode;
use tonic::Code;
use ulo_transport::{CallError, ErrorKind};

/// The canonical code for `kind`: `BadRequest` and `Unprocessable` INVALID_ARGUMENT, `Conflict`
/// ABORTED, `Timeout` DEADLINE_EXCEEDED, `TooManyRequests` RESOURCE_EXHAUSTED, and so on.
pub fn code_for(kind: ErrorKind) -> Code {
    let _ = kind;
    todo!()
}

/// The code for a non-200 HTTP response a pre-dispatch entry answered, by the gRPC specification's
/// table: 400 INTERNAL, 401 UNAUTHENTICATED, 403 PERMISSION_DENIED, 404 UNIMPLEMENTED, 429, 502,
/// 503 and 504 UNAVAILABLE, any other UNKNOWN.
pub fn code_for_http(status: StatusCode) -> Code {
    let _ = status;
    todo!()
}

/// `err` as a status: its kind's code, its public message, and its details packed into a
/// `google.rpc.Status` in `grpc-status-details-bin` through tonic-types, a `Detail::Json` as a
/// `google.protobuf.Value` packed in an `Any`.
pub fn to_status(err: &CallError) -> tonic::Status {
    let _ = err;
    todo!()
}
