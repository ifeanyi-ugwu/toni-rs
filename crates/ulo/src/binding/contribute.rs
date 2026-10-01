use std::any::type_name;
use std::marker::PhantomData;
use std::panic::Location;
use std::sync::Arc;

use crate::binding::factory::{Factory, erase_factory, erase_try_factory};
use crate::binding::handle::{Contribution, Handle, Open, Set};
use crate::binding::{BindingRecord, Coercion, ErasedCtor, Qualifier, Recipe, coercion, erase_construct, instance_of};
use crate::construct::Construct;
use crate::dependency::Dependencies;
use crate::hooks::erase_trait_hooks;
use crate::key::{BindingKind, Key};
use crate::module::def::ModuleNode;
use crate::scope::{PerExecution, Scope, ScopeKind, Singleton, Transient};
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

    /// The record of one contribution to `U @ Q`, before what a particular recipe adds.
    fn record(
        built: &'static str,
        scope: ScopeKind,
        recipe: Recipe,
        dependencies: Dependencies,
        into_primary: Coercion,
        location: &'static Location<'static>,
    ) -> BindingRecord {
        let mut record =
            BindingRecord::new(Key::of::<U, ()>(), built, BindingKind::Collection, scope, recipe, dependencies, location);
        record.qualifier = Qualifier::of::<Q>();
        record.into_primary = Some(into_primary);
        record
    }

    fn push<T: ?Sized, K>(self, record: BindingRecord) -> Handle<'m, T, K> {
        let node = self.node;
        let index = node.bindings.len();
        node.bindings.push(record);
        Handle::new(&mut node.bindings[index])
    }

    /// A factory contribution: a plain factory runs no trait hooks.
    fn push_factory<T: Send + Sync + 'static, S: Scope, Args, F: Factory<Args>, K>(
        self,
        ctor: ErasedCtor,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
        location: &'static Location<'static>,
    ) -> Handle<'m, T, K> {
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        let into_primary = coercion::<T, U, _>(coerce);
        let record = Self::record(type_name::<T>(), S::KIND, Recipe::Factory(ctor), dependencies, into_primary, location);
        self.push(record)
    }

    /// A `Construct` type, with its declared scope, its `CONSTRUCT_TIMEOUT` and its trait hooks.
    #[track_caller]
    pub fn provide<T: Construct>(
        self,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, Contribution<T::Scope, Set>> {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        T::dependencies(&mut dependencies);
        let mut record = Self::record(
            type_name::<T>(),
            <T::Scope as Scope>::KIND,
            Recipe::Construct(erase_construct::<T>()),
            dependencies,
            coercion::<T, U, _>(coerce),
            location,
        );
        record.construct_bound = T::CONSTRUCT_TIMEOUT;
        record.hooks = erase_trait_hooks::<T>(location);
        record.constructs = true;
        self.push(record)
    }

    /// A shared value, already built.
    #[track_caller]
    pub fn value(self, value: Arc<U>) -> Handle<'m, U, Contribution<Singleton, Set>> {
        let location = Location::caller();
        let record = Self::record(
            type_name::<U>(),
            ScopeKind::Singleton,
            Recipe::Value(instance_of(value)),
            Dependencies::default(),
            coercion::<U, U, _>(|a| a),
            location,
        );
        self.push(record)
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
        let ctor = erase_factory::<Args, F>(factory);
        self.push_factory::<F::Output, Singleton, Args, F, _>(ctor, coerce, Location::caller())
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
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        self.push_factory::<T, Singleton, Args, F, _>(ctor, coerce, Location::caller())
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
        let ctor = erase_factory::<Args, F>(factory);
        self.push_factory::<F::Output, PerExecution, Args, F, _>(ctor, coerce, Location::caller())
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
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        self.push_factory::<T, PerExecution, Args, F, _>(ctor, coerce, Location::caller())
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
        let ctor = erase_factory::<Args, F>(factory);
        self.push_factory::<F::Output, Transient, Args, F, _>(ctor, coerce, Location::caller())
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
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        self.push_factory::<T, Transient, Args, F, _>(ctor, coerce, Location::caller())
    }
}
