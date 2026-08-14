use async_trait::async_trait;

use crate::context::HandlerContext;

/// A guard decides whether the current request is allowed to proceed.
///
/// `C` is the per-request context. Implement `Guard<HttpContext>` /
/// `Guard<RpcContext>` / `Guard<WsContext>` for transport-specific guards, or
/// `impl<C: HandlerContext + ?Sized> Guard<C> for ...` for a guard that runs
/// on every transport.
///
/// A guard may attach data to `ctx.extensions()` for a later enhancer or the
/// handler to read once activation succeeds — an authenticating guard puts the
/// principal there rather than making the handler re-derive it:
///
/// ```ignore
/// async fn can_activate(&self, ctx: &mut HttpContext) -> bool {
///     let Some(user) = self.authenticate(ctx.request()) else { return false };
///     ctx.extensions().insert(user);
///     true
/// }
/// ```
///
/// How the handler reads it depends on the transport: an HTTP handler takes
/// `Extensions` as a parameter, a WebSocket handler reads `client.extensions`,
/// and an RPC or gRPC handler already holds the context.
#[async_trait]
pub trait Guard<C: ?Sized + HandlerContext>: Send + Sync {
    async fn can_activate(&self, context: &mut C) -> bool;
}
