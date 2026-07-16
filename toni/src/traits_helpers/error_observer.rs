//! Universal observer for errors that pass through the framework's chain.
//!
//! Observers are a fire-and-forget tap on the error pipeline. They don't
//! shape responses — that's the transport's rendering and the
//! `ErrorHandler<C, R>` chain — they observe.
//!
//! The chain fires on framework events (guard rejections, panic recovery,
//! missing routes). User errors are rendered by the transport and don't
//! reach the chain, so observers don't see them by default; to log user
//! errors, do it inside a `#[catch(MyError)]` handler. For "log every
//! non-2xx," reach for response middleware.

use std::error::Error;

use crate::async_trait;
use crate::context::HandlerContext;

/// Universal error observer. Implementors get notified each time an error
/// passes through the framework's chain.
///
/// Observation is fire-and-forget — no return value, no way to reshape the
/// response. Response shaping belongs to the transport's rendering and
/// `ErrorHandler<C, R>`; `ErrorObserver` is for cross-cutting concerns
/// (logging, metrics, tracing, Sentry integrations).
#[async_trait]
pub trait ErrorObserver: Send + Sync {
    async fn observe<'a>(
        &'a self,
        error: &'a (dyn Error + Send + Sync + 'static),
        ctx: &'a mut (dyn HandlerContext + 'a),
    );
}
