use std::marker::PhantomData;
use std::sync::Arc;

use crate::binding::factory::Factory;
use crate::binding::handle::{Contribution, Handle, Open, Set};
use crate::construct::Construct;
use crate::module::def::ModuleNode;
use crate::scope::{PerExecution, Singleton, Transient};
use crate::timer::BoxError;

/// Contributions to the collection `U @ Q`, read as `Many<U, Q>` or, by a transport, through
/// `Resolver::entries` (§3.2, §7).
///
/// Each contribution is built as its own type and widened to `U` by the coercion closure,
/// written `|a| a`. Contributions are app-wide: they are not exports, a keyed module's keep their
/// key, and a binding under a role key such as `AnyGuard<Http>` registers that role.
///
/// ```ignore
/// m.contribute::<dyn Plugin>().provide::<MetricsPlugin>(|a| a);
/// m.contribute::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a);
/// m.contribute::<dyn HealthIndicator>()
///     .singleton(|pool: Dep<PgPool>| async move { PgHealth::new(pool) }, |a| a);
/// ```
pub struct Contribute<'m, U: ?Sized, Q = ()> {
    node: &'m mut ModuleNode,
    _u: PhantomData<fn() -> (PhantomData<U>, Q)>,
}

impl<'m, U: ?Sized + Send + Sync + 'static, Q: 'static> Contribute<'m, U, Q> {
    pub(crate) fn new(node: &'m mut ModuleNode) -> Self {
        Contribute { node, _u: PhantomData }
    }

    /// Contributes to `U @ Q2` instead of the unqualified collection.
    pub fn qualified<Q2: 'static>(self) -> Contribute<'m, U, Q2> {
        Contribute { node: self.node, _u: PhantomData }
    }

    /// A `Construct` type, with its declared scope and its trait hooks.
    #[track_caller]
    pub fn provide<T: Construct>(
        self,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, Contribution<T::Scope, Set>> {
        todo!()
    }

    /// A shared value, already built.
    #[track_caller]
    pub fn value(self, value: Arc<U>) -> Handle<'m, U, Contribution<Singleton, Set>> {
        todo!()
    }

    #[track_caller]
    pub fn singleton<Args, F>(
        self,
        factory: F,
        coerce: impl Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, F::Output, Contribution<Singleton, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn try_singleton<Args, F, T, E>(
        self,
        factory: F,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, Contribution<Singleton, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn execution<Args, F>(
        self,
        factory: F,
        coerce: impl Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, F::Output, Contribution<PerExecution, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn try_execution<Args, F, T, E>(
        self,
        factory: F,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, Contribution<PerExecution, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn transient<Args, F>(
        self,
        factory: F,
        coerce: impl Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, F::Output, Contribution<Transient, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn try_transient<Args, F, T, E>(
        self,
        factory: F,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, Contribution<Transient, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
    }
}
