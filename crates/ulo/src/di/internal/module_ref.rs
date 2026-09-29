use std::sync::Arc;

use parking_lot::RwLock;

use crate::error::ResolutionError;
use rustc_hash::FxHashMap;

use crate::di::Execution;
use crate::di::Key;
use crate::spi::Provider;
/// What a [`ModuleRef`] reads: each module's own instances, and the global registry.
#[derive(Default)]
pub(crate) struct ProviderStore {
    pub(crate) modules: FxHashMap<String, FxHashMap<String, Arc<Box<dyn Provider>>>>,
    /// What the global modules export, the registry `#[module(global: true)]` fills.
    pub(crate) globals: FxHashMap<String, Arc<Box<dyn Provider>>>,
}

/// Provides runtime dependency resolution within a module context
///
/// `ModuleRef` is scoped to a specific module and allows dynamic resolution
/// of providers at runtime. A lookup reads the module's own providers, and with
/// `.or_global()` falls back to what the global modules export; no other module's
/// provider is reached, an import's export included.
///
/// [`get`](Self::get) builds outside any execution, which limits it to providers
/// that can exist there. An execution-scoped provider cannot, so it is reached with
/// [`resolve`](Self::resolve), which builds in an execution you hand it.
///
/// # Examples
///
/// ```ignore
/// #[injectable]
/// pub struct PluginLoader {
///     #[inject]
///     module_ref: ModuleRef,
/// }
/// impl PluginLoader {
///     pub async fn load_plugin(&self) {
///         // Strict mode (default): only search current module
///         let plugin = self.module_ref.get_key::<PrimaryPlugin>().await?;
///
///         // The current module first, then what the global modules export
///         let config = self.module_ref.get::<Config>().or_global().await?;
///     }
/// }
/// ```
#[derive(Clone)]
pub struct ModuleRef {
    module_token: String,
    store: Arc<RwLock<ProviderStore>>,
}

impl std::fmt::Debug for ModuleRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleRef")
            .field("module", &self.module_token)
            .finish_non_exhaustive()
    }
}

impl ModuleRef {
    pub(crate) fn new(module_token: String, store: Arc<RwLock<ProviderStore>>) -> Self {
        Self {
            module_token,
            store,
        }
    }

    /// Get a provider instance by its type
    ///
    /// By default, searches only the current module. `.or_global()` falls back to what the global
    /// modules export.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // The current module only
    /// let service = module_ref.get::<MyService>().await?;
    ///
    /// // The current module, then the global registry
    /// let shared = module_ref.get::<SharedService>().or_global().await?;
    /// ```
    pub fn get<T: 'static>(&self) -> ModuleRefQuery<'_, T> {
        ModuleRefQuery {
            module_ref: self,
            token: std::any::type_name::<T>().to_string(),
            or_global: false,
            execution: Execution::None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get the value under the marker `K`. Search mode works as it does for
    /// [`get`](Self::get). A slot holding a trait object is not reached here; a field reads it
    /// through `#[inject(K)]`.
    pub fn get_key<K>(&self) -> ModuleRefQuery<'_, K::Value>
    where
        K: Key,
        K::Value: Sized,
    {
        ModuleRefQuery {
            module_ref: self,
            token: crate::di::token_of::<K>(),
            or_global: false,
            execution: Execution::None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Resolve a provider by its type in an execution
    ///
    /// Reaches what [`get`](Self::get) cannot: an execution-scoped provider is built
    /// into the execution's cache, so resolving one twice in the same execution
    /// returns the instance the handler is holding rather than a second one.
    ///
    /// Any execution will do — an HTTP request, a WebSocket message, an RPC or
    /// gRPC call. Search mode works as it does for `get`: current module only,
    /// or `.or_global()` for the fallback.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // In a guard, an interceptor, or anywhere the context reaches:
    /// let execution: Execution = ctx.clone().into();
    /// let audit = module_ref.resolve::<AuditLog>(&execution).await?;
    /// ```
    pub fn resolve<T: 'static>(&self, execution: &Execution) -> ModuleRefQuery<'_, T> {
        ModuleRefQuery {
            module_ref: self,
            token: std::any::type_name::<T>().to_string(),
            or_global: false,
            execution: execution.clone(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Resolve the value under the marker `K` in an execution. See
    /// [`resolve`](Self::resolve) and [`get_key`](Self::get_key).
    pub fn resolve_key<K>(&self, execution: &Execution) -> ModuleRefQuery<'_, K::Value>
    where
        K: Key,
        K::Value: Sized,
    {
        ModuleRefQuery {
            module_ref: self,
            token: crate::di::token_of::<K>(),
            or_global: false,
            execution: execution.clone(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get the current module's token
    pub fn current_module(&self) -> &str {
        &self.module_token
    }
}

/// A [`ModuleRef`] lookup, run by awaiting it.
pub struct ModuleRefQuery<'a, T: 'static> {
    module_ref: &'a ModuleRef,
    token: String,
    or_global: bool,
    /// The execution to build in; `None` for a `get`, which has none.
    execution: Execution,
    _phantom: std::marker::PhantomData<T>,
}

impl<'a, T: 'static> ModuleRefQuery<'a, T> {
    /// Fall back to the global registry: the current module first, then what the global modules
    /// export. No other module's provider is reached, whether exported to an importer or kept
    /// private.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let service = module_ref.get::<Service>().or_global().await?;
    /// ```
    pub fn or_global(mut self) -> Self {
        self.or_global = true;
        self
    }

    /// Execute the query and return the provider instance
    pub async fn execute(self) -> Result<T, ResolutionError>
    where
        T: Send,
    {
        let provider_instance = {
            let store = self.module_ref.store.read();
            let local = store
                .modules
                .get(&self.module_ref.module_token)
                .and_then(|m| m.get(&self.token))
                .cloned();
            let found = match local {
                Some(instance) => Some(instance),
                None if self.or_global => store.globals.get(&self.token).cloned(),
                None => None,
            };
            found.ok_or_else(|| ResolutionError::ProviderNotFound {
                token: self.token.clone(),
                module: Some(self.module_ref.module_token.clone()),
            })?
        };

        self.execution
            .ensure_can_build(provider_instance.scope(), &self.token)?;

        provider_instance
            .resolve(self.execution.clone())
            .await
            .downcast::<T>()
            .map(|boxed| *boxed)
            .map_err(|_| ResolutionError::TypeMismatch {
                token: self.token.clone(),
            })
    }
}

/// Awaiting the query runs it, so a lookup reads as `module.get::<T>().await`.
impl<'a, T: 'static + Send> std::future::IntoFuture for ModuleRefQuery<'a, T> {
    type Output = Result<T, ResolutionError>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.execute())
    }
}
