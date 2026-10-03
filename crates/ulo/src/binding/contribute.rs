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
use crate::module::def::{ModuleNode, ValueFailure};
use crate::scope::{Auto, PerExecution, Scope, ScopeKind, Singleton, Transient};
use crate::timer::BoxError;

/// Contributions to the collection `U @ Q`, read as `Many<U, Q>` or, by a transport, through
/// `Resolver::entries` (§3.2, §7).
///
/// Each contribution is built as its own type and widened to `U` by the coercion closure,
/// written `|a| a`. Contributions are app-wide: they are not exports, and a keyed module's keep
/// their key.
///
/// A contribution under a role key is an enhancer, whichever builder registered it, when a
/// mounted handler's transport reads that key or when any module contributes under it through
/// [`ModuleDef::enhancer`](crate::ModuleDef::enhancer); scope inference treats an enhancer as it
/// treats a controller (§3.3, §7). Any other contribution is a provider contribution. One written
/// through [`with`](Self::with) takes its scope from that role rather than from inference: per
/// execution for an enhancer, singleton for a provider. `enhancer` returns this builder marked
/// [`Enhancer`], which has no `qualified`; it is also how a role key is marked when no mounted
/// handler's transport reads it.
///
/// ```ignore
/// m.contribute::<dyn Plugin>().provide::<MetricsPlugin>(|a| a);
/// m.enhancer::<AnyGuard<Http>>().provide::<AuthGuard>(|a| a);
/// m.contribute::<dyn HealthIndicator>()
///     .singleton(|pool: Dep<PgPool>| async move { PgHealth::new(pool) }, |a| a);
/// ```
pub struct Contribute<'m, U: ?Sized, Q = (), M = Plain> {
    node: &'m mut ModuleNode,
    /// `true` exactly when `M` is `Enhancer`: each record this builder pushes is listed in
    /// `ModuleNode::enhancers`.
    enhancer: bool,
    _u: PhantomData<fn() -> (PhantomData<U>, Q, M)>,
}

/// The mark of the builder [`ModuleDef::contribute`](crate::ModuleDef::contribute) returns.
pub enum Plain {}

/// The mark of the builder [`ModuleDef::enhancer`](crate::ModuleDef::enhancer) returns. It has no
/// `qualified`: a transport reads only a role key's unqualified collection, so a qualified
/// enhancer would be read by nothing.
pub enum Enhancer {}

impl<'m, U: ?Sized + Send + Sync + 'static, Q: 'static> Contribute<'m, U, Q, Plain> {
    pub(crate) fn new(node: &'m mut ModuleNode) -> Self {
        Contribute { node, enhancer: false, _u: PhantomData }
    }

    /// Contributes to `U @ Q2` instead of the unqualified collection. Under a role key `wire()`
    /// refuses it: a transport reads only the unqualified collection.
    pub fn qualified<Q2: 'static>(self) -> Contribute<'m, U, Q2, Plain> {
        Contribute { node: self.node, enhancer: self.enhancer, _u: PhantomData }
    }
}

impl<'m, U: ?Sized + Send + Sync + 'static> Contribute<'m, U, (), Enhancer> {
    /// The builder `ModuleDef::enhancer` returns, its `U` a role key.
    pub(crate) fn for_role(node: &'m mut ModuleNode) -> Self {
        Contribute { node, enhancer: true, _u: PhantomData }
    }
}

impl<'m, U: ?Sized + Send + Sync + 'static, Q: 'static, M> Contribute<'m, U, Q, M> {
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
        if self.enhancer {
            node.enhancers.push(index);
        }
        Handle::new(&mut node.bindings[index])
    }

    /// A factory contribution: a plain factory runs no trait hooks.
    fn factory_record<T: Send + Sync + 'static, S: Scope, Args, F: Factory<Args>>(
        ctor: ErasedCtor,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
        location: &'static Location<'static>,
    ) -> BindingRecord {
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        let into_primary = coercion::<T, U, _>(coerce);
        Self::record(type_name::<T>(), S::KIND, Recipe::Factory(ctor), dependencies, into_primary, location)
    }

    fn push_factory<T: Send + Sync + 'static, S: Scope, Args, F: Factory<Args>, K>(
        self,
        ctor: ErasedCtor,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
        location: &'static Location<'static>,
    ) -> Handle<'m, T, K> {
        let record = Self::factory_record::<T, S, Args, F>(ctor, coerce, location);
        self.push(record)
    }

    fn push_by_role<T: Send + Sync + 'static, Args, F: Factory<Args>, K>(
        self,
        ctor: ErasedCtor,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
        location: &'static Location<'static>,
    ) -> Handle<'m, T, K> {
        let mut record = Self::factory_record::<T, Auto, Args, F>(ctor, coerce, location);
        record.scope_by_role = true;
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

    /// The lowering of `value = expr?` in an `into` list. An `Err` is recorded, redacted, under a
    /// `WiringErrors` entry naming the module and the collection; `register` carries on. An `Ok`
    /// is contributed as [`value`](Self::value) contributes it.
    #[track_caller]
    pub fn try_value<E: Into<BoxError>>(self, value: Result<Arc<U>, E>) -> Handle<'m, U, Contribution<Singleton, Set>> {
        let location = Location::caller();
        let recipe = match value {
            Ok(value) => Recipe::Value(instance_of(value)),
            Err(error) => {
                self.node.failures.push(ValueFailure { key: Key::of::<U, Q>(), error: error.into(), location });
                Recipe::Failed
            }
        };
        let record = Self::record(
            type_name::<U>(),
            ScopeKind::Singleton,
            recipe,
            Dependencies::default(),
            coercion::<U, U, _>(|a| a),
            location,
        );
        self.push(record)
    }

    /// A factory contribution whose scope follows its role, decided at `wire()`: built per
    /// execution when the contribution is an enhancer, as every enhancer declared by closure is,
    /// and a singleton when it is a provider contribution, refused if it reads execution data.
    /// The lowering of `with = closure` in an `into` list. [`singleton`](Self::singleton),
    /// [`execution`](Self::execution) and [`transient`](Self::transient) write the scope instead.
    ///
    /// The handle's hooks and readiness check run as a singleton's on a provider contribution;
    /// on an enhancer `wire()` refuses them, since a binding built per call never reaches
    /// `connect`.
    #[track_caller]
    pub fn with<Args, F>(
        self,
        factory: F,
        coerce: impl Fn(Arc<F::Output>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, F::Output, Contribution<Auto, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        let ctor = erase_factory::<Args, F>(factory);
        self.push_by_role::<F::Output, Args, F, _>(ctor, coerce, Location::caller())
    }

    /// [`with`](Self::with) for a factory returning a `Result`: its `Err` fails the construction,
    /// as [`try_singleton`](Self::try_singleton)'s does.
    #[track_caller]
    pub fn try_with<Args, F, T, E>(
        self,
        factory: F,
        coerce: impl Fn(Arc<T>) -> Arc<U> + Send + Sync + 'static,
    ) -> Handle<'m, T, Contribution<Auto, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        self.push_by_role::<T, Args, F, _>(ctor, coerce, Location::caller())
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
