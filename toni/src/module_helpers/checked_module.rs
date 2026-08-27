use crate::startup_check::StartupCheck;
use crate::traits_helpers::{ControllerFactory, ModuleMetadata, ProviderFactory};

use super::DynamicModule;

/// A [`DynamicModule`] carrying the [`StartupCheck`] its providers run, reconfigurable up to the
/// point it is imported.
///
/// Integrations that verify an external dependency return this instead of a `DynamicModule`, so a
/// caller can weaken or drop the check without a second constructor for every combination:
///
/// ```ignore
/// #[module(imports: [
///     SeaOrmModule::for_root(primary),
///     RedisModule::for_root(cache).without_startup_check(),
/// ])]
/// pub struct AppModule;
/// ```
///
/// The module is rebuilt whenever the check changes, because the check is folded into the provider
/// factories rather than read later.
pub struct CheckedModule {
    make: Box<dyn Fn(Option<StartupCheck>) -> DynamicModule + Send + Sync>,
    check: Option<StartupCheck>,
    built: DynamicModule,
}

impl CheckedModule {
    /// Wraps a maker that folds the check into whatever providers need it. Starts checked, with
    /// [`StartupCheck::default`].
    pub fn new(
        make: impl Fn(Option<StartupCheck>) -> DynamicModule + Send + Sync + 'static,
    ) -> Self {
        let check = Some(StartupCheck::default());
        let built = make(check.clone());
        Self {
            make: Box::new(make),
            check,
            built,
        }
    }

    /// Replaces the check this module's providers run.
    pub fn with_startup_check(mut self, check: StartupCheck) -> Self {
        self.check = Some(check);
        self.built = (self.make)(self.check.clone());
        self
    }

    /// Starts the application without contacting the dependency.
    ///
    /// Failures then surface where the dependency is first used, which is a request for most of
    /// them. Worth taking when a readiness probe already reports the dependency and the
    /// application should stay up while it recovers.
    pub fn without_startup_check(mut self) -> Self {
        self.check = None;
        self.built = (self.make)(None);
        self
    }

    /// The check these providers will run, or `None` when it has been dropped.
    pub fn startup_check(&self) -> Option<&StartupCheck> {
        self.check.as_ref()
    }
}

impl ModuleMetadata for CheckedModule {
    fn identity(&self) -> crate::ModuleIdentity {
        self.built.identity()
    }

    fn is_global(&self) -> bool {
        self.built.is_global()
    }

    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
        self.built.imports()
    }

    fn controllers(&self) -> Option<Vec<Box<dyn ControllerFactory>>> {
        self.built.controllers()
    }

    // Delegates to the built module, whose providers are drained on the first call. Rebuilding
    // here instead would hand the scanner a second set.
    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> {
        self.built.providers()
    }

    fn exports(&self) -> Option<Vec<String>> {
        self.built.exports()
    }
}
