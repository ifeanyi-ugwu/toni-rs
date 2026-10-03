use std::panic::Location;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::binding::factory::Factory;
use crate::binding::{Coercion, coercion};
use crate::dependency::Dependencies;
use crate::error::LookupError;
use crate::key::Key;
use crate::resolver::Resolver;
use crate::scope::{Auto, ExplicitScope, Scope, ScopeKind};
use crate::timer::BoxFuture;
use crate::transport::controller::{ClosureDep, EnhancerDep};
use crate::transport::{AnyErrorHandler, AnyGuard, AnyInterceptor, ErrorHandler, Guard, Interceptor, Transport};

/// The enhancers one tier declares for one handler: the controller's, or the method's. Each is
/// declared by type (resolved from the container with the controller module's visibility), by
/// value (built once and shared), or by closure, whose parameters are injection points.
///
/// A closure's scope follows the rule a type's does (§3.3). The `_with` forms declare `Auto`:
/// built once, on first use, when nothing the closure's parameters read needs an execution, and
/// once per execution otherwise. The `_with_in` forms name the scope instead. Inference reads the
/// parameters, not the body, so a closure that creates per-call state while reading nothing per
/// call, such as a timer started when it is built, is declared `_with_in::<PerExecution>`.
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

    /// A guard already built and shared with other handlers: a controller-level `value = expr`,
    /// which `Controller::mount` builds once into an `Arc` and hands to every handler's mount
    /// function (transports DESIGN §2.1, X2). Each handler unsizes its own clone of the `Arc`
    /// into its transport's role, so the role is checked per handler.
    pub fn guard_arc<G: Guard<T>>(&mut self, guard: Arc<G>) -> &mut Self {
        self.guards.push(Decl::Value(widen_guard::<T, G>(guard)));
        self
    }

    /// A guard by closure, with `Auto` scope: built once if nothing it reads needs an execution,
    /// per execution if something does.
    #[track_caller]
    pub fn guard_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: Guard<T>,
    {
        self.guards.push(Decl::by_closure::<Auto, Args, F>(build, widen_guard::<T, F::Output>, Location::caller()));
        self
    }

    /// A guard by closure, in the scope `S`: `Singleton`, built once and refused at `wire()` if
    /// it reads per-execution data; `PerExecution`; or `Transient`, built each time the pipeline
    /// obtains it.
    #[track_caller]
    pub fn guard_with_in<S, Args, F>(&mut self, build: F) -> &mut Self
    where
        S: ExplicitScope,
        F: Factory<Args>,
        F::Output: Guard<T>,
    {
        self.guards.push(Decl::by_closure::<S, Args, F>(build, widen_guard::<T, F::Output>, Location::caller()));
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

    /// [`guard_arc`](Self::guard_arc) for an interceptor.
    pub fn interceptor_arc<I: Interceptor<T>>(&mut self, interceptor: Arc<I>) -> &mut Self {
        self.interceptors.push(Decl::Value(widen_interceptor::<T, I>(interceptor)));
        self
    }

    /// [`guard_with`](Self::guard_with) for an interceptor.
    #[track_caller]
    pub fn interceptor_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: Interceptor<T>,
    {
        let decl = Decl::by_closure::<Auto, Args, F>(build, widen_interceptor::<T, F::Output>, Location::caller());
        self.interceptors.push(decl);
        self
    }

    /// [`guard_with_in`](Self::guard_with_in) for an interceptor.
    #[track_caller]
    pub fn interceptor_with_in<S, Args, F>(&mut self, build: F) -> &mut Self
    where
        S: ExplicitScope,
        F: Factory<Args>,
        F::Output: Interceptor<T>,
    {
        let decl = Decl::by_closure::<S, Args, F>(build, widen_interceptor::<T, F::Output>, Location::caller());
        self.interceptors.push(decl);
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

    /// [`guard_arc`](Self::guard_arc) for an error handler.
    pub fn error_handler_arc<E: ErrorHandler<T>>(&mut self, handler: Arc<E>) -> &mut Self {
        self.error_handlers.push(Decl::Value(widen_error_handler::<T, E>(handler)));
        self
    }

    /// [`guard_with`](Self::guard_with) for an error handler.
    #[track_caller]
    pub fn error_handler_with<Args, F>(&mut self, build: F) -> &mut Self
    where
        F: Factory<Args>,
        F::Output: ErrorHandler<T>,
    {
        let decl = Decl::by_closure::<Auto, Args, F>(build, widen_error_handler::<T, F::Output>, Location::caller());
        self.error_handlers.push(decl);
        self
    }

    /// [`guard_with_in`](Self::guard_with_in) for an error handler.
    #[track_caller]
    pub fn error_handler_with_in<S, Args, F>(&mut self, build: F) -> &mut Self
    where
        S: ExplicitScope,
        F: Factory<Args>,
        F::Output: ErrorHandler<T>,
    {
        let decl = Decl::by_closure::<S, Args, F>(build, widen_error_handler::<T, F::Output>, Location::caller());
        self.error_handlers.push(decl);
        self
    }

    /// What the wiring pass resolves for this tier: guards, then interceptors, then error
    /// handlers, each in the order written. A by-value declaration depends on nothing. `tier`
    /// names the tier in reports.
    pub(crate) fn deps(&self, tier: &'static str, out: &mut Vec<EnhancerDep>) {
        out.extend(deps_of(&self.guards, "guard", tier));
        out.extend(deps_of(&self.interceptors, "interceptor", tier));
        out.extend(deps_of(&self.error_handlers, "error handler", tier));
    }
}

fn deps_of<'d, R: ?Sized + Send + Sync + 'static>(
    decls: &'d [Decl<R>],
    role: &'static str,
    tier: &'static str,
) -> impl Iterator<Item = EnhancerDep> + 'd {
    decls.iter().enumerate().filter_map(move |(index, decl)| decl.dep(role, tier, index + 1))
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

    fn by_closure<S, Args, F>(build: F, widen: fn(Arc<F::Output>) -> Arc<R>, location: &'static Location<'static>) -> Self
    where
        S: Scope,
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
        Decl::Closure(Arc::new(ClosureDecl {
            id: ClosureId::next(),
            scope: S::KIND,
            dependencies: Arc::new(dependencies),
            build,
            location,
        }))
    }

    fn dep(&self, role: &'static str, tier: &'static str, position: usize) -> Option<EnhancerDep> {
        match self {
            Decl::Type { key, .. } => Some(EnhancerDep::Type(*key)),
            Decl::Value(_) => None,
            Decl::Closure(c) => Some(EnhancerDep::Closure(ClosureDep {
                id: c.id,
                scope: c.scope,
                dependencies: Arc::clone(&c.dependencies),
                role,
                tier,
                position,
                location: c.location,
            })),
        }
    }
}

pub(crate) struct ClosureDecl<R: ?Sized> {
    pub(crate) id: ClosureId,
    /// As declared; `Graph::closures` holds the scope the wiring pass decided from it.
    pub(crate) scope: ScopeKind,
    /// Shared with the handler record the wiring pass reads.
    pub(crate) dependencies: Arc<Dependencies>,
    pub(crate) build: Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Arc<R>, LookupError>> + Send + Sync>,
    pub(crate) location: &'static Location<'static>,
}

/// One closure declaration, unique in the process; a clone of the declaration keeps it. The
/// decided scope, the app's once-built instances and an execution's cache are keyed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClosureId(u64);

impl ClosureId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        ClosureId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
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
