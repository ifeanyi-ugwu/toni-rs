use std::sync::Arc;

use crate::binding::Coercion;
use crate::binding::factory::Factory;
use crate::error::LookupError;
use crate::key::Key;
use crate::resolver::Resolver;
use crate::site::Sites;
use crate::timer::BoxFuture;
use crate::transport::{AnyErrorHandler, AnyGuard, AnyInterceptor, ErrorHandler, Guard, Interceptor, Transport};

/// The enhancers one tier declares for one handler: the controller's, or the method's. Each is
/// declared by type (resolved from the container with the controller module's visibility), by
/// value (built once and shared), or by closure (built per execution, its parameters sites).
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
        todo!()
    }

    /// A guard by value, built once and shared by every call.
    pub fn guard_value<G: Guard<T>>(&mut self, guard: G) -> &mut Self {
        todo!()
    }

    /// A guard by closure, built per execution from the closure's sites.
    pub fn guard_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: Guard<T>,
    {
        todo!()
    }

    pub fn interceptor<I: Interceptor<T>>(&mut self) -> &mut Self {
        todo!()
    }

    pub fn interceptor_value<I: Interceptor<T>>(&mut self, interceptor: I) -> &mut Self {
        todo!()
    }

    pub fn interceptor_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: Interceptor<T>,
    {
        todo!()
    }

    pub fn error_handler<E: ErrorHandler<T>>(&mut self) -> &mut Self {
        todo!()
    }

    pub fn error_handler_value<E: ErrorHandler<T>>(&mut self, handler: E) -> &mut Self {
        todo!()
    }

    pub fn error_handler_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: ErrorHandler<T>,
    {
        todo!()
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

pub(crate) struct ClosureDecl<R: ?Sized> {
    /// Checked by the wiring pass like a per-execution binding's sites; shared with the
    /// handler record the wiring pass reads.
    pub(crate) sites: Arc<Sites>,
    pub(crate) build: Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Arc<R>, LookupError>> + Send + Sync>,
}
