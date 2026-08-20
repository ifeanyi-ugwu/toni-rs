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
/// async fn can_activate(&self, ctx: &HttpContext) -> bool {
///     let Some(user) = self.authenticate(ctx.request()) else { return false };
///     ctx.extensions().insert(user);
///     true
/// }
/// ```
///
/// An HTTP or WebSocket handler takes `Extensions` as a parameter, an RPC
/// handler already holds the context, and a gRPC handler takes it off the tonic
/// request with `Extensions::adopt(request.extensions())`.
///
/// On HTTP, [`Extension<T>`](crate::Extension) injects the same value into a
/// guard and into anything below the controller, so both ends declare it rather
/// than reaching for the bag by type.
#[async_trait]
pub trait Guard<C: ?Sized + HandlerContext>: Send + Sync {
    async fn can_activate(&self, context: &C) -> bool;
}
