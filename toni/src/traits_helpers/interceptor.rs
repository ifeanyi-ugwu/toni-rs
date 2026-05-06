use async_trait::async_trait;

use crate::context::HandlerContext;

/// The next step in the interceptor chain.
///
/// `run` consumes `Box<Self>` so it can only be called once — the type system
/// prevents an interceptor from invoking the downstream handler twice.
#[async_trait]
pub trait InterceptorNext<C: ?Sized + HandlerContext>: Send {
    async fn run(self: Box<Self>, context: &mut C);
}

/// An interceptor wraps the handler with code that runs before and/or after.
///
/// Skip the handler entirely by not calling `next.run(context).await` —
/// useful for caching, circuit breakers, and short-circuit responses.
#[async_trait]
pub trait Interceptor<C: ?Sized + HandlerContext>: Send + Sync {
    async fn intercept(&self, context: &mut C, next: Box<dyn InterceptorNext<C>>);
}
