use std::sync::Arc;

use crate::binding::factory::Factory;
use crate::binding::{Coercion, coercion};
use crate::dependency::Dependencies;
use crate::error::LookupError;
use crate::key::Key;
use crate::resolver::Resolver;
use crate::timer::BoxFuture;
use crate::transport::controller::EnhancerDep;
use crate::transport::{AnyErrorHandler, AnyGuard, AnyInterceptor, ErrorHandler, Guard, Interceptor, Transport};

/// The enhancers one tier declares for one handler: the controller's, or the method's. Each is
/// declared by type (resolved from the container with the controller module's visibility), by
/// value (built once and shared), or by closure (built per execution, its parameters injection
/// points).
///
/// Within a tier, declarations keep the order written. The stack runs global, then controller,
/// then method; error handlers run in the reverse.
pub struct EnhancerSpec<T: Transport> {
    pub(crate) guards: Vec<Decl<AnyGuard<T>>>,
    pub(crate) interceptors: Vec<Decl<AnyInterceptor<T>>>,
    pub(crate) error_handlers: Vec<Decl<AnyErrorHandler<T>>>,
}

impl<T: Transport> EnhancerSpec<T> {
    pub fn new() -> Self {
        EnhancerSpec { guards: Vec::new(), interceptors: Vec::new(), error_handlers: Vec::new() }
    }

    /// A guard by type: the binding of `G`, which must be visible from the controller's module.
    pub fn guard<G: Guard<T>>(&mut self) -> &mut Self {
        self.guards.push(Decl::by_type(widen_guard::<T, G>));
        self
    }

    /// A guard by value, built once and shared by every call.
    pub fn guard_value<G: Guard<T>>(&mut self, guard: G) -> &mut Self {
        self.guards.push(Decl::by_value(guard, widen_guard::<T, G>));
        self
    }

    /// A guard by closure, built per execution from the closure's injection points.
    pub fn guard_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: Guard<T>,
    {
        self.guards.push(Decl::by_closure::<Args, F>(build, widen_guard::<T, F::Output>));
        self
    }

    pub fn interceptor<I: Interceptor<T>>(&mut self) -> &mut Self {
        self.interceptors.push(Decl::by_type(widen_interceptor::<T, I>));
        self
    }

    pub fn interceptor_value<I: Interceptor<T>>(&mut self, interceptor: I) -> &mut Self {
        self.interceptors.push(Decl::by_value(interceptor, widen_interceptor::<T, I>));
        self
    }

    pub fn interceptor_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: Interceptor<T>,
    {
        self.interceptors.push(Decl::by_closure::<Args, F>(build, widen_interceptor::<T, F::Output>));
        self
    }

    pub fn error_handler<E: ErrorHandler<T>>(&mut self) -> &mut Self {
        self.error_handlers.push(Decl::by_type(widen_error_handler::<T, E>));
        self
    }

    pub fn error_handler_value<E: ErrorHandler<T>>(&mut self, handler: E) -> &mut Self {
        self.error_handlers.push(Decl::by_value(handler, widen_error_handler::<T, E>));
        self
    }

    pub fn error_handler_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: ErrorHandler<T>,
    {
        self.error_handlers.push(Decl::by_closure::<Args, F>(build, widen_error_handler::<T, F::Output>));
        self
    }

    /// What the wiring pass resolves for this tier: guards, then interceptors, then error
    /// handlers, each in the order written. A by-value declaration depends on nothing.
    pub(crate) fn deps(&self, out: &mut Vec<EnhancerDep>) {
        out.extend(self.guards.iter().filter_map(Decl::dep));
        out.extend(self.interceptors.iter().filter_map(Decl::dep));
        out.extend(self.error_handlers.iter().filter_map(Decl::dep));
    }
}

impl<T: Transport> Default for EnhancerSpec<T> {
    fn default() -> Self {
        EnhancerSpec::new()
    }
}

impl<T: Transport> Clone for EnhancerSpec<T> {
    fn clone(&self) -> Self {
        EnhancerSpec {
            guards: self.guards.clone(),
            interceptors: self.interceptors.clone(),
            error_handlers: self.error_handlers.clone(),
        }
    }
}

/// One declaration of an enhancer whose role trait object is `R`.
pub(crate) enum Decl<R: ?Sized> {
    /// The binding under `key` (the enhancer's own type, unqualified), widened to `R`. The
    /// wiring pass resolves it against the controller module's visibility and marks the binding
    /// an enhancer for the `Auto` rule.
    Type { key: Key, coerce: Coercion },
    Value(Arc<R>),
    Closure(Arc<ClosureDecl<R>>),
}

impl<R: ?Sized> Clone for Decl<R> {
    fn clone(&self) -> Self {
        match self {
            Decl::Type { key, coerce } => Decl::Type { key: *key, coerce: Arc::clone(coerce) },
            Decl::Value(v) => Decl::Value(Arc::clone(v)),
            Decl::Closure(c) => Decl::Closure(Arc::clone(c)),
        }
    }
}

impl<R: ?Sized + Send + Sync + 'static> Decl<R> {
    fn by_type<X: Send + Sync + 'static>(widen: fn(Arc<X>) -> Arc<R>) -> Self {
        Decl::Type { key: Key::of::<X, ()>(), coerce: coercion(widen) }
    }

    fn by_value<X>(value: X, widen: fn(Arc<X>) -> Arc<R>) -> Self {
        Decl::Value(widen(Arc::new(value)))
    }

    fn by_closure<Args, F>(build: F, widen: fn(Arc<F::Output>) -> Arc<R>) -> Self
    where
        F: Factory<Args>,
    {
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        let factory = Arc::new(build);
        let build = erase_build::<R, _>(move |r| {
            let factory = Arc::clone(&factory);
            let fut: BoxFuture<'_, Result<Arc<R>, LookupError>> = Box::pin(async move {
                <F as Factory<Args>>::call(&*factory, r).await.map(|built| widen(Arc::new(built)))
            });
            fut
        });
        Decl::Closure(Arc::new(ClosureDecl { dependencies: Arc::new(dependencies), build }))
    }

    fn dep(&self) -> Option<EnhancerDep> {
        match self {
            Decl::Type { key, .. } => Some(EnhancerDep::Type(*key)),
            Decl::Value(_) => None,
            Decl::Closure(c) => Some(EnhancerDep::Closure(Arc::clone(&c.dependencies))),
        }
    }
}

pub(crate) struct ClosureDecl<R: ?Sized> {
    /// Checked by the wiring pass like a per-execution binding's dependencies; shared with the
    /// handler record the wiring pass reads.
    pub(crate) dependencies: Arc<Dependencies>,
    pub(crate) build: Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Arc<R>, LookupError>> + Send + Sync>,
}

/// Gives a closure the higher-ranked signature of `ClosureDecl::build`: a closure passed where
/// this bound is expected has its signature deduced from it, which one boxed in place does not.
fn erase_build<R, F>(f: F) -> Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Arc<R>, LookupError>> + Send + Sync>
where
    R: ?Sized + 'static,
    F: for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Arc<R>, LookupError>> + Send + Sync + 'static,
{
    Arc::new(f)
}

fn widen_guard<T: Transport, G: Guard<T>>(guard: Arc<G>) -> Arc<AnyGuard<T>> {
    guard
}

fn widen_interceptor<T: Transport, I: Interceptor<T>>(interceptor: Arc<I>) -> Arc<AnyInterceptor<T>> {
    interceptor
}

fn widen_error_handler<T: Transport, E: ErrorHandler<T>>(handler: Arc<E>) -> Arc<AnyErrorHandler<T>> {
    handler
}
