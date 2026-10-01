use std::sync::Arc;

use crate::timer::{BoxError, BoxFuture};
use crate::transport::{AnyInterceptor, ErasedInterceptor, Transport};

/// The rest of the interceptor chain and the handler, handed to [`Interceptor::intercept`](crate::Interceptor).
///
/// [`run`](Next::run) consumes it, so the rest of the chain runs at most once; an interceptor
/// that returns without calling it answers the call itself.
pub struct Next<'a, T: Transport> {
    pub(crate) cx: &'a T::Cx,
    /// The interceptors after the current one, already built.
    pub(crate) rest: &'a [Arc<AnyInterceptor<T>>],
    pub(crate) handler: Box<dyn FnOnce(&'a T::Cx) -> BoxFuture<'a, Result<T::Reply, BoxError>> + Send + 'a>,
}

impl<'a, T: Transport> Next<'a, T> {
    /// Runs the next interceptor, or the handler after the last one, with the same call.
    pub fn run(self) -> BoxFuture<'a, Result<T::Reply, BoxError>> {
        let Next { cx, rest, handler } = self;
        match rest.split_first() {
            Some((first, rest)) => <AnyInterceptor<T> as ErasedInterceptor<T>>::intercept(&**first, cx, Next { cx, rest, handler }),
            None => handler(cx),
        }
    }
}
