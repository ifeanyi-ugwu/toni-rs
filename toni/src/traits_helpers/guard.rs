use async_trait::async_trait;

use crate::context::HandlerContext;

/// A guard decides whether the current request is allowed to proceed.
///
/// `C` is the per-request context. Implement `Guard<HttpContext>` /
/// `Guard<RpcContext>` / `Guard<WsContext>` for transport-specific guards, or
/// `impl<C: HandlerContext + ?Sized> Guard<C> for ...` for a guard that runs
/// on every transport.
///
/// The context is passed by exclusive reference, so a guard may attach data to
/// `ctx.extensions_mut()` for the handler to read once activation succeeds.
#[async_trait]
pub trait Guard<C: ?Sized + HandlerContext>: Send + Sync {
    async fn can_activate(&self, context: &mut C) -> bool;
}
