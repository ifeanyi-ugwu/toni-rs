//! Transport-neutral mirror of `tonic::Status`.
//!
//! Lives in toni core so error handlers, guards, and the `ErrorResponse`
//! enum can name a "gRPC error response" without forcing toni core to
//! depend on tonic. The toni-grpc adapter converts to/from
//! `tonic::Status` at its boundary.

use crate::errors::{Error, ErrorKind};

/// gRPC status codes — wire-format equivalents of the canonical gRPC codes.
/// Numeric values match `tonic::Code` so a round-trip via `as i32` works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum GrpcCode {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
}

impl GrpcCode {
    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Ok,
            1 => Self::Cancelled,
            3 => Self::InvalidArgument,
            4 => Self::DeadlineExceeded,
            5 => Self::NotFound,
            6 => Self::AlreadyExists,
            7 => Self::PermissionDenied,
            8 => Self::ResourceExhausted,
            9 => Self::FailedPrecondition,
            10 => Self::Aborted,
            11 => Self::OutOfRange,
            12 => Self::Unimplemented,
            13 => Self::Internal,
            14 => Self::Unavailable,
            15 => Self::DataLoss,
            16 => Self::Unauthenticated,
            _ => Self::Unknown,
        }
    }
}

/// Transport-neutral gRPC error response. The toni-grpc adapter converts
/// this into a `tonic::Status` at the wire boundary.
#[derive(Debug, Clone)]
pub struct GrpcStatus {
    pub code: GrpcCode,
    pub message: String,
}

impl GrpcStatus {
    pub fn new(code: GrpcCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(GrpcCode::PermissionDenied, message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(GrpcCode::InvalidArgument, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(GrpcCode::Internal, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(GrpcCode::NotFound, message)
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(GrpcCode::Unauthenticated, message)
    }
}

impl std::fmt::Display for GrpcStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gRPC {:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for GrpcStatus {}

/// The gRPC code for an [`ErrorKind`], the way
/// [`http_status`](crate::errors::http_status) gives its HTTP status.
///
/// Follows the canonical HTTP-to-gRPC table, so a `NotFound` is `NOT_FOUND`
/// on the wire and a caller's generated client sees what it expects.
/// `Conflict` maps to `Aborted`, the table's answer for 409; a service that
/// means "this already exists" rather than "the state moved under you" says
/// `GrpcCode::AlreadyExists` itself.
pub fn grpc_code(kind: ErrorKind) -> GrpcCode {
    match kind {
        ErrorKind::BadRequest => GrpcCode::InvalidArgument,
        ErrorKind::Unauthorized => GrpcCode::Unauthenticated,
        ErrorKind::Forbidden => GrpcCode::PermissionDenied,
        ErrorKind::NotFound => GrpcCode::NotFound,
        ErrorKind::Conflict => GrpcCode::Aborted,
        ErrorKind::UnprocessableEntity => GrpcCode::InvalidArgument,
        ErrorKind::TooManyRequests => GrpcCode::ResourceExhausted,
        ErrorKind::Timeout => GrpcCode::DeadlineExceeded,
        ErrorKind::Unavailable => GrpcCode::Unavailable,
        ErrorKind::Unimplemented => GrpcCode::Unimplemented,
        ErrorKind::Internal => GrpcCode::Internal,
    }
}

impl From<ErrorKind> for GrpcCode {
    fn from(kind: ErrorKind) -> Self {
        grpc_code(kind)
    }
}

/// Lifts any domain error into a gRPC status, the way `?` lifts one into
/// [`HttpError`](crate::errors::HttpError) on the other transports.
///
/// A gRPC handler's signature belongs to tonic, so it answers with
/// `tonic::Status` and the orphan rule keeps toni from converting into it.
/// `toni_grpc::to_status` closes that last hop; this is the mapping under it,
/// and what a `#[catch]`-registered handler returns.
impl<E: Error> From<E> for GrpcStatus {
    fn from(e: E) -> Self {
        Self {
            code: grpc_code(e.kind()),
            message: e.message().into_owned(),
        }
    }
}

/// What a gRPC call answers with. `Ok(())` means the delegate ran and its typed
/// response is in the macro's side-channel; `Err` short-circuits the chain. The
/// `R` of [`Interceptor`](crate::traits_helpers::Interceptor) on this transport.
pub type GrpcHandlerResult = Result<(), GrpcStatus>;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct OutOfStock;

    impl std::fmt::Display for OutOfStock {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "sku is out of stock")
        }
    }

    impl std::error::Error for OutOfStock {}

    impl Error for OutOfStock {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Conflict
        }
    }

    #[test]
    fn every_kind_maps_to_its_canonical_code() {
        let table = [
            (ErrorKind::BadRequest, GrpcCode::InvalidArgument),
            (ErrorKind::Unauthorized, GrpcCode::Unauthenticated),
            (ErrorKind::Forbidden, GrpcCode::PermissionDenied),
            (ErrorKind::NotFound, GrpcCode::NotFound),
            (ErrorKind::Conflict, GrpcCode::Aborted),
            (ErrorKind::UnprocessableEntity, GrpcCode::InvalidArgument),
            (ErrorKind::TooManyRequests, GrpcCode::ResourceExhausted),
            (ErrorKind::Timeout, GrpcCode::DeadlineExceeded),
            (ErrorKind::Unavailable, GrpcCode::Unavailable),
            (ErrorKind::Unimplemented, GrpcCode::Unimplemented),
            (ErrorKind::Internal, GrpcCode::Internal),
        ];
        for (kind, code) in table {
            assert_eq!(grpc_code(kind), code, "kind {kind:?}");
        }
    }

    #[test]
    fn a_domain_error_carries_its_kind_and_message() {
        let status = GrpcStatus::from(OutOfStock);
        assert_eq!(status.code, GrpcCode::Aborted);
        assert_eq!(status.message, "sku is out of stock");
    }
}
