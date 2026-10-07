//! A guard, an interceptor and an error handler for every transport, which the handlers name by
//! type, by value and by closure.

use ulo::{BoxError, ErrorHandler, Guard, Interceptor, Next, Transport};

/// Admits every call.
pub struct Allow;

impl<T: Transport> Guard<T> for Allow {
    async fn can_activate(&self, _cx: &T::Cx) -> Result<bool, BoxError> {
        Ok(true)
    }
}

/// Hands every call on.
pub struct Pass;

impl<T: Transport> Interceptor<T> for Pass {
    async fn intercept(&self, _cx: &T::Cx, next: Next<'_, T>) -> Result<T::Reply, BoxError> {
        next.run().await
    }
}

/// Hands every error on.
pub struct Relay;

impl<T: Transport> ErrorHandler<T> for Relay {
    async fn handle(&self, err: BoxError, _cx: &T::Cx) -> Result<T::Reply, BoxError> {
        Err(err)
    }
}

/// A metadata value for `#[meta]`.
#[derive(Clone, Debug)]
pub struct Tag(pub &'static str);
