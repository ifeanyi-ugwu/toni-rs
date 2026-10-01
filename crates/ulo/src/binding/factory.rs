//! The closure traits behind every factory, readiness check, hook closure and enhancer closure:
//! parameters are sites, read through the same `Site::read` a field is, and the output type of
//! the returned future is what the closure produces (§4, §5).

use std::future::Future;

use crate::binding::{CheckFn, ErasedCtor};
use crate::error::LookupError;
use crate::hooks::HookFn;
use crate::resolver::Resolver;
use crate::signal::Signal;
use crate::site::{Site, Sites};
use crate::timer::{BoxError, BoxFuture};

/// A closure whose parameters are sites and which returns a future: `|cfg: Dep<DbConfig>| async
/// move { .. }`. Implemented for closures of zero to twelve parameters.
///
/// Every parameter needs its type written; an unannotated closure parameter has no site type to
/// read. The output of the future is the binding's key for `singleton`, `execution` and
/// `transient`, and its `Ok` type for the `try_` forms.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a factory over sites",
    label = "expected a closure returning a future, every parameter an injection site",
    note = "annotate each parameter with its site type, as in `|cfg: Dep<DbConfig>| async move {{ .. }}`"
)]
pub trait Factory<Args>: Send + Sync + 'static {
    type Output: Send + 'static;

    /// The parameters' sites, in order.
    fn sites(s: &mut Sites);

    /// Read every parameter, then call the closure and await its future. A failed read is the
    /// site's own error.
    fn call<'a>(&'a self, r: &'a Resolver<'a>) -> BoxFuture<'a, Result<Self::Output, LookupError>>;
}

/// A closure `before_shutdown` or `on_shutdown` takes on a binding handle or a `ModuleDef`: the
/// shutdown's [`Signal`] first, then any number of sites, returning a future of `()`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a shutdown hook closure",
    label = "expected a closure taking the `Signal` first, then injection sites, returning a future of `()`",
    note = "write `|signal: Signal, pool: Dep<PgPool>| async move {{ .. }}`"
)]
pub trait ShutdownFactory<Args>: Send + Sync + 'static {
    fn sites(s: &mut Sites);

    fn call<'a>(&'a self, signal: Signal, r: &'a Resolver<'a>) -> BoxFuture<'a, Result<(), LookupError>>;
}

macro_rules! factory_impls {
    ($($A:ident),*) => {
        impl<F, Fut, $($A),*> Factory<($($A,)*)> for F
        where
            F: Fn($($A),*) -> Fut + Send + Sync + 'static,
            Fut: Future + Send + 'static,
            Fut::Output: Send + 'static,
            $($A: Site,)*
        {
            type Output = Fut::Output;

            #[allow(unused_variables)]
            fn sites(s: &mut Sites) {
                $( s.site::<$A>(); )*
            }

            fn call<'a>(&'a self, r: &'a Resolver<'a>) -> BoxFuture<'a, Result<Self::Output, LookupError>> {
                todo!()
            }
        }

        impl<F, Fut, $($A),*> ShutdownFactory<($($A,)*)> for F
        where
            F: Fn(Signal, $($A),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = ()> + Send + 'static,
            $($A: Site,)*
        {
            #[allow(unused_variables)]
            fn sites(s: &mut Sites) {
                $( s.site::<$A>(); )*
            }

            fn call<'a>(&'a self, signal: Signal, r: &'a Resolver<'a>) -> BoxFuture<'a, Result<(), LookupError>> {
                todo!()
            }
        }
    };
}

factory_impls!();
factory_impls!(A1);
factory_impls!(A1, A2);
factory_impls!(A1, A2, A3);
factory_impls!(A1, A2, A3, A4);
factory_impls!(A1, A2, A3, A4, A5);
factory_impls!(A1, A2, A3, A4, A5, A6);
factory_impls!(A1, A2, A3, A4, A5, A6, A7);
factory_impls!(A1, A2, A3, A4, A5, A6, A7, A8);
factory_impls!(A1, A2, A3, A4, A5, A6, A7, A8, A9);
factory_impls!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
factory_impls!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
factory_impls!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12);

/// A plain factory: the future's output is the instance.
pub(crate) fn erase_factory<Args, F>(f: F) -> ErasedCtor
where
    F: Factory<Args>,
    F::Output: Send + Sync + 'static,
{
    todo!()
}

/// A `try_` factory: `Err` becomes `ConstructError::Failed`, which the core redacts when it
/// stores it as `FailureReason::Errored`.
pub(crate) fn erase_try_factory<Args, F, T, E>(f: F) -> ErasedCtor
where
    F: Factory<Args, Output = Result<T, E>>,
    T: Send + Sync + 'static,
    E: Into<BoxError> + Send + 'static,
{
    todo!()
}

pub(crate) fn erase_check<Args, F, E>(f: F) -> CheckFn
where
    F: Factory<Args, Output = Result<(), E>>,
    E: Into<BoxError> + Send + 'static,
{
    todo!()
}

/// An `on_init` closure: its `Err` fails `connect` as `ConnectError::Hook { reason: Errored }`.
pub(crate) fn erase_init_hook<Args, F, E>(f: F) -> HookFn
where
    F: Factory<Args, Output = Result<(), E>>,
    E: Into<BoxError> + Send + 'static,
{
    todo!()
}

pub(crate) fn erase_destroy_hook<Args, F>(f: F) -> HookFn
where
    F: Factory<Args, Output = ()>,
{
    todo!()
}

/// A `before_shutdown` or `on_shutdown` closure, handed the winning trigger's signal.
pub(crate) fn erase_signalled_hook<Args, F>(f: F) -> HookFn
where
    F: ShutdownFactory<Args>,
{
    todo!()
}
