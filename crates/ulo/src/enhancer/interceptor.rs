use async_trait::async_trait;

use crate::context::ExecutionContext;

/// The next step in the interceptor chain.
///
/// `run` consumes `Box<Self>` so it can only be called once — the type system
/// prevents an interceptor from invoking the downstream handler twice.
#[async_trait]
pub trait InterceptorNext<C: ?Sized + ExecutionContext, R>: Send {
    async fn run(self: Box<Self>, context: &C) -> R;
}

/// An interceptor wraps the handler with code that runs before and/or after.
///
/// Skip the handler entirely by returning without calling
/// `next.run(context).await` — useful for caching, circuit breakers, and
/// short-circuit responses. Whatever is returned is the answer, whether it came
/// from downstream or from the interceptor itself.
///
/// `R` is what the transport answers with: `HttpHandlerResult`,
/// `RpcHandlerResult`, `WsHandlerResult` or `GrpcHandlerResult`, each a
/// `Result` of the transport's reply and its error. An interceptor answers the
/// call, or fails it, by returning.
#[async_trait]
pub trait Interceptor<C: ?Sized + ExecutionContext, R>: Send + Sync {
    async fn intercept(&self, context: &C, next: Box<dyn InterceptorNext<C, R>>) -> R;
}

/// An interceptor held in a `static` is shared by reference, written
/// `#[use_interceptors(value &TIMING)]`.
#[diagnostic::do_not_recommend]
#[async_trait]
impl<'i, C, R, I> Interceptor<C, R> for &'i I
where
    C: ?Sized + ExecutionContext,
    R: Send + 'static,
    I: Interceptor<C, R> + ?Sized,
{
    async fn intercept(&self, context: &C, next: Box<dyn InterceptorNext<C, R>>) -> R {
        (**self).intercept(context, next).await
    }
}
