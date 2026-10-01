use std::any::{Any, TypeId, type_name};
use std::fmt;
use std::marker::PhantomData;
use std::panic::Location;
use std::sync::Arc;
use std::time::Duration;

use crate::binding::alias::{Alias, Input};
use crate::binding::contribute::Contribute;
use crate::binding::factory::{
    Factory, ShutdownFactory, erase_destroy_hook, erase_factory, erase_init_hook, erase_signalled_hook,
    erase_try_factory,
};
use crate::binding::handle::{Binding, Handle, Open, Set};
use crate::binding::{BindingRecord, Qualifier, Recipe, erase_construct, instance_of};
use crate::construct::Construct;
use crate::hooks::{HookFn, HookKind, HookRecord, erase_trait_hooks};
use crate::key::{BindingKind, Key};
use crate::module::Module;
use crate::module::ModuleIdentity;
use crate::module::meta::{Meta, MetaMap};
use crate::redact::Secret;
use crate::scope::{PerExecution, Scope, ScopeKind, Singleton, Transient};
use crate::site::Sites;
use crate::timer::{BoxError, Bound};
use crate::transport::controller::{Controller, ControllerRecord};

/// What `Module::register` writes into: imports, bindings, contributions, controllers, exports,
/// inputs, module hooks, secrets and typed metadata. Registration is synchronous and free of
/// I/O; nothing is built until `connect`.
///
/// Every method records its caller's source location, which the wiring errors print.
pub struct ModuleDef<'a> {
    pub(crate) node: &'a mut ModuleNode,
}

impl<'a> ModuleDef<'a> {
    pub(crate) fn new(node: &'a mut ModuleNode) -> Self {
        ModuleDef { node }
    }

    /// Imports `module`: this module sees its exports. Two imports of an equal identity, from
    /// anywhere in the graph, are one module.
    #[track_caller]
    pub fn import<M: Module>(&mut self, module: M) {
        self.node.imports.push(ImportRecord { module: Box::new(module), location: Location::caller() });
    }

    /// Makes this module's exports visible to every module.
    pub fn global(&mut self) {
        self.node.global = true;
    }

    /// Exports the binding `T @ ()`. An export is the only way a binding leaves its module.
    #[track_caller]
    pub fn export<T: ?Sized + 'static>(&mut self) {
        self.push_export(Key::of::<T, ()>(), false, Location::caller());
    }

    /// Exports the binding `T @ Q`.
    #[track_caller]
    pub fn export_qualified<T: ?Sized + 'static, Q: 'static>(&mut self) {
        self.push_export(Key::of::<T, Q>(), false, Location::caller());
    }

    /// Re-exports `T @ ()` from an import. Allowed only for a key this module can see
    /// unambiguously.
    #[track_caller]
    pub fn reexport<T: ?Sized + 'static>(&mut self) {
        self.push_export(Key::of::<T, ()>(), true, Location::caller());
    }

    #[track_caller]
    pub fn reexport_qualified<T: ?Sized + 'static, Q: 'static>(&mut self) {
        self.push_export(Key::of::<T, Q>(), true, Location::caller());
    }

    /// Registers a secret this module holds, so the redaction function replaces it in every
    /// error this graph reports. Registration is per graph, never process-wide.
    pub fn secret<T: fmt::Display>(&mut self, secret: &Secret<T>) {
        self.node.secrets.push(secret.expose().to_string());
    }

    /// Typed per-module metadata a transport reads, such as `ulo-http`'s middleware
    /// configuration: `m.meta::<Middleware>().apply::<RequestLogger>().for_routes([..])`.
    pub fn meta<T: Meta>(&mut self) -> &mut T {
        self.node.meta.get_or_default::<T>()
    }

    /// The module's own init hook, run after the init hooks of its providers.
    #[track_caller]
    pub fn on_init<Args, F, E>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: Factory<Args, Output = Result<(), E>>,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        self.push_hook(HookKind::OnModuleInit, erase_init_hook::<Args, F, E>(hook), sites, location)
    }

    #[track_caller]
    pub fn on_destroy<Args, F>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: Factory<Args, Output = ()>,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        self.push_hook(HookKind::OnModuleDestroy, erase_destroy_hook::<Args, F>(hook), sites, location)
    }

    #[track_caller]
    pub fn before_shutdown<Args, F>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: ShutdownFactory<Args>,
    {
        let location = Location::caller();
        let sites = shutdown_sites::<Args, F>();
        self.push_hook(HookKind::BeforeApplicationShutdown, erase_signalled_hook::<Args, F>(hook), sites, location)
    }

    #[track_caller]
    pub fn on_shutdown<Args, F>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: ShutdownFactory<Args>,
    {
        let location = Location::caller();
        let sites = shutdown_sites::<Args, F>();
        self.push_hook(HookKind::OnApplicationShutdown, erase_signalled_hook::<Args, F>(hook), sites, location)
    }

    /// A `Construct` type under its own key, with its declared scope, `CONSTRUCT_TIMEOUT` and
    /// trait hooks. The trait const writes the construction's bound, so the handle starts `Set`.
    #[track_caller]
    pub fn provide<T: Construct>(&mut self) -> Handle<'_, T, Binding<T::Scope, Set>> {
        let location = Location::caller();
        let mut sites = Sites::default();
        T::sites(&mut sites);
        let mut record = construct_record::<T>(Recipe::Construct(erase_construct::<T>()), sites, location);
        record.construct_bound = T::CONSTRUCT_TIMEOUT;
        Handle::new(self.push(record))
    }

    /// A `Construct` type built by `factory` instead of `T::construct`: keyed by `T`, with
    /// `T::Scope` and `T::hooks`, which a plain factory never runs. The factory's parameters are
    /// the binding's sites; `T::sites` describes a constructor that does not run.
    #[track_caller]
    pub fn provide_with<T: Construct, Args>(
        &mut self,
        factory: impl Factory<Args, Output = T>,
    ) -> Handle<'_, T, Binding<T::Scope, Open>> {
        let location = Location::caller();
        let sites = sites_of::<Args, _>(&factory);
        let record = construct_record::<T>(Recipe::Factory(erase_factory::<Args, _>(factory)), sites, location);
        Handle::new(self.push(record))
    }

    #[track_caller]
    pub fn try_provide_with<T: Construct, Args, E>(
        &mut self,
        factory: impl Factory<Args, Output = Result<T, E>>,
    ) -> Handle<'_, T, Binding<T::Scope, Open>>
    where
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let sites = sites_of::<Args, _>(&factory);
        let ctor = erase_try_factory::<Args, _, T, E>(factory);
        let record = construct_record::<T>(Recipe::Factory(ctor), sites, location);
        Handle::new(self.push(record))
    }

    /// A value under the key of its own type, a `Result` included. A `Secret<String>` value
    /// registers itself for redaction.
    #[track_caller]
    pub fn value<T: Send + Sync + 'static>(&mut self, value: T) -> Handle<'_, T, Binding<Singleton, Set>> {
        let location = Location::caller();
        self.register_secret(&value);
        let record = value_record::<T>(Recipe::Value(instance_of(Arc::new(value))), location);
        Handle::new(self.push(record))
    }

    /// The lowering of `expr?` in a `#[module]` providers list. An `Err` is recorded, redacted,
    /// under a `WiringErrors` entry naming the module and the key; `register` carries on.
    #[track_caller]
    pub fn try_value<T, E>(&mut self, value: Result<T, E>) -> Handle<'_, T, Binding<Singleton, Set>>
    where
        T: Send + Sync + 'static,
        E: Into<BoxError>,
    {
        let location = Location::caller();
        let recipe = match value {
            Ok(value) => {
                self.register_secret(&value);
                Recipe::Value(instance_of(Arc::new(value)))
            }
            Err(error) => {
                self.node.failures.push(ValueFailure { key: Key::of::<T, ()>(), error: error.into(), location });
                Recipe::Failed
            }
        };
        Handle::new(self.push(value_record::<T>(recipe, location)))
    }

    /// A singleton built by an async factory whose parameters are sites; the future's output
    /// is the key.
    #[track_caller]
    pub fn singleton<Args, F>(&mut self, factory: F) -> Handle<'_, F::Output, Binding<Singleton, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        let ctor = erase_factory::<Args, F>(factory);
        Handle::new(self.push(factory_record::<Singleton, F::Output>(ctor, sites, location)))
    }

    /// A singleton whose factory returns a `Result`; the `Ok` type is the key and an `Err`
    /// fails `connect` as `ConnectError::Construct { reason: Errored }`.
    #[track_caller]
    pub fn try_singleton<Args, F, T, E>(&mut self, factory: F) -> Handle<'_, T, Binding<Singleton, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        Handle::new(self.push(factory_record::<Singleton, T>(ctor, sites, location)))
    }

    #[track_caller]
    pub fn execution<Args, F>(&mut self, factory: F) -> Handle<'_, F::Output, Binding<PerExecution, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        let ctor = erase_factory::<Args, F>(factory);
        Handle::new(self.push(factory_record::<PerExecution, F::Output>(ctor, sites, location)))
    }

    /// An execution-scoped binding whose factory returns a `Result`; an `Err` inside a call
    /// reports as `LookupError::Construct { reason: Errored }`.
    #[track_caller]
    pub fn try_execution<Args, F, T, E>(&mut self, factory: F) -> Handle<'_, T, Binding<PerExecution, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        Handle::new(self.push(factory_record::<PerExecution, T>(ctor, sites, location)))
    }

    #[track_caller]
    pub fn transient<Args, F>(&mut self, factory: F) -> Handle<'_, F::Output, Binding<Transient, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        let ctor = erase_factory::<Args, F>(factory);
        Handle::new(self.push(factory_record::<Transient, F::Output>(ctor, sites, location)))
    }

    #[track_caller]
    pub fn try_transient<Args, F, T, E>(&mut self, factory: F) -> Handle<'_, T, Binding<Transient, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let sites = factory_sites::<Args, F>();
        let ctor = erase_try_factory::<Args, F, T, E>(factory);
        Handle::new(self.push(factory_record::<Transient, T>(ctor, sites, location)))
    }

    /// Contributions to the collection `U`, from this and any other module.
    pub fn contribute<U: ?Sized + Send + Sync + 'static>(&mut self) -> Contribute<'_, U> {
        Contribute::new(&mut *self.node)
    }

    /// The key `T @ Q` as a second name for an existing binding of `T`, named in `.of::<Existing>()`.
    pub fn alias<T: ?Sized + Send + Sync + 'static, Q: 'static>(&mut self) -> Alias<'_, T, Q> {
        Alias::new(&mut *self.node)
    }

    /// An execution input, declared by a transport's own global module.
    pub fn input<T: Send + Sync + 'static>(&mut self) -> Input<'_, T> {
        Input::new(&mut *self.node)
    }

    /// A controller: a `Construct` type whose handlers `C::mount` declares. With `Auto` scope it
    /// is built per call when anything below it needs an execution.
    #[track_caller]
    pub fn controller<C: Controller>(&mut self) {
        let location = Location::caller();
        let mut sites = Sites::default();
        C::sites(&mut sites);
        let mut record = construct_record::<C>(Recipe::Construct(erase_construct::<C>()), sites, location);
        record.construct_bound = C::CONSTRUCT_TIMEOUT;
        record.controller = true;
        let binding = self.node.bindings.len();
        self.node.bindings.push(record);
        self.node.controllers.push(ControllerRecord { binding, mount: C::mount });
    }

    /// Marks this module as the inner module of a `Keyed<Q, M>`.
    pub(crate) fn keyed_by(&mut self, qualifier: Qualifier) {
        self.node.keyed = Some(qualifier);
    }

    fn push(&mut self, record: BindingRecord) -> &mut BindingRecord {
        let index = self.node.bindings.len();
        self.node.bindings.push(record);
        &mut self.node.bindings[index]
    }

    fn push_export(&mut self, key: Key, reexport: bool, location: &'static Location<'static>) {
        self.node.exports.push(ExportRecord { key, reexport, location });
    }

    fn push_hook(
        &mut self,
        kind: HookKind,
        run: HookFn,
        sites: Sites,
        location: &'static Location<'static>,
    ) -> ModuleHook<'_> {
        self.node.hooks.push(HookRecord { kind, bound: Bound::Default, run, sites, location });
        ModuleHook::new(&mut *self.node)
    }

    /// A `Secret<String>` is the one secret a generic value can be recognized as; any other
    /// `Secret<T>` bound by value is registered with `m.secret(..)`.
    fn register_secret(&mut self, value: &dyn Any) {
        if let Some(secret) = value.downcast_ref::<Secret<String>>() {
            self.node.secrets.push(secret.expose().clone());
        }
    }
}

/// A binding built by `T::construct` or by a factory standing in for it: `T::Scope` and
/// `T::hooks` either way.
fn construct_record<T: Construct>(recipe: Recipe, sites: Sites, location: &'static Location<'static>) -> BindingRecord {
    let kind = <T::Scope as Scope>::KIND;
    let mut record = BindingRecord::new(Key::of::<T, ()>(), type_name::<T>(), BindingKind::Single, kind, recipe, sites, location);
    record.hooks = erase_trait_hooks::<T>(location);
    record.constructs = true;
    record
}

fn factory_record<S: Scope, T: 'static>(
    ctor: crate::binding::ErasedCtor,
    sites: Sites,
    location: &'static Location<'static>,
) -> BindingRecord {
    BindingRecord::new(Key::of::<T, ()>(), type_name::<T>(), BindingKind::Single, S::KIND, Recipe::Factory(ctor), sites, location)
}

fn value_record<T: 'static>(recipe: Recipe, location: &'static Location<'static>) -> BindingRecord {
    BindingRecord::new(
        Key::of::<T, ()>(),
        type_name::<T>(),
        BindingKind::Single,
        ScopeKind::Singleton,
        recipe,
        Sites::default(),
        location,
    )
}

fn factory_sites<Args, F: Factory<Args>>() -> Sites {
    let mut sites = Sites::default();
    <F as Factory<Args>>::sites(&mut sites);
    sites
}

fn sites_of<Args, F: Factory<Args>>(_factory: &F) -> Sites {
    factory_sites::<Args, F>()
}

fn shutdown_sites<Args, F: ShutdownFactory<Args>>() -> Sites {
    let mut sites = Sites::default();
    <F as ShutdownFactory<Args>>::sites(&mut sites);
    sites
}

/// The handle a module hook returns: its bound is written once, with `.timeout(..)` or
/// `.unbounded()`, or left at `Default`.
pub struct ModuleHook<'m, B = Open> {
    node: &'m mut ModuleNode,
    _b: PhantomData<B>,
}

impl<'m> ModuleHook<'m, Open> {
    pub(crate) fn new(node: &'m mut ModuleNode) -> Self {
        ModuleHook { node, _b: PhantomData }
    }

    pub fn timeout(self, d: Duration) -> ModuleHook<'m, Set> {
        self.write(Bound::After(d))
    }

    pub fn unbounded(self) -> ModuleHook<'m, Set> {
        self.write(Bound::Unbounded)
    }

    fn write(self, bound: Bound) -> ModuleHook<'m, Set> {
        if let Some(hook) = self.node.hooks.last_mut() {
            hook.bound = bound;
        }
        ModuleHook { node: self.node, _b: PhantomData }
    }
}

/// One module as `register` left it: everything the graph needs to deduplicate, order, check
/// and freeze it.
pub(crate) struct ModuleNode {
    pub(crate) identity: ModuleIdentity,
    pub(crate) global: bool,
    /// Set inside `Keyed<Q, M>`: unqualified exports leave the boundary as `T @ Q`.
    pub(crate) keyed: Option<Qualifier>,
    /// Queued by `import` and registered by the graph after `register` returns, in the order
    /// written; the order is part of collection order.
    pub(crate) imports: Vec<ImportRecord>,
    /// Declaration order is the tie-break inside a module.
    pub(crate) bindings: Vec<BindingRecord>,
    pub(crate) exports: Vec<ExportRecord>,
    pub(crate) controllers: Vec<ControllerRecord>,
    pub(crate) inputs: Vec<InputRecord>,
    /// The module's own hooks, run after those of its providers.
    pub(crate) hooks: Vec<HookRecord>,
    pub(crate) meta: MetaMap,
    pub(crate) secrets: Vec<String>,
    /// `try_value` errors, redacted when `wire()` reports them.
    pub(crate) failures: Vec<ValueFailure>,
}

impl ModuleNode {
    pub(crate) fn new(identity: ModuleIdentity) -> Self {
        ModuleNode {
            identity,
            global: false,
            keyed: None,
            imports: Vec::new(),
            bindings: Vec::new(),
            exports: Vec::new(),
            controllers: Vec::new(),
            inputs: Vec::new(),
            hooks: Vec::new(),
            meta: MetaMap::default(),
            secrets: Vec::new(),
            failures: Vec::new(),
        }
    }
}

pub(crate) struct ImportRecord {
    pub(crate) module: Box<dyn Module>,
    pub(crate) location: &'static Location<'static>,
}

pub(crate) struct ExportRecord {
    /// As written inside the module; a keyed module requalifies an unqualified key at freeze.
    pub(crate) key: Key,
    pub(crate) reexport: bool,
    pub(crate) location: &'static Location<'static>,
}

pub(crate) struct InputRecord {
    pub(crate) key: Key,
    pub(crate) seeder: TypeId,
    pub(crate) seeder_name: &'static str,
    pub(crate) location: &'static Location<'static>,
}

pub(crate) struct ValueFailure {
    pub(crate) key: Key,
    pub(crate) error: BoxError,
    pub(crate) location: &'static Location<'static>,
}
