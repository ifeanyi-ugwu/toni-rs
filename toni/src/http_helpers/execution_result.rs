//! Result shape every transport's controller boundary returns.
//!
//! Carries either the rendered response (success path) or the user's typed
//! error (error path). The dispatcher uses the typed error to fan
//! [`ErrorObserver`](crate::traits_helpers::ErrorObserver)s and run the
//! [`ErrorHandler`](crate::traits_helpers::ErrorHandler) chain on it; if no
//! chain handler claims, it falls back to the error's `AppError::into_*`
//! envelope (HTTP / RPC / WS).
//!
//! Preserving the typed error past the macro boundary is what lets observers
//! and per-scope chain handlers see *user* errors uniformly with framework-
//! generated events. Without this shape, the user's `MyError` would be
//! consumed at the rendering call before either could observe or claim it.
//!
//! The success type is parameterised so each transport can carry its own
//! concrete shape — `HttpResponse` for HTTP, `Option<RpcData>` for RPC,
//! `WsHandlerOutput` for WS.

use crate::AppError;

/// Outcome of running a controller / handler at the macro boundary.
///
/// `Ok(R)` is the success-side response (concrete per transport). `Err`
/// preserves the typed error as a boxed [`AppError`] so the dispatcher can
/// route it through observers + the error chain before falling back to the
/// canonical envelope.
pub enum ExecutionResult<R> {
    Ok(R),
    Err(Box<dyn AppError + Send + Sync>),
}

impl<R> ExecutionResult<R> {
    /// Construct a success result.
    pub fn ok(value: R) -> Self {
        Self::Ok(value)
    }

    /// Box a user-typed error and construct an error result.
    pub fn err<E: AppError + Send + Sync>(error: E) -> Self {
        Self::Err(Box::new(error))
    }
}

impl<R> From<R> for ExecutionResult<R> {
    fn from(value: R) -> Self {
        Self::Ok(value)
    }
}

impl<R, E: AppError + Send + Sync> From<Result<R, E>> for ExecutionResult<R> {
    fn from(result: Result<R, E>) -> Self {
        match result {
            Ok(value) => Self::Ok(value),
            Err(err) => Self::Err(Box::new(err)),
        }
    }
}
