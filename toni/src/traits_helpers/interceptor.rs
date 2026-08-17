use async_trait::async_trait;

use crate::context::HandlerContext;

/// The next step in the interceptor chain.
///
/// `run` consumes `Box<Self>` so it can only be called once — the type system
/// prevents an interceptor from invoking the downstream handler twice.
#[async_trait]
pub trait InterceptorNext<C: ?Sized + HandlerContext, R>: Send {
    async fn run(self: Box<Self>, context: &C) -> R;
}

/// An interceptor wraps the handler with code that runs before and/or after.
///
/// Skip the handler entirely by returning without calling
/// `next.run(context).await` — useful for caching, circuit breakers, and
/// short-circuit responses. Whatever is returned is the answer, whether it came
/// from downstream or from the interceptor itself.
///
/// `R` is what the transport answers with: `HttpResponse` on HTTP,
/// `Result<Option<RpcData>, RpcError>` on RPC, `Result<Option<WsMessage>,
/// WsError>` on WebSocket, `Result<(), GrpcStatus>` on gRPC.
#[async_trait]
pub trait Interceptor<C: ?Sized + HandlerContext, R>: Send + Sync {
    async fn intercept(&self, context: &C, next: Box<dyn InterceptorNext<C, R>>) -> R;
}
