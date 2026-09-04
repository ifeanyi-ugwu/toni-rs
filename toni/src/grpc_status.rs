//! Transport-neutral mirror of `tonic::Status`.
//!
//! Lives in toni core so error handlers, guards, and the `ErrorResponse`
//! enum can name a "gRPC error response" without forcing toni core to
//! depend on tonic. The toni-grpc adapter converts to/from
//! `tonic::Status` at its boundary.

use std::sync::Arc;

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
///
/// A status built from a domain error keeps that error, so the code goes to the
/// wire and the type reaches the error chain. A status built by hand carries
/// nothing but what it was given.
#[derive(Debug, Clone)]
pub struct GrpcStatus {
    pub code: GrpcCode,
    pub message: String,
    source: Option<Arc<dyn Error>>,
}

impl GrpcStatus {
    pub fn new(code: GrpcCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
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

    /// Keep `error` on a status whose code was named rather than derived.
    ///
    /// The code and message stay as written and the chain is handed the error,
    /// which is how a handler answers with a code no [`ErrorKind`] reaches and
    /// still has `#[catch(MyError)]` match.
    pub fn caused_by<E: Error>(mut self, error: E) -> Self {
        self.source = Some(Arc::new(error));
        self
    }

    /// The error this status was built from, where it was built from one.
    pub fn source(&self) -> Option<&dyn Error> {
        self.source.as_deref()
    }

    /// Take the carried error, leaving the code and message behind.
    pub fn into_source(self) -> Option<Arc<dyn Error>> {
        self.source
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

/// The [`ErrorKind`] a gRPC code stands for, the inverse of [`grpc_code`] where
/// the table has an answer.
///
/// Five codes are outside it — `FailedPrecondition`, `OutOfRange`,
/// `AlreadyExists`, `DataLoss`, `Cancelled` — and answer `Internal`, which is
/// what a `GrpcStatus` renders as on a transport that reads kinds rather than
/// codes.
pub fn error_kind(code: GrpcCode) -> ErrorKind {
    match code {
        GrpcCode::InvalidArgument => ErrorKind::BadRequest,
        GrpcCode::Unauthenticated => ErrorKind::Unauthorized,
        GrpcCode::PermissionDenied => ErrorKind::Forbidden,
        GrpcCode::NotFound => ErrorKind::NotFound,
        GrpcCode::Aborted => ErrorKind::Conflict,
        GrpcCode::ResourceExhausted => ErrorKind::TooManyRequests,
        GrpcCode::DeadlineExceeded => ErrorKind::Timeout,
        GrpcCode::Unavailable => ErrorKind::Unavailable,
        GrpcCode::Unimplemented => ErrorKind::Unimplemented,
        _ => ErrorKind::Internal,
    }
}

/// A `GrpcStatus` is a `toni::Error`, so a handler can return one and name a
/// code no [`ErrorKind`] reaches.
impl Error for GrpcStatus {
    fn kind(&self) -> ErrorKind {
        error_kind(self.code)
    }

    fn message(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(&self.message)
    }
}

impl GrpcStatus {
    /// The status a domain error maps to, keeping the error itself.
    ///
    /// A `GrpcStatus` answers as itself rather than being re-derived, so the
    /// code a handler named is the code on the wire.
    pub fn of<E: Error>(error: E) -> Self {
        if let Some(status) = (&error as &dyn std::any::Any).downcast_ref::<Self>() {
            return status.clone();
        }
        Self {
            code: grpc_code(error.kind()),
            message: error.message().into_owned(),
            source: Some(Arc::new(error)),
        }
    }

    /// The status a domain error maps to, read through a reference.
    ///
    /// [`of`](Self::of) is the owned form and the one that keeps the error;
    /// this is what the framework calls where it holds an error it cannot take.
    pub fn from_error(error: &(dyn Error + Send + Sync)) -> Self {
        Self {
            code: grpc_code(error.kind()),
            message: error.message().into_owned(),
            source: None,
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

    /// `source()` answers a `toni::Error`, which has to be upcast before the
    /// standard downcast is reachable.
    fn carries<E: std::error::Error + 'static>(status: &GrpcStatus) -> bool {
        status
            .source()
            .is_some_and(|e| (e as &dyn std::error::Error).is::<E>())
    }

    #[test]
    fn a_status_built_from_an_error_keeps_it() {
        let status = GrpcStatus::of(OutOfStock);

        assert_eq!(status.code, GrpcCode::Aborted);
        assert!(
            carries::<OutOfStock>(&status),
            "the error itself has to survive the mapping"
        );
    }

    #[test]
    fn a_status_answers_as_itself() {
        // A handler naming a code the kind table cannot reach: re-deriving one
        // from `kind()` would answer `Internal` instead.
        let named = GrpcStatus::new(GrpcCode::FailedPrecondition, "the window is closed");

        assert_eq!(GrpcStatus::of(named).code, GrpcCode::FailedPrecondition);
    }

    #[test]
    fn a_named_code_carries_an_error_too() {
        let status =
            GrpcStatus::new(GrpcCode::OutOfRange, "past the last page").caused_by(OutOfStock);

        assert_eq!(status.code, GrpcCode::OutOfRange);
        assert!(carries::<OutOfStock>(&status));
    }

    #[test]
    fn a_borrowed_error_maps_without_being_kept() {
        // `from_error` cannot take the value, so nothing is there to carry.
        assert!(GrpcStatus::from_error(&OutOfStock).source().is_none());
    }

    #[test]
    fn every_mapped_code_reads_back_as_its_kind() {
        for kind in [
            ErrorKind::BadRequest,
            ErrorKind::Unauthorized,
            ErrorKind::Forbidden,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::TooManyRequests,
            ErrorKind::Timeout,
            ErrorKind::Unavailable,
            ErrorKind::Unimplemented,
            ErrorKind::Internal,
        ] {
            assert_eq!(
                error_kind(grpc_code(kind)),
                kind,
                "{kind:?} did not survive"
            );
        }
        // The two kinds that share `InvalidArgument` cannot both come back.
        assert_eq!(
            error_kind(grpc_code(ErrorKind::UnprocessableEntity)),
            ErrorKind::BadRequest
        );
    }

    #[test]
    fn a_code_outside_the_table_reads_as_internal() {
        for code in [
            GrpcCode::FailedPrecondition,
            GrpcCode::OutOfRange,
            GrpcCode::AlreadyExists,
            GrpcCode::DataLoss,
            GrpcCode::Cancelled,
        ] {
            assert_eq!(error_kind(code), ErrorKind::Internal, "{code:?}");
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
        let status = GrpcStatus::of(OutOfStock);
        assert_eq!(status.code, GrpcCode::Aborted);
        assert_eq!(status.message, "sku is out of stock");
    }
}
