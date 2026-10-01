//! Building a module graph with some bindings overridden, without editing the production
//! modules (§11).
//!
//! An override replaces the recipe of an existing key and keeps its origin module, its scope,
//! its visibility and its exports. It names no module by default and must match exactly one
//! binding: one that matches none is a wiring error, which catches stale mocks after refactors,
//! and one that matches several is a wiring error listing every match. An override cannot change
//! a key's kind; `override_many` replaces a whole collection.

use std::any::TypeId;
use std::marker::PhantomData;
use std::panic::Location;
use std::sync::Arc;
use std::time::Duration;

use crate::app::{App, AppBuilder, Connected, Wired};
use crate::binding::factory::Factory;
use crate::binding::{Instance, Qualifier, Recipe};
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
        todo!()
    }
}

impl<S> TestApp<S> {
    /// Replaces the recipe of the binding under `T` with a shared value.
    /// `override_value::<dyn Timer>` is a wiring error: set it with [`timer`](Self::timer).
    #[track_caller]
    pub fn override_value<T: ?Sized + Send + Sync + 'static>(self, value: Arc<T>) -> TestApp<Pending> {
        todo!()
    }

    /// Replaces the recipe of the binding under the future's output type.
    #[track_caller]
    pub fn override_factory<Args, F>(self, factory: F) -> TestApp<Pending>
    where
        F: Factory<Args>,
        F::Output: Send + Sync + 'static,
    {
        todo!()
    }

    /// Replaces the recipe of the binding under the `Ok` type of the future's output.
    #[track_caller]
    pub fn override_try_factory<Args, F, T, E>(self, factory: F) -> TestApp<Pending>
    where
        F: Factory<Args, Output = Result<T, E>>,
        T: Send + Sync + 'static,
        E: Into<BoxError> + Send + 'static,
    {
        todo!()
    }

    /// Replaces every contribution to the collection `T` with `items`.
    #[track_caller]
    pub fn override_many<T: ?Sized + Send + Sync + 'static>(self, items: impl IntoIterator<Item = Arc<T>>) -> TestApp<Settled> {
        todo!()
    }

    /// Swaps a module by identity. The replacement must export a superset of the original's
    /// keys, or wiring reports what is missing.
    #[track_caller]
    pub fn replace_module(self, original: impl Module, replacement: impl Module) -> TestApp<Settled> {
        todo!()
    }

    /// The one clock that times the drain, the hooks, the constructions and the readiness
    /// checks, and that services read as `Dep<dyn Timer>`.
    pub fn timer(self, timer: impl Timer) -> TestApp<Settled> {
        todo!()
    }

    pub fn drain_timeout(self, d: Duration) -> TestApp<Settled> {
        todo!()
    }

    pub fn shutdown_timeout(self, d: Duration) -> TestApp<Settled> {
        todo!()
    }

    pub fn hook_timeout(self, d: Duration) -> TestApp<Settled> {
        todo!()
    }

    pub fn construct_timeout(self, d: Duration) -> TestApp<Settled> {
        todo!()
    }

    pub fn wire(self) -> Result<App<Wired>, StartupError> {
        todo!()
    }

    pub async fn connect(self) -> Result<App<Connected>, StartupError> {
        todo!()
    }
}

impl TestApp<Pending> {
    /// Scopes the last override to the module of type `M`; over several instances of `M`,
    /// configured or keyed, it is itself ambiguous and fails as one.
    pub fn in_module<M: 'static>(self) -> TestApp<Settled> {
        todo!()
    }

    /// Scopes the last override to the keyed instance `Q` of `M`.
    pub fn in_module_keyed<M: 'static, Q: 'static>(self) -> TestApp<Settled> {
        todo!()
    }

    /// Scopes the last override to the module with `config`'s identity.
    pub fn in_module_of<M: Module>(self, config: &M) -> TestApp<Settled> {
        todo!()
    }

    /// Replaces every binding the last override matches.
    pub fn everywhere(self) -> TestApp<Settled> {
        todo!()
    }
}
