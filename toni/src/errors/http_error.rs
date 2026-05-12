//! HTTP handler error type.
//!
//! `HttpError` is the error carried across the HTTP dispatcher and adapter
//! boundary. Handlers may return any type implementing
//! [`toni::Error`](crate::errors::Error) from their function body — the
//! [`From<E: Error>`] blanket lifts it into [`HttpError::AppError`] at
//! the macro boundary, and [`HttpError::to_response`] renders the canonical
//! envelope.
//!
//! Named variants cover the trivial cases:
//!
//! ```ignore
//! fn find_user(id: &str) -> Result<User, HttpError> {
//!     db.find(id).ok_or_else(|| HttpError::not_found(format!("user {id}")))
//! }
//! ```
//!
//! Custom rendering (headers like `Retry-After`, domain-specific body shapes)
//! goes through a `#[catch(T)]` chain handler — it produces an `HttpResponse`
//! directly and runs ahead of [`HttpError::to_response`]'s default rendering.
//!
//! `HttpError` does not implement [`toni::Error`](crate::errors::Error); the
//! `From` blanket requires source and target to be distinct types.

use std::sync::Arc;
use std::{borrow::Cow, fmt};

use serde_json::{Value, json};

use crate::errors::{Error, ErrorKind};
use crate::http_helpers::{Body, HttpResponse, IntoResponse};

/// HTTP status code for an [`ErrorKind`]. The HTTP transport owns this
/// mapping — `ErrorKind` itself is transport-independent.
pub fn http_status(kind: ErrorKind) -> u16 {
    match kind {
        ErrorKind::BadRequest => 400,
        ErrorKind::Unauthorized => 401,
        ErrorKind::Forbidden => 403,
        ErrorKind::NotFound => 404,
        ErrorKind::Conflict => 409,
        ErrorKind::UnprocessableEntity => 422,
        ErrorKind::TooManyRequests => 429,
        ErrorKind::Timeout => 504,
        ErrorKind::Unavailable => 503,
        ErrorKind::Unimplemented => 501,
        ErrorKind::Internal => 500,
    }
}

/// HTTP reason phrase for an [`ErrorKind`], used in the canonical envelope.
pub fn http_reason(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::BadRequest => "Bad Request",
        ErrorKind::Unauthorized => "Unauthorized",
        ErrorKind::Forbidden => "Forbidden",
        ErrorKind::NotFound => "Not Found",
        ErrorKind::Conflict => "Conflict",
        ErrorKind::UnprocessableEntity => "Unprocessable Entity",
        ErrorKind::TooManyRequests => "Too Many Requests",
        ErrorKind::Timeout => "Gateway Timeout",
        ErrorKind::Unavailable => "Service Unavailable",
        ErrorKind::Unimplemented => "Not Implemented",
        ErrorKind::Internal => "Internal Server Error",
    }
}

/// HTTP error variants — convenience constructors plus a wrapper for
/// user-domain [`toni::Error`](crate::errors::Error) values.
#[derive(Debug, Clone)]
pub enum HttpError {
    /// 400 Bad Request - Client sent invalid data
    BadRequest(String),

    /// 401 Unauthorized - Authentication required or failed
    Unauthorized(String),

    /// 403 Forbidden - Client doesn't have permission
    Forbidden(String),

    /// 404 Not Found - Resource doesn't exist
    NotFound(String),

    /// 409 Conflict - Request conflicts with current state
    Conflict(String),

    /// 422 Unprocessable Entity - Validation failed
    UnprocessableEntity(String),

    /// 500 Internal Server Error - Server-side error
    InternalServerError(String),

    /// Custom error with any status code
    Custom { status: u16, message: String },

    /// Carries a user-domain error implementing
    /// [`toni::Error`](crate::errors::Error). Constructed by the
    /// [`From<E: Error>`] blanket; handlers don't build this variant by
    /// hand. `Arc` rather than `Box` so the enum stays `Clone`.
    AppError(Arc<dyn Error + Send + Sync>),
}

impl HttpError {
    /// Create a 400 Bad Request error
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    /// Create a 401 Unauthorized error
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized(message.into())
    }

    /// Create a 403 Forbidden error
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    /// Create a 404 Not Found error
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Create a 409 Conflict error
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    /// Create a 422 Unprocessable Entity error
    pub fn unprocessable_entity(message: impl Into<String>) -> Self {
        Self::UnprocessableEntity(message.into())
    }

    /// Create a 500 Internal Server Error
    pub fn internal_server_error(message: impl Into<String>) -> Self {
        Self::InternalServerError(message.into())
    }

    /// Create a custom error with any status code
    pub fn custom(status: u16, message: impl Into<String>) -> Self {
        Self::Custom {
            status,
            message: message.into(),
        }
    }

    /// Get the HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Conflict(_) => 409,
            Self::UnprocessableEntity(_) => 422,
            Self::InternalServerError(_) => 500,
            Self::Custom { status, .. } => *status,
            Self::AppError(e) => http_status(e.kind()),
        }
    }

    /// Get the error message.
    pub fn message(&self) -> Cow<'_, str> {
        match self {
            Self::BadRequest(msg)
            | Self::Unauthorized(msg)
            | Self::Forbidden(msg)
            | Self::NotFound(msg)
            | Self::Conflict(msg)
            | Self::UnprocessableEntity(msg)
            | Self::InternalServerError(msg) => Cow::Borrowed(msg),
            Self::Custom { message, .. } => Cow::Borrowed(message),
            Self::AppError(e) => e.message(),
        }
    }

    /// Get the error reason phrase / type label.
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "Bad Request",
            Self::Unauthorized(_) => "Unauthorized",
            Self::Forbidden(_) => "Forbidden",
            Self::NotFound(_) => "Not Found",
            Self::Conflict(_) => "Conflict",
            Self::UnprocessableEntity(_) => "Unprocessable Entity",
            Self::InternalServerError(_) => "Internal Server Error",
            Self::Custom { .. } => "Error",
            Self::AppError(e) => http_reason(e.kind()),
        }
    }

    /// Render as an [`HttpResponse`] using the canonical envelope:
    /// ```json
    /// { "statusCode": 404, "message": "...", "error": "Not Found" }
    /// ```
    /// For [`AppError`](Self::AppError), reads `kind` / `message` / `details`
    /// from the wrapped error; the named variants use their fixed status
    /// and reason phrase.
    pub fn to_response(&self) -> HttpResponse {
        match self {
            Self::AppError(e) => render_error(e.as_ref()),
            _ => HttpResponse {
                status: self.status_code(),
                body: Some(Body::json(json!({
                    "statusCode": self.status_code(),
                    "message": self.message(),
                    "error": self.error_type(),
                }))),
                headers: vec![],
            },
        }
    }
}

/// Render an arbitrary [`toni::Error`](crate::errors::Error) as the
/// canonical HTTP envelope. Merges `details()` into the body when present.
pub fn render_error(err: &dyn Error) -> HttpResponse {
    let kind = err.kind();
    let mut body = json!({
        "statusCode": http_status(kind),
        "message": err.message(),
        "error": http_reason(kind),
    });
    if let Some(details) = err.details()
        && let Value::Object(map) = &mut body
    {
        map.insert("details".to_string(), details);
    }
    HttpResponse {
        status: http_status(kind),
        body: Some(Body::json(body)),
        headers: vec![],
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AppError(e) => write!(f, "{}: {}", e.kind().name(), e.message()),
            _ => write!(f, "{}: {}", self.error_type(), self.message()),
        }
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // Expose the wrapped error so `Error::downcast_ref::<MyError>()`
            // reaches the original type from `#[catch]` chain handlers.
            Self::AppError(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for HttpError {
    fn from(e: serde_json::Error) -> Self {
        Self::InternalServerError(e.to_string())
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> HttpResponse {
        self.to_response()
    }
}

/// Lift any [`toni::Error`](crate::errors::Error) into
/// [`HttpError::AppError`]. Handlers returning `Result<T, MyDomainError>`
/// use this via `?` and via the macro's auto-conversion at the dispatcher
/// boundary.
impl<E: Error> From<E> for HttpError {
    fn from(e: E) -> Self {
        Self::AppError(Arc::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ErrorKind;

    #[test]
    fn test_not_found_error() {
        let error = HttpError::not_found("User not found");
        assert_eq!(error.status_code(), 404);
        assert_eq!(error.message(), "User not found");
        assert_eq!(error.error_type(), "Not Found");
    }

    #[test]
    fn test_bad_request_error() {
        let error = HttpError::bad_request("Invalid input");
        assert_eq!(error.status_code(), 400);
        assert_eq!(error.message(), "Invalid input");
    }

    #[test]
    fn test_custom_error() {
        let error = HttpError::custom(418, "I'm a teapot");
        assert_eq!(error.status_code(), 418);
        assert_eq!(error.message(), "I'm a teapot");
    }

    #[test]
    fn test_to_response() {
        let error = HttpError::not_found("Resource not found");
        let response = error.to_response();
        assert_eq!(response.status, 404);
        assert!(response.body.is_some());
    }

    #[test]
    fn test_app_error_variant_renders_canonically() {
        #[derive(Debug)]
        struct DomainErr;
        impl fmt::Display for DomainErr {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("nope")
            }
        }
        impl std::error::Error for DomainErr {}
        impl Error for DomainErr {
            fn kind(&self) -> ErrorKind {
                ErrorKind::NotFound
            }
        }

        let err: HttpError = DomainErr.into();
        let resp = err.to_response();
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn test_display() {
        let error = HttpError::unauthorized("Token expired");
        assert_eq!(format!("{}", error), "Unauthorized: Token expired");
    }
}
