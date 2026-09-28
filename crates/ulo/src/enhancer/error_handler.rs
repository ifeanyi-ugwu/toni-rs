use crate::async_trait;
use crate::context::ExecutionContext;
use std::error::Error;

/// Convenience alias for the borrowed error reference passed to handlers.
///
/// The chain owns the boxed error and lends it to each handler in turn —
/// handlers borrow, downcast, and either claim (`Some`) or fall through
/// (`None`). Borrowing avoids forcing user errors to be `Clone` so each
/// chain iteration can re-box them.
pub type ChainError<'a> = &'a (dyn Error + Send + Sync + 'static);

/// Customize how errors are turned into responses.
///
/// Handlers are tried in order (method > controller > global) until one returns `Some`. Return
/// `None` to pass the error to the next one.
///
/// When every handler passes, the transport renders the error itself: the canonical envelope,
/// carrying the status, code or frame its [`ErrorKind`](crate::errors::ErrorKind) maps to. That
/// rendering is not an `ErrorHandler` and a handler catching a domain error cannot reproduce it —
/// it reads `kind()`, `message()` and `details()` off [`Error`](crate::errors::Error), and this
/// trait is handed the `std::error::Error` supertrait, which those do not reach. A handler that
/// catches the transport's own error type holds the renderer: `HttpError::to_response`,
/// `RpcError::to_data` and `WsError::to_message` each build the same envelope, `to_message`
/// answering a `WsError::Refused` with its Close instead.
#[async_trait]
pub trait ErrorHandler<C: ?Sized + ExecutionContext, R>: Send + Sync {
    async fn handle_error(&self, error: ChainError<'_>, ctx: &C) -> Option<R>;
}

/// An error handler held in a `static` is shared by reference, written
/// `#[use_error_handlers(value &FALLBACK)]`.
#[diagnostic::do_not_recommend]
#[async_trait]
impl<'h, C, R, H> ErrorHandler<C, R> for &'h H
where
    C: ?Sized + ExecutionContext,
    R: Send + 'static,
    H: ErrorHandler<C, R> + ?Sized,
{
    async fn handle_error(&self, error: ChainError<'_>, ctx: &C) -> Option<R> {
        (**self).handle_error(error, ctx).await
    }
}
