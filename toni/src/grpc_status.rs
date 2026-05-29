//! Transport-neutral mirror of `tonic::Status`.
//!
//! Lives in toni core so error handlers, guards, and the `ErrorResponse`
//! enum can name a "gRPC error response" without forcing toni core to
//! depend on tonic. The toni-grpc adapter converts to/from
//! `tonic::Status` at its boundary.

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
