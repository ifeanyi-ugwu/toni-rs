//! Result shape every transport's controller boundary returns.
//!
//! Carries either the rendered response (success path) or the transport's
//! handler error type (error path). The dispatcher runs the
//! [`ErrorHandler`](crate::traits_helpers::ErrorHandler) chain on the typed
//! error; if no chain handler claims, the error renders itself via the
//! transport's
//! inherent rendering method (`HttpError::to_response`,
//! `RpcError::to_data`, `WsError::to_message`).
//!
//! `R` is the success type (per transport: `HttpResponse`,
//! `Option<RpcData>`, `WsHandlerOutput`); `E` is the transport's handler
//! error type. Domain errors flow into `E` via the transport-specific
//! `From<X: Error>` blanket — handlers return their own focused error
//! types and the macro converts at the dispatcher boundary.

/// Outcome of running a controller / handler at the macro boundary.
///
/// `Ok(R)` is the rendered response (concrete per transport). `Err(E)`
/// is the transport's handler error type — `HttpError` for HTTP,
/// `RpcError` for RPC, `WsError` for WS.
pub enum ExecutionResult<R, E> {
    Ok(R),
    Err(E),
}

impl<R, E> ExecutionResult<R, E> {
    /// Construct a success result.
    pub fn ok(value: R) -> Self {
        Self::Ok(value)
    }

    /// Construct an error result.
    pub fn err(error: E) -> Self {
        Self::Err(error)
    }
}

impl<R, E> From<R> for ExecutionResult<R, E>
where
    R: Sized,
{
    fn from(value: R) -> Self {
        Self::Ok(value)
    }
}

impl<R, E> From<Result<R, E>> for ExecutionResult<R, E> {
    fn from(result: Result<R, E>) -> Self {
        match result {
            Ok(value) => Self::Ok(value),
            Err(err) => Self::Err(err),
        }
    }
}
