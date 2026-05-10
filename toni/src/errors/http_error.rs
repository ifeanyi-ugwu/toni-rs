//! HTTP handler error type — owns rendering, wraps domain `AppError`s.
//!
//! `HttpError` is the canonical error type the HTTP dispatcher flows through
//! its pipeline. Handlers don't have to return it directly: any type
//! implementing [`AppError`](crate::errors::AppError) gets a free conversion
//! via [`From<E: AppError> for HttpError`], so writing
//! `Result<Body, MyDomainError>` from a handler is fine — the framework
//! lifts the domain error into [`HttpError::AppError`] at the dispatcher
//! boundary and renders the canonical envelope.
//!
//! Use the named variants for trivial cases:
//!
//! ```ignore
//! fn find_user(id: &str) -> Result<User, HttpError> {
//!     db.find(id).ok_or_else(|| HttpError::not_found(format!("user {id}")))
//! }
//! ```
//!
//! Use [`HttpError::AppError`] (usually via `?` and the auto-`From` blanket)
//! when bubbling a domain error up to the framework for canonical rendering.
//!
//! For fully-custom rendering (a `Retry-After` header on a 429, an
//! arbitrary domain envelope) register a chain handler with `#[catch(T)]`
//! — it produces an `HttpResponse` directly, with full control over body
//! and headers. The chain runs ahead of `to_response()`'s fallback.
//!
//! `HttpError` itself does **not** implement `AppError`. The split is
//! deliberate: `AppError` is the domain-vocabulary trait (kind / message /
//! details), `HttpError` is the transport's wire-rendering type. Mixing
//! them would also break the `From<E: AppError>` blanket via std's
//! reflexive `From<T> for T`.

use std::sync::Arc;
use std::{borrow::Cow, fmt};

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::http_helpers::{Body, HttpResponse, IntoResponse};

/// HTTP error types that map to standard HTTP status codes.
///
/// Named variants are the convenience cases. The
/// [`AppError`](Self::AppError) variant wraps a domain error implementing
/// [`AppError`](crate::errors::AppError) for canonical-envelope rendering.
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

    /// Wraps a domain error implementing [`AppError`](crate::errors::AppError).
    /// Renders through the canonical envelope derived from the error's
    /// [`kind`](crate::errors::AppError::kind). Constructed automatically
    /// via the [`From<E: AppError>`] blanket — handlers normally don't build
    /// this variant directly.
    ///
    /// `Arc` rather than `Box` so `HttpError` stays `Clone`.
    AppError(Arc<dyn AppError + Send + Sync>),
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
            Self::AppError(e) => e.kind().http_status(),
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
            Self::AppError(e) => e.kind().http_reason(),
        }
    }

    /// Render this error as an [`HttpResponse`].
    ///
    /// Named variants and `Custom` produce the canonical envelope:
    /// ```json
    /// { "statusCode": 404, "message": "...", "error": "Not Found" }
    /// ```
    /// [`Self::AppError`] renders the wrapped error's `kind` / `message` /
    /// `details` through the same canonical shape.
    pub fn to_response(&self) -> HttpResponse {
        match self {
            Self::AppError(e) => render_app_error(e.as_ref()),
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

/// Canonical HTTP envelope rendering for an arbitrary [`AppError`]. Used by
/// [`HttpError::AppError`] and by tests / framework internals that need to
/// render an `AppError` to HTTP without going through the wrapper variant.
pub fn render_app_error(err: &dyn AppError) -> HttpResponse {
    let kind = err.kind();
    let mut body = json!({
        "statusCode": kind.http_status(),
        "message": err.message(),
        "error": kind.http_reason(),
    });
    if let Some(details) = err.details()
        && let Value::Object(map) = &mut body
    {
        map.insert("details".to_string(), details);
    }
    HttpResponse {
        status: kind.http_status(),
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
            // Surface the wrapped domain error so chain handlers can
            // downcast through `Error::source()` to the original type.
            // `dyn AppError + Send + Sync` upcasts to `dyn Error + 'static`
            // because `AppError: Error + 'static`.
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

/// Lift any [`AppError`] into [`HttpError::AppError`] so handlers returning
/// `Result<T, MyDomainError>` work via `?` and the macro's auto-conversion
/// at the dispatcher boundary.
///
/// This is a blanket — it covers every type implementing [`AppError`].
/// `HttpError` itself does not implement `AppError`, which keeps this from
/// conflicting with std's reflexive `From<T> for T`.
impl<E: AppError> From<E> for HttpError {
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
        impl AppError for DomainErr {
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
