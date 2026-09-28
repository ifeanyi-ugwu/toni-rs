use parking_lot::Mutex;

use super::ModuleIdentity;
use crate::di::ModuleMetadata;
use crate::dispatch::ControllerFactory;
use crate::spi::ProviderFactory;
/// A module whose providers and exports are determined at runtime rather than compile time.
///
/// Integration crates (e.g. `ulo-db-seaorm`) use this to implement `forRoot`/`forFeature`-style
/// factory functions without having to implement all of `ModuleMetadata` manually.
///
/// # Example
/// ```ignore
/// pub struct SeaOrmModule;
///
/// impl SeaOrmModule {
///     pub fn for_root(database_url: &str) -> DynamicModule {
///         DynamicModule::builder("SeaOrmModule")
///             .provider(SeaOrmConnectionFactory::new(database_url))
///             .export::<DatabaseConnection>()
///             .build()
///     }
/// }
/// ```
///
/// Then in the application module:
/// ```ignore
/// #[module(imports: [SeaOrmModule::for_root(DATABASE_URL)])]
/// pub struct AppModule;
/// ```
pub struct DynamicModule {
    // Base name plus a fingerprint of the providers' `identity_hint`s. Two calls to the same
    // maker with identical config collapse to one identity (a diamond import); with different
    // config they stay distinct so the downstream export-token clash surfaces.
    identity: ModuleIdentity,
    // Wrapped in Mutex<Option<...>> so ownership can be moved out on the first call to
    // providers(), which takes &self. The scanner calls providers() exactly once per module
    // during scan_modules_for_dependencies, so draining on first call is safe.
    providers: Mutex<Option<Vec<Box<dyn ProviderFactory>>>>,
    controllers: Mutex<Option<Vec<Box<dyn ControllerFactory>>>>,
    exports: Vec<String>,
    global: bool,
}

impl ModuleMetadata for DynamicModule {
    fn identity(&self) -> ModuleIdentity {
        self.identity.clone()
    }

    fn is_global(&self) -> bool {
        self.global
    }

    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
        None
    }

    fn controllers(&self) -> Option<Vec<Box<dyn ControllerFactory>>> {
        self.controllers.lock().take()
    }

    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> {
        self.providers.lock().take()
    }

    fn exports(&self) -> Option<Vec<String>> {
        Some(self.exports.clone())
    }
}

pub struct DynamicModuleBuilder {
    id: String,
    providers: Vec<Box<dyn ProviderFactory>>,
    controllers: Vec<Box<dyn ControllerFactory>>,
    exports: Vec<String>,
    global: bool,
}

impl DynamicModuleBuilder {
    /// Declare a provider by a factory value, for one that carries configuration it was built
    /// with — a URL, a pool size. Use [`provider`](Self::provider) where the type declares itself.
    pub fn provider_factory<F: ProviderFactory + 'static>(mut self, factory: F) -> Self {
        self.providers.push(Box::new(factory));
        self
    }

    /// Declare a dispatch target this module serves, by the type that declares it.
    ///
    /// What `controllers: [Orders]` takes on a `#[module]`, spelled for a builder.
    pub fn controller<T: crate::di::DeclaresController>(mut self) -> Self {
        self.controllers.push(Box::new(T::controller_factory()));
        self
    }

    /// Declare a provider by the type that declares it.
    ///
    /// What `providers: [Db]` takes on a `#[module]`. Use
    /// [`provider_factory`](Self::provider_factory) for a factory carrying configuration.
    pub fn provider<T: crate::di::DeclaresProvider>(mut self) -> Self {
        self.providers.push(Box::new(T::provider_factory()));
        self
    }

    /// Declare a dispatch target this module serves.
    ///
    /// What `controllers:` takes on a `#[module]`, for a module built at runtime: an integration
    /// whose target comes from a value it was configured with — a schema, a path — rather than
    /// from an attribute on a struct.
    pub fn controller_factory<F: ControllerFactory + 'static>(mut self, factory: F) -> Self {
        self.controllers.push(Box::new(factory));
        self
    }

    /// Export the slot under `T`: a type's own, a marker's, or a trait object's. The slot is named
    /// by [`token_of`](crate::di::token_of), which is how every declaration registers.
    pub fn export<T: ?Sized + 'static>(mut self) -> Self {
        self.exports.push(crate::di::token_of::<T>());
        self
    }

    /// Make this module global so its exports are available to every module without importing.
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }

    pub fn build(self) -> DynamicModule {
        let identity = derive_identity(&self.id, &self.providers);
        DynamicModule {
            identity,
            providers: Mutex::new(Some(self.providers)),
            controllers: Mutex::new(Some(self.controllers)),
            exports: self.exports,
            global: self.global,
        }
    }
}

/// Fold the providers' configuration fingerprints into the module identity.
///
/// With no fingerprints (no provider overrides `identity_hint`), the identity is the base name —
/// preserving the pre-fingerprint behavior. Hints are sorted so identity is independent of
/// provider declaration order, then hashed so configuration values (which may hold credentials)
/// never appear verbatim in a key that surfaces in logs and error messages.
fn derive_identity(base: &str, providers: &[Box<dyn ProviderFactory>]) -> ModuleIdentity {
    let mut hints: Vec<String> = providers.iter().filter_map(|p| p.identity_hint()).collect();
    if hints.is_empty() {
        return ModuleIdentity::named(base);
    }
    hints.sort();
    ModuleIdentity::named(base).fingerprinted(&hints)
}

impl DynamicModule {
    pub fn builder(id: impl Into<String>) -> DynamicModuleBuilder {
        DynamicModuleBuilder {
            id: id.into(),
            providers: Vec::new(),
            controllers: Vec::new(),
            exports: Vec::new(),
            global: false,
        }
    }
}
