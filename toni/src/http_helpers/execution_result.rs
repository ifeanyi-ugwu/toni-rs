//! Result shape every HTTP controller's `execute` method returns.
//!
//! Carries either the rendered response (success path) or the user's typed
//! error (error path). The dispatcher uses the typed error to fan
//! [`ErrorObserver`](crate::traits_helpers::ErrorObserver)s and run the
//! [`ErrorHandler`](crate::traits_helpers::ErrorHandler) chain on it; if no
//! chain handler claims, it falls back to the error's
//! [`AppError::into_http_response`](crate::AppError::into_http_response)
//! envelope.
//!
//! Preserving the typed error past the macro boundary is what lets observers
//! and per-scope `#[catch]` handlers see *user* errors uniformly with
//! framework-generated events. Without this shape, the user's `MyError` would
//! be consumed by `into_response()` before either could observe or claim it.

use crate::AppError;
use crate::http_helpers::HttpResponse;

/// What an HTTP controller's `execute` produces.
///
/// `Ok` means the handler returned a value that rendered successfully.
/// `Err` carries the user's typed error as a boxed [`AppError`] so the
/// dispatcher can route it through observers + the error chain before
/// rendering the canonical envelope.
pub enum ExecutionResult {
    Ok(HttpResponse),
    Err(Box<dyn AppError + Send + Sync>),
}

impl From<HttpResponse> for ExecutionResult {
    fn from(response: HttpResponse) -> Self {
        Self::Ok(response)
    }
}

impl<E: AppError + Send + Sync> From<E> for ExecutionResult {
    fn from(err: E) -> Self {
        Self::Err(Box::new(err))
    }
}
