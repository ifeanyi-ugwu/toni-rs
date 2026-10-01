//! Building a module graph with some bindings overridden, without editing the production
//! modules (§11).
//!
//! An override replaces the recipe of an existing key and keeps its origin module, its scope,
//! its visibility and its exports. It names no module by default and must match exactly one
//! binding: one that matches none is a wiring error, which catches stale mocks after refactors,
//! and one that matches several is a wiring error listing every match. An override cannot change
//! a key's kind; `override_many` replaces a whole collection.

use std::any::{TypeId, type_name};
use std::marker::PhantomData;
use std::panic::Location;
use std::sync::Arc;
use std::time::Duration;

use crate::app::{App, AppBuilder, Connected, Wired};
use crate::binding::factory::{Factory, erase_factory, erase_try_factory};
use crate::binding::{Instance, Qualifier, Recipe, instance_of};
use crate::dependency::Dependencies;
use crate::error::StartupError;
use crate::key::Key;
use crate::module::{Module, ModuleIdentity};
use crate::timer::{BoxError, Timer};

/// No override is waiting to be scoped.
pub enum Settled {}

/// The last call was an override, which `in_module*` or `everywhere` may scope.
pub enum Pending {}

/// The test builder: `TestApp::of(AppModule)`, overrides, then `wire()` or `connect()`.
///
/// ```ignore
/// let app = TestApp::of(AppModule)
///     .override_value::<dyn UserRepo>(Arc::new(InMemoryRepo::default()))
///     .override_value::<AuditLog>(Arc::new(NullAudit)).in_module::<BillingModule>()
///     .replace_module(MailModule, FakeMailModule)
///     .connect()
///     .await?;
/// ```
pub struct TestApp<S = Settled> {
    builder: AppBuilder,
    _s: PhantomData<S>,
}

/// The overrides and replacements a `TestApp` carries into `wire()`.
#[derive(Default)]
pub(crate) struct TestPlan {
    pub(crate) overrides: Vec<Override>,
    pub(crate) collections: Vec<CollectionOverride>,
    pub(crate) replacements: Vec<Replacement>,
}

pub(crate) struct Override {
    pub(crate) key: Key,
    pub(crate) recipe: Recipe,
    /// The replacement recipe's own dependencies: a factory's parameters, empty for a value.
    /// They take the place of the replaced binding's dependencies, since the replaced recipe no
    /// longer reads them.
    pub(crate) dependencies: Dependencies,
    pub(crate) target: OverrideTarget,
    pub(crate) location: &'static Location<'static>,
}

pub(crate) enum OverrideTarget {
    /// Must match exactly one binding.
    Unscoped,
    /// Every instance of the module type; ambiguous over several.
    ModuleType { ty: TypeId, name: &'static str },
    ModuleKeyed { ty: TypeId, name: &'static str, qualifier: Qualifier },
    Module(ModuleIdentity),
    Everywhere,
}

pub(crate) struct CollectionOverride {
    pub(crate) key: Key,
    pub(crate) items: Vec<Instance>,
    pub(crate) location: &'static Location<'static>,
}

pub(crate) struct Replacement {
    pub(crate) original: ModuleIdentity,
    pub(crate) replacement: Box<dyn Module>,
    pub(crate) location: &'static Location<'static>,
}

impl TestApp {
    pub fn of(root: impl Module) -> TestApp<Settled> {
        let mut builder = App::builder(root);
        builder.plan = Some(TestPlan::default());
        TestApp { builder, _s: PhantomData }
    }
}

impl<S> TestApp<S> {
    /// Replaces the recipe of the binding under `T` with a shared value.
    /// `override_value::<dyn Timer>` is a wiring error: set it with [`timer`](Self::timer).
    #[track_caller]
    pub fn override_value<T: ?Sized + Send + Sync + 'static>(self, value: Arc<T>) -> TestApp<Pending> {
        let location = Location::caller();
        self.push_override(Override {
            key: Key::of::<T, ()>(),
            recipe: Recipe::Value(instance_of(value)),
            dependencies: Dependencies::default(),
            target: OverrideTarget::Unscoped,
            location,
        })
    }

    /// Replaces the recipe of the binding under the future's output type.
    #[track_caller]
    pub fn override_factory<Args, F>(self, factory: F) -> TestApp<Pending>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        self.push_override(Override {
            key: Key::of::<F::Output, ()>(),
            recipe: Recipe::Factory(erase_factory::<Args, F>(factory)),
            dependencies,
            target: OverrideTarget::Unscoped,
            location,
        })
    }

    /// Replaces the recipe of the binding under the `Ok` type of the future's output.
    #[track_caller]
    pub fn override_try_factory<Args, F, T, E>(self, factory: F) -> TestApp<Pending>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        let location = Location::caller();
        let mut dependencies = Dependencies::default();
        <F as Factory<Args>>::dependencies(&mut dependencies);
        self.push_override(Override {
            key: Key::of::<T, ()>(),
            recipe: Recipe::Factory(erase_try_factory::<Args, F, T, E>(factory)),
            dependencies,
            target: OverrideTarget::Unscoped,
            location,
        })
    }

    /// Replaces every contribution to the collection `T` with `items`.
    #[track_caller]
    pub fn override_many<T: ?Sized + Send + Sync + 'static>(self, items: impl IntoIterator<Item = Arc<T>>) -> TestApp<Settled> {
        let location = Location::caller();
        let mut this = self.into_state::<Settled>();
        this.plan().collections.push(CollectionOverride {
            key: Key::of::<T, ()>(),
            items: items.into_iter().map(instance_of).collect(),
            location,
        });
        this
    }

    /// Swaps a module by identity. The replacement must export a superset of the original's
    /// keys, or wiring reports what is missing.
    #[track_caller]
    pub fn replace_module(self, original: impl Module, replacement: impl Module) -> TestApp<Settled> {
        let location = Location::caller();
        let mut this = self.into_state::<Settled>();
        this.plan().replacements.push(Replacement {
            original: original.identity(),
            replacement: Box::new(replacement),
            location,
        });
        this
    }

    /// The one clock that times the drain, the hooks, the constructions and the readiness
    /// checks, and that services read as `Dep<dyn Timer>`.
    pub fn timer(self, timer: impl Timer) -> TestApp<Settled> {
        TestApp { builder: self.builder.timer(timer), _s: PhantomData }
    }

    pub fn drain_timeout(self, d: Duration) -> TestApp<Settled> {
        TestApp { builder: self.builder.drain_timeout(d), _s: PhantomData }
    }

    pub fn shutdown_timeout(self, d: Duration) -> TestApp<Settled> {
        TestApp { builder: self.builder.shutdown_timeout(d), _s: PhantomData }
    }

    pub fn hook_timeout(self, d: Duration) -> TestApp<Settled> {
        TestApp { builder: self.builder.hook_timeout(d), _s: PhantomData }
    }

    pub fn construct_timeout(self, d: Duration) -> TestApp<Settled> {
        TestApp { builder: self.builder.construct_timeout(d), _s: PhantomData }
    }

    pub fn wire(self) -> Result<App<Wired>, StartupError> {
        self.builder.wire()
    }

    pub async fn connect(self) -> Result<App<Connected>, StartupError> {
        self.builder.wire()?.connect().await
    }

    fn push_override(self, item: Override) -> TestApp<Pending> {
        let mut this = self.into_state::<Pending>();
        this.plan().overrides.push(item);
        this
    }

    fn into_state<S2>(self) -> TestApp<S2> {
        TestApp { builder: self.builder, _s: PhantomData }
    }

    fn plan(&mut self) -> &mut TestPlan {
        self.builder.plan.get_or_insert_with(TestPlan::default)
    }
}

impl TestApp<Pending> {
    /// Scopes the last override to the module of type `M`; over several instances of `M`,
    /// configured or keyed, it is itself ambiguous and fails as one.
    pub fn in_module<M: 'static>(self) -> TestApp<Settled> {
        self.scope_last(OverrideTarget::ModuleType { ty: TypeId::of::<M>(), name: type_name::<M>() })
    }

    /// Scopes the last override to the keyed instance `Q` of `M`.
    pub fn in_module_keyed<M: 'static, Q: 'static>(self) -> TestApp<Settled> {
        self.scope_last(OverrideTarget::ModuleKeyed { ty: TypeId::of::<M>(), name: type_name::<M>(), qualifier: Qualifier::of::<Q>() })
    }

    /// Scopes the last override to the module with `config`'s identity.
    pub fn in_module_of<M: Module>(self, config: &M) -> TestApp<Settled> {
        self.scope_last(OverrideTarget::Module(config.identity()))
    }

    /// Replaces every binding the last override matches.
    pub fn everywhere(self) -> TestApp<Settled> {
        self.scope_last(OverrideTarget::Everywhere)
    }

    /// `Pending` exists only after an `override_*` call pushed an override, so the last one is
    /// the one this scopes.
    fn scope_last(self, target: OverrideTarget) -> TestApp<Settled> {
        let mut this = self.into_state::<Settled>();
        if let Some(last) = this.plan().overrides.last_mut() {
            last.target = target;
        }
        this
    }
}
