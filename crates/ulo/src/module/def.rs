use std::any::TypeId;
use std::fmt;
use std::marker::PhantomData;
use std::panic::Location;
use std::time::Duration;

use crate::binding::alias::{Alias, Input};
use crate::binding::contribute::Contribute;
use crate::binding::factory::{Factory, ShutdownFactory};
use crate::binding::handle::{Binding, Handle, Open, Set};
use crate::binding::{BindingRecord, Qualifier};
use crate::construct::Construct;
use crate::hooks::HookRecord;
use crate::key::Key;
use crate::module::Module;
use crate::module::ModuleIdentity;
use crate::module::meta::{Meta, MetaMap};
use crate::redact::Secret;
use crate::scope::{PerExecution, Singleton, Transient};
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

    #[track_caller]
    pub fn import<M: Module>(&mut self, module: M) {
        todo!()
    }

    /// Makes this module's exports visible to every module.
    pub fn global(&mut self) {
        self.node.global = true;
    }

    /// Exports the binding `T @ ()`. An export is the only way a binding leaves its module.
    #[track_caller]
    pub fn export<T: ?Sized + 'static>(&mut self) {
        todo!()
    }

    /// Exports the binding `T @ Q`.
    #[track_caller]
    pub fn export_qualified<T: ?Sized + 'static, Q: 'static>(&mut self) {
        todo!()
    }

    /// Re-exports `T @ ()` from an import. Allowed only for a key this module can see
    /// unambiguously.
    #[track_caller]
    pub fn reexport<T: ?Sized + 'static>(&mut self) {
        todo!()
    }

    #[track_caller]
    pub fn reexport_qualified<T: ?Sized + 'static, Q: 'static>(&mut self) {
        todo!()
    }

    /// Registers a secret this module holds, so the redaction function replaces it in every
    /// error this graph reports. Registration is per graph, never process-wide.
    pub fn secret<T: fmt::Display>(&mut self, secret: &Secret<T>) {
        todo!()
    }

    /// Typed per-module metadata a transport reads, such as `ulo-http`'s middleware
    /// configuration: `m.meta::<Middleware>().apply::<RequestLogger>().for_routes([..])`.
    pub fn meta<T: Meta>(&mut self) -> &mut T {
        todo!()
    }

    /// The module's own init hook, run after the init hooks of its providers.
    #[track_caller]
    pub fn on_init<Args, F, E>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: Factory<Args, Output = Result<(), E>>,
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn on_destroy<Args, F>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: Factory<Args, Output = ()>,
    {
        todo!()
    }

    #[track_caller]
    pub fn before_shutdown<Args, F>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: ShutdownFactory<Args>,
    {
        todo!()
    }

    #[track_caller]
    pub fn on_shutdown<Args, F>(&mut self, hook: F) -> ModuleHook<'_>
    where
        F: ShutdownFactory<Args>,
    {
        todo!()
    }

    /// A `Construct` type under its own key, with its declared scope, `CONSTRUCT_TIMEOUT` and
    /// trait hooks. The trait const writes the construction's bound, so the handle starts `Set`.
    #[track_caller]
    pub fn provide<T: Construct>(&mut self) -> Handle<'_, T, Binding<T::Scope, Set>> {
        todo!()
    }

    /// A `Construct` type built by `factory` instead of `T::construct`: keyed by `T`, with
    /// `T::Scope` and `T::hooks`, which a plain factory never runs.
    #[track_caller]
    pub fn provide_with<T: Construct, Args>(
        &mut self,
        factory: impl Factory<Args, Output = T>,
    ) -> Handle<'_, T, Binding<T::Scope, Open>> {
        todo!()
    }

    #[track_caller]
    pub fn try_provide_with<T: Construct, Args, E>(
        &mut self,
        factory: impl Factory<Args, Output = Result<T, E>>,
    ) -> Handle<'_, T, Binding<T::Scope, Open>>
    where
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
    }

    /// A value under the key of its own type, a `Result` included. A `Secret<String>` value
    /// registers itself for redaction.
    #[track_caller]
    pub fn value<T: Send + Sync + 'static>(&mut self, value: T) -> Handle<'_, T, Binding<Singleton, Set>> {
        todo!()
    }

    /// The lowering of `expr?` in a `#[module]` providers list. An `Err` is recorded, redacted,
    /// under a `WiringErrors` entry naming the module and the key; `register` carries on.
    #[track_caller]
    pub fn try_value<T, E>(&mut self, value: Result<T, E>) -> Handle<'_, T, Binding<Singleton, Set>>
    where
        T: Send + Sync + 'static,
        E: Into<BoxError>,
    {
        todo!()
    }

    /// A singleton built by an async factory whose parameters are sites; the future's output
    /// is the key.
    #[track_caller]
    pub fn singleton<Args, F>(&mut self, factory: F) -> Handle<'_, F::Output, Binding<Singleton, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
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
        todo!()
    }

    #[track_caller]
    pub fn execution<Args, F>(&mut self, factory: F) -> Handle<'_, F::Output, Binding<PerExecution, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
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
        todo!()
    }

    #[track_caller]
    pub fn transient<Args, F>(&mut self, factory: F) -> Handle<'_, F::Output, Binding<Transient, Open>>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
    }

    #[track_caller]
    pub fn try_transient<Args, F, T, E>(&mut self, factory: F) -> Handle<'_, T, Binding<Transient, Open>>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
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
        todo!()
    }

    /// Marks this module as the inner module of a `Keyed<Q, M>`.
    pub(crate) fn keyed_by(&mut self, qualifier: Qualifier) {
        self.node.keyed = Some(qualifier);
    }
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
        todo!()
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
