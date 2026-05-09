//! Universal observer for errors that pass through the framework's chain.
//!
//! Observers are a fire-and-forget tap on the error pipeline — they don't
//! shape responses (that's [`AppError::into_*`](crate::AppError) or the
//! `ErrorHandler<C, R>` chain), they observe.
//!
//! Today's chain only fires on framework-generated errors (guard
//! rejections, panic recovery, missing routes). User errors render at
//! the macro boundary via `AppError` and don't reach the chain — so the
//! observer doesn't see them either. If you want to log user errors,
//! override their `AppError::into_http_response` (or the RPC/WS
//! equivalent) to call your logging there. If you want "log every
//! non-2xx," that's response middleware territory.

use std::error::Error;

use crate::async_trait;
use crate::context::HandlerContext;

/// Universal error observer. Implementors get notified each time an error
/// passes through the framework's chain.
///
/// The observation is fire-and-forget — there's no return value, no way to
/// reshape the response. That separation is deliberate: response shaping
/// belongs to [`AppError`](crate::AppError) and `ErrorHandler<C, R>`;
/// `ErrorObserver` is for cross-cutting concerns (logging, metrics,
/// tracing, Sentry integrations) that should not lie about what they
/// produce.
#[async_trait]
pub trait ErrorObserver: Send + Sync {
    async fn observe<'a>(
        &'a self,
        error: &'a (dyn Error + Send + Sync + 'static),
        ctx: &'a (dyn HandlerContext + 'a),
    );
}
