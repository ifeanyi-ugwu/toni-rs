//! The closure traits behind every factory, readiness check, hook closure and enhancer closure:
//! parameters are sites, read through the same `Site::read` a field is, and the output type of
//! the returned future is what the closure produces (§4, §5).

use std::future::Future;
use std::sync::Arc;

use crate::binding::{CheckFn, ErasedCtor, check_fn, ctor_fn, instance_of};
use crate::construct::ConstructError;
use crate::error::LookupError;
use crate::hooks::{HookCx, HookFn, hook_fn};
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
    ///
    /// Parameters are read one after another, in the order written, and the first failure ends
    /// the call: no read starts after one has failed, and none is left running when the call's
    /// future is dropped.
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

            #[allow(non_snake_case, unused_variables)]
            fn call<'a>(&'a self, r: &'a Resolver<'a>) -> BoxFuture<'a, Result<Self::Output, LookupError>> {
                Box::pin(async move {
                    $( let $A = <$A as Site>::read(r).await?; )*
                    Ok::<_, LookupError>((self)($($A),*).await)
                })
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

            #[allow(non_snake_case, unused_variables)]
            fn call<'a>(&'a self, signal: Signal, r: &'a Resolver<'a>) -> BoxFuture<'a, Result<(), LookupError>> {
                Box::pin(async move {
                    $( let $A = <$A as Site>::read(r).await?; )*
                    (self)(signal, $($A),*).await;
                    Ok::<(), LookupError>(())
                })
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

// Each erased closure clones the `Arc` into its future rather than borrowing the factory: the
// slot's future lives for the resolver's lifetime, which a borrow of the closure's own state
// cannot promise.

/// A plain factory: the future's output is the instance.
pub(crate) fn erase_factory<Args, F>(f: F) -> ErasedCtor
where
    F: Factory<Args>,
    F::Output: Send + Sync + 'static,
{
    let f = Arc::new(f);
    ctor_fn(move |r| {
        let f = Arc::clone(&f);
        Box::pin(async move {
            let value = <F as Factory<Args>>::call(&f, r).await?;
            Ok::<_, ConstructError>(instance_of(Arc::new(value)))
        })
    })
}

/// A `try_` factory: `Err` becomes `ConstructError::Failed`, which the core redacts when it
/// stores it as `FailureReason::Errored`.
pub(crate) fn erase_try_factory<Args, F, T, E>(f: F) -> ErasedCtor
where
    F: Factory<Args, Output = Result<T, E>>,
    T: Send + Sync + 'static,
    E: Into<BoxError> + Send + 'static,
{
    let f = Arc::new(f);
    ctor_fn(move |r| {
        let f = Arc::clone(&f);
        Box::pin(async move {
            match <F as Factory<Args>>::call(&f, r).await? {
                Ok(value) => Ok::<_, ConstructError>(instance_of(Arc::new(value))),
                Err(e) => Err(ConstructError::failed(e)),
            }
        })
    })
}

/// A readiness closure: a failed site read is `ConstructError::Site`, the closure's own `Err`
/// is `ConstructError::Failed`.
pub(crate) fn erase_check<Args, F, E>(f: F) -> CheckFn
where
    F: Factory<Args, Output = Result<(), E>>,
    E: Into<BoxError> + Send + 'static,
{
    let f = Arc::new(f);
    check_fn(move |r| {
        let f = Arc::clone(&f);
        Box::pin(async move {
            match <F as Factory<Args>>::call(&f, r).await? {
                Ok(()) => Ok::<(), ConstructError>(()),
                Err(e) => Err(ConstructError::failed(e)),
            }
        })
    })
}

/// An `on_init` closure: its `Err` fails `connect` as `ConnectError::Hook { reason: Errored }`,
/// and so does a failed site read, carrying the `LookupError`.
pub(crate) fn erase_init_hook<Args, F, E>(f: F) -> HookFn
where
    F: Factory<Args, Output = Result<(), E>>,
    E: Into<BoxError> + Send + 'static,
{
    let f = Arc::new(f);
    hook_fn(move |cx| {
        let f = Arc::clone(&f);
        let resolver = cx.resolver;
        Box::pin(async move {
            let outcome: Result<(), BoxError> = match <F as Factory<Args>>::call(&f, resolver).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.into()),
                Err(lookup) => Err(BoxError::from(lookup)),
            };
            outcome
        })
    })
}

/// An `on_destroy` closure. It returns `()`, and its only `Err` is a failed site read: the hook
/// never ran.
pub(crate) fn erase_destroy_hook<Args, F>(f: F) -> HookFn
where
    F: Factory<Args, Output = ()>,
{
    let f = Arc::new(f);
    hook_fn(move |cx| {
        let f = Arc::clone(&f);
        let resolver = cx.resolver;
        Box::pin(async move { <F as Factory<Args>>::call(&f, resolver).await.map_err(BoxError::from) })
    })
}

/// A `before_shutdown` or `on_shutdown` closure, handed the winning trigger's signal. As with a
/// destroy hook, its only `Err` is a hook that never ran.
pub(crate) fn erase_signalled_hook<Args, F>(f: F) -> HookFn
where
    F: ShutdownFactory<Args>,
{
    let f = Arc::new(f);
    hook_fn(move |cx| {
        let f = Arc::clone(&f);
        let HookCx { resolver, signal, .. } = cx;
        Box::pin(async move {
            let Some(signal) = signal else {
                return Err(BoxError::from("a shutdown hook closure did not run: it was handed no shutdown signal"));
            };
            <F as ShutdownFactory<Args>>::call(&f, signal.clone(), resolver).await.map_err(BoxError::from)
        })
    })
}
