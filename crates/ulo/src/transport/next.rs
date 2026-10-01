use std::sync::Arc;

use crate::error::DispatchStage;
use crate::redact::SecretRegistry;
use crate::timer::{BoxError, BoxFuture};
use crate::transport::pipeline::caught;
use crate::transport::{AnyInterceptor, ErasedInterceptor, Transport};

/// The rest of the interceptor chain and the handler, handed to [`Interceptor::intercept`](crate::Interceptor).
///
/// [`run`](Next::run) consumes it, so the rest of the chain runs at most once; an interceptor
/// that returns without calling it answers the call itself. A panic further down the chain
/// comes back from `run` as an `Err` holding [`PanicRecovered`](crate::PanicRecovered).
pub struct Next<'a, T: Transport> {
    pub(crate) cx: &'a T::Cx,
    /// The interceptors after the current one, already built.
    pub(crate) rest: &'a [Arc<AnyInterceptor<T>>],
    pub(crate) handler: Box<dyn FnOnce(&'a T::Cx) -> BoxFuture<'a, Result<T::Reply, BoxError>> + Send + 'a>,
    /// Redacts the message of a panic caught in the chain.
    pub(crate) secrets: &'a SecretRegistry,
}

impl<'a, T: Transport> Next<'a, T> {
    /// Runs the next interceptor, or the handler after the last one, with the same call.
    pub fn run(self) -> BoxFuture<'a, Result<T::Reply, BoxError>> {
        let Next { cx, rest, handler, secrets } = self;
        // The call happens inside the caught future, so a panic before the callee's first await
        // is attributed to it as well.
        match rest.split_first() {
            Some((first, rest)) => {
                let next = Next { cx, rest, handler, secrets };
                Box::pin(caught(DispatchStage::Interceptor, secrets, async move {
                    <AnyInterceptor<T> as ErasedInterceptor<T>>::intercept(&**first, cx, next).await
                }))
            }
            None => Box::pin(caught(DispatchStage::Handler, secrets, async move { handler(cx).await })),
        }
    }
}
