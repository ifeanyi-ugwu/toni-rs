use std::sync::Arc;

use parking_lot::RwLock;

use crate::error::ResolutionError;
use rustc_hash::FxHashMap;

use crate::di::Execution;
use crate::di::Key;
use crate::spi::Provider;
/// Each token a module's imports export into it, with every import declaring the export and its
/// instance, `None` where the import declares a token it does not build.
pub(crate) type ImportedExports = FxHashMap<String, Vec<(String, Option<Arc<dyn Provider>>)>>;

/// What a [`ModuleRef`] reads: each module's own instances, what its imports export into it, and
/// the global registry.
#[derive(Default)]
pub(crate) struct ProviderStore {
    pub(crate) modules: FxHashMap<String, FxHashMap<String, Arc<dyn Provider>>>,
    pub(crate) imports: FxHashMap<String, ImportedExports>,
    /// What the global modules export, the registry `#[module(global: true)]` fills.
    pub(crate) globals: FxHashMap<String, Arc<dyn Provider>>,
}

/// Provides runtime dependency resolution within a module context
///
/// `ModuleRef` is scoped to a specific module and allows dynamic resolution
/// of providers at runtime. A lookup reads the module's own providers, and with
/// `.visible()` everything visible to the module: its own providers, what its
/// imports export, then what the global modules export. A provider exported
/// neither to this module nor globally is not reached, and neither is a collection.
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
///         // The current module only (the default)
///         let plugin = self.module_ref.get_key::<PrimaryPlugin>().await?;
///
///         // Everything visible to the module: its own, its imports' exports, the globals
///         let config = self.module_ref.get::<Config>().visible().await?;
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
    /// By default, searches only the current module. `.visible()` searches everything visible to
    /// the module: its own providers, its imports' exports, then what the global modules export.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // The current module only
    /// let service = module_ref.get::<MyService>().await?;
    ///
    /// // Everything visible to the module
    /// let shared = module_ref.get::<SharedService>().visible().await?;
    /// ```
    pub fn get<T: 'static>(&self) -> ModuleRefQuery<'_, T> {
        ModuleRefQuery {
            module_ref: self,
            token: crate::di::token_of::<T>(),
            visible: false,
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
            visible: false,
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
    /// gRPC call. It searches as `get` does: the current module only, or
    /// everything visible to it with `.visible()`.
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
            token: crate::di::token_of::<T>(),
            visible: false,
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
            visible: false,
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
    visible: bool,
    /// The execution to build in; `None` for a `get`, which has none.
    execution: Execution,
    _phantom: std::marker::PhantomData<T>,
}

impl<'a, T: 'static> ModuleRefQuery<'a, T> {
    /// Reach what an `#[inject]` field naming one binding in this module would: the module's own
    /// providers, then what its imports export into it, then what the global modules export. A key
    /// two imports export answers [`ResolutionError::AmbiguousModule`] naming each exporting
    /// module. A provider exported neither to this module nor globally is not reached, and neither
    /// is a collection.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let service = module_ref.get::<Service>().visible().await?;
    /// ```
    pub fn visible(mut self) -> Self {
        self.visible = true;
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
                None if self.visible => match store
                    .imports
                    .get(&self.module_ref.module_token)
                    .and_then(|imported| imported.get(&self.token))
                    .map(Vec::as_slice)
                {
                    // The one import declaring it and not building it leaves nothing to answer,
                    // as it leaves an injection nothing to inject.
                    Some([(_, instance)]) => instance.clone(),
                    Some(exporters @ [_, _, ..]) => {
                        let mut candidates: Vec<String> =
                            exporters.iter().map(|(module, _)| module.clone()).collect();
                        candidates.sort();
                        return Err(ResolutionError::AmbiguousModule {
                            base: self.token.clone(),
                            candidates,
                        });
                    }
                    _ => store.globals.get(&self.token).cloned(),
                },
                None => None,
            };
            found.ok_or_else(|| ResolutionError::ProviderNotFound {
                token: self.token.clone(),
                module: Some(self.module_ref.module_token.clone()),
            })?
        };

        provider_instance
            .resolve(self.execution.clone())
            .await?
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
