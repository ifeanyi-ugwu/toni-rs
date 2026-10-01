//! The traits a transport implements against, and the enhancer roles generic over a transport
//! (§3.7, §7).
//!
//! `Guard<Http>` and `Guard<Rpc>` are different traits, so an HTTP guard and an RPC guard are
//! different roles. A role comes only from a trait implementation; there are no marker
//! attributes.

pub(crate) mod controller;
pub(crate) mod enhancer;
pub(crate) mod next;
pub(crate) mod pipeline;
pub(crate) mod server;

use std::any::type_name;
use std::future::Future;

use crate::timer::{BoxError, BoxFuture};
use crate::transport::next::Next;

/// Implemented by marker types in transport crates: `ulo_http::Http`, `ulo_rpc::Rpc`, ...
///
/// `Cx` wraps an `ExecutionRef` together with the call's wire data, and every clone is the same
/// call. Everything a guard or interceptor writes goes through a shared reference and interior
/// mutability. A reply that streams carries a clone of `Cx`, which keeps its per-execution
/// instances and its cancellation signal alive past `intercept`'s return. A request body stream
/// lives on `Cx` behind a take-once slot, not as an execution input: inputs are `Sync` and a
/// body stream is read once.
pub trait Transport: 'static {
    /// Per-call context: a cheap-clone handle to the execution.
    type Cx: Clone + Send + Sync;
    type Reply: Send;
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a guard for `{T}`",
    label = "a guard for `{T}` is needed here",
    note = "implement `Guard<{T}>` for `{Self}`"
)]
pub trait Guard<T: Transport>: Send + Sync + 'static {
    fn can_activate(&self, cx: &T::Cx) -> impl Future<Output = Result<bool, BoxError>> + Send;
}

#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an interceptor for `{T}`",
    label = "an interceptor for `{T}` is needed here",
    note = "implement `Interceptor<{T}>` for `{Self}`"
)]
pub trait Interceptor<T: Transport>: Send + Sync + 'static {
    fn intercept(&self, cx: &T::Cx, next: Next<'_, T>) -> impl Future<Output = Result<T::Reply, BoxError>> + Send;
}

/// Errors reach the error handlers method level first, then controller, then global. `Ok`
/// claims the error with a reply; `Err` hands the next handler the error it returns, the same
/// one or a reshaped one. A guard's refusal arrives as `GuardRejected`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an error handler for `{T}`",
    label = "an error handler for `{T}` is needed here",
    note = "implement `ErrorHandler<{T}>` for `{Self}`"
)]
pub trait ErrorHandler<T: Transport>: Send + Sync + 'static {
    fn handle(&self, err: BoxError, cx: &T::Cx) -> impl Future<Output = Result<T::Reply, BoxError>> + Send;
}

/// The dyn-compatible twin of [`Guard`], implemented for every guard; its trait object is the
/// role key [`AnyGuard`].
pub trait ErasedGuard<T: Transport>: Send + Sync + 'static {
    fn can_activate<'a>(&'a self, cx: &'a T::Cx) -> BoxFuture<'a, Result<bool, BoxError>>;

    /// The guard's type name, which a refusal reports as `GuardRejected::guard`.
    fn name(&self) -> &'static str;
}

impl<T: Transport, G: Guard<T>> ErasedGuard<T> for G {
    fn can_activate<'a>(&'a self, cx: &'a T::Cx) -> BoxFuture<'a, Result<bool, BoxError>> {
        Box::pin(Guard::can_activate(self, cx))
    }

    fn name(&self) -> &'static str {
        type_name::<G>()
    }
}

/// The dyn-compatible twin of [`Interceptor`]; its trait object is the role key [`AnyInterceptor`].
pub trait ErasedInterceptor<T: Transport>: Send + Sync + 'static {
    fn intercept<'a>(&'a self, cx: &'a T::Cx, next: Next<'a, T>) -> BoxFuture<'a, Result<T::Reply, BoxError>>;
}

impl<T: Transport, I: Interceptor<T>> ErasedInterceptor<T> for I {
    fn intercept<'a>(&'a self, cx: &'a T::Cx, next: Next<'a, T>) -> BoxFuture<'a, Result<T::Reply, BoxError>> {
        Box::pin(Interceptor::intercept(self, cx, next))
    }
}

/// The dyn-compatible twin of [`ErrorHandler`]; its trait object is the role key [`AnyErrorHandler`].
pub trait ErasedErrorHandler<T: Transport>: Send + Sync + 'static {
    fn handle<'a>(&'a self, err: BoxError, cx: &'a T::Cx) -> BoxFuture<'a, Result<T::Reply, BoxError>>;
}

impl<T: Transport, E: ErrorHandler<T>> ErasedErrorHandler<T> for E {
    fn handle<'a>(&'a self, err: BoxError, cx: &'a T::Cx) -> BoxFuture<'a, Result<T::Reply, BoxError>> {
        Box::pin(ErrorHandler::handle(self, err, cx))
    }
}

/// The role key for global guards: `m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a)`.
pub type AnyGuard<T> = dyn ErasedGuard<T>;
pub type AnyInterceptor<T> = dyn ErasedInterceptor<T>;
pub type AnyErrorHandler<T> = dyn ErasedErrorHandler<T>;
