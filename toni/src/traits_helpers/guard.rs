use async_trait::async_trait;

use crate::context::HandlerContext;
use crate::injector::Context;

/// A guard decides whether the current request is allowed to proceed.
///
/// `C` is the per-request context. Implement `Guard<HttpContext>` /
/// `Guard<RpcContext>` / `Guard<WsContext>` for transport-specific guards, or
/// `impl<C: HandlerContext + ?Sized> Guard<C> for ...` for a guard that runs
/// on every transport.
// TODO: drop `= Context` default once the legacy `Context` is removed.
#[async_trait]
pub trait Guard<C: ?Sized + HandlerContext = Context>: Send + Sync {
    async fn can_activate(&self, context: &C) -> bool;
}
