//! How a declaration names an enhancer, before any of it is resolved.
//!
//! `#[use_guards(...)]` and its siblings name an enhancer three ways, and which way alone decides
//! the lifecycle:
//!
//! | written | is | lifecycle | DI |
//! | --- | --- | --- | --- |
//! | a type — `AuthGuard`, `guards::AuthGuard`, a marker `Auth` | the binding under it | whatever its binding's scope says | yes |
//! | a value expression — `RateLimiter::new(100)`, `Simple {}`; `value LIMIT`, `value &SHARED` | a value | built once at startup, shared by every execution | no |
//! | a closure — `\|ctx\| Audit::for_call(ctx)` | a constructor | built per execution, at this site | no |
//!
//! A bare path names a type, never a const, so a value held in a const takes `value`, and a value
//! held in a `static` is taken by reference, a reference to an enhancer being that enhancer. The
//! `{}` on a unit struct is what tells a value from a type. Every entry keeps its place: the order
//! a declaration writes is the order the resolver keeps, whichever ways it mixes.
//!
//! An error handler takes the first two only. It is built once and shared, so the closure form
//! has nothing to build per execution, and `#[use_error_handlers(|ctx| ..)]` is refused where it
//! is written.

use std::sync::Arc;

use crate::dispatch::transport::{
    Answer, ConstructedGuard, ConstructedInterceptor, GuardFactory, InterceptorFactory, Transport,
};
use crate::enhancer::{ErrorHandler, Guard, Interceptor};

/// One entry of `#[use_guards(...)]`.
#[non_exhaustive]
pub enum GuardDeclaration<T: Transport> {
    /// `#[use_guards(AuthGuard)]` or `#[use_guards(Auth)]` — the binding under a type, resolved
    /// against the role registry at `create`. A key that resolves against nothing fails `create`.
    Token(String),
    /// `#[use_guards(RoleGuard::new("admin"))]` — one value, shared by every execution.
    Value(Arc<dyn Guard<T::Context>>),
    /// `#[use_guards(|ctx| Audit::for_call(ctx))]` — built inside each execution.
    Constructor(GuardConstructor<T>),
}

impl<T: Transport> GuardDeclaration<T> {
    /// The value spelling. `guard` is built here and shared from then on.
    pub fn value<G: Guard<T::Context> + 'static>(guard: G) -> Self {
        Self::Value(Arc::new(guard))
    }

    /// The closure spelling. `build` runs once per execution, given that execution's context.
    pub fn constructor<F, G>(build: F) -> Self
    where
        F: Fn(&T::Context) -> G + Send + Sync + 'static,
        G: Guard<T::Context> + 'static,
    {
        Self::Constructor(GuardConstructor(Arc::new(ConstructedGuard(build))))
    }
}

/// Builds a guard inside each execution. Made by [`GuardDeclaration::constructor`].
pub struct GuardConstructor<T: Transport>(pub(crate) Arc<dyn GuardFactory<T>>);

/// One entry of `#[use_interceptors(...)]`. The spellings are [`GuardDeclaration`]'s.
#[non_exhaustive]
pub enum InterceptorDeclaration<T: Transport> {
    /// A DI token, resolved against the role registry at `create`.
    Token(String),
    /// One value, shared by every execution.
    Value(Arc<dyn Interceptor<T::Context, Answer<T>>>),
    /// Built inside each execution.
    Constructor(InterceptorConstructor<T>),
}

impl<T: Transport> InterceptorDeclaration<T> {
    /// The value spelling. `interceptor` is built here and shared from then on.
    pub fn value<I: Interceptor<T::Context, Answer<T>> + 'static>(interceptor: I) -> Self {
        Self::Value(Arc::new(interceptor))
    }

    /// The closure spelling. `build` runs once per execution, given that execution's context.
    pub fn constructor<F, I>(build: F) -> Self
    where
        F: Fn(&T::Context) -> I + Send + Sync + 'static,
        I: Interceptor<T::Context, Answer<T>> + 'static,
    {
        Self::Constructor(InterceptorConstructor(Arc::new(ConstructedInterceptor(
            build,
        ))))
    }
}

/// Builds an interceptor inside each execution. Made by [`InterceptorDeclaration::constructor`].
pub struct InterceptorConstructor<T: Transport>(pub(crate) Arc<dyn InterceptorFactory<T>>);

/// One entry of `#[use_error_handlers(...)]`.
///
/// A type or a value, never a closure: an error handler is built once and shared, so there is no
/// per-execution arm for a closure to reach.
#[non_exhaustive]
pub enum ErrorHandlerDeclaration<T: Transport> {
    /// A DI token, resolved against the role registry at `create`.
    Token(String),
    /// One value, shared by every execution.
    Value(Arc<dyn ErrorHandler<T::Context, Answer<T>>>),
}

impl<T: Transport> ErrorHandlerDeclaration<T> {
    /// The value spelling. `handler` is built here and shared from then on.
    pub fn value<H: ErrorHandler<T::Context, Answer<T>> + 'static>(handler: H) -> Self {
        Self::Value(Arc::new(handler))
    }
}
