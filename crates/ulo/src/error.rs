use std::error::Error;

/// Return type for lifecycle startup hooks (`on_module_init`, `on_application_bootstrap`).
///
/// Any error type implementing `std::error::Error + Send + Sync` can be returned with `?`.
/// The framework wraps failures into [`StartupError::HookFailed`] with module and hook name context
/// at the scanner layer, where that information is in scope.
pub type InitResult = Result<(), Box<dyn Error + Send + Sync + 'static>>;

/// Return type for the transport adapter SPI — [`HttpAdapter`], [`WsAdapter`],
/// [`RpcAdapter`] and [`GrpcAdapter`].
///
/// Any error type implementing `std::error::Error + Send + Sync` can be returned with `?`, and
/// `format!("…").into()` covers a failure that has no type of its own. The framework wraps
/// whatever arrives into [`StartupError::Adapter`] with the transport name attached, at the layer
/// holding it, and reads nothing else off the value.
///
/// [`HttpAdapter`]: crate::http::HttpAdapter
/// [`WsAdapter`]: crate::ws::WsAdapter
/// [`RpcAdapter`]: crate::rpc::RpcAdapter
/// [`GrpcAdapter`]: crate::grpc::GrpcAdapter
pub type AdapterResult<T = ()> = Result<T, Box<dyn Error + Send + Sync + 'static>>;

/// Return type for building a provider or a controller — [`ProviderFactory::build`] and
/// [`ControllerFactory::build`].
///
/// A build that fails ends startup: the loader wraps the error into [`StartupError::BuildFailed`]
/// with the token and module attached, and reads nothing else off the value. A dependency that
/// cannot be resolved arrives as a [`ResolutionError`], which `?` converts.
///
/// [`ProviderFactory::build`]: crate::spi::ProviderFactory::build
/// [`ControllerFactory::build`]: crate::dispatch::ControllerFactory::build
pub type BuildResult<T> = Result<T, Box<dyn Error + Send + Sync + 'static>>;

/// Return type for the DI setup surface — the container, the instance loader, the dependency
/// graph and the dispatch-target resolvers, none of which a caller reaches directly.
///
/// These build the module graph, and what they report is the graph being unbuildable: a provider
/// nothing exports, a module that is not imported, a cycle. A caller has no recovery to choose
/// between, which is why the type names no cases and [`StartupError::Setup`] is where every one of
/// them arrives.
pub(crate) type SetupResult<T = ()> = Result<T, Box<dyn Error + Send + Sync + 'static>>;

/// Errors from the startup phases: building the application
/// ([`UloFactory::create`]) and acquiring its sockets ([`UloApplication::bind`]).
///
/// [`HookFailed`] carries the module name and hook name so callers can identify which startup
/// hook failed without inspecting the error message. [`BuildFailed`] names the provider or
/// controller whose build failed and its module. [`Adapter`] names the transport that could not
/// start, so a caller can report which half of a multi-transport application is unavailable;
/// only `bind` produces it. [`Setup`] covers framework-level failures (a module graph that does
/// not resolve, no adapter registered for something the application declares, wrong call order)
/// that are typically fatal and not worth pattern-matching on.
///
/// [`HookFailed`]: StartupError::HookFailed
/// [`BuildFailed`]: StartupError::BuildFailed
/// [`Adapter`]: StartupError::Adapter
/// [`Setup`]: StartupError::Setup
/// [`UloFactory::create`]: crate::UloFactory::create
/// [`UloApplication::bind`]: crate::UloApplication::bind
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StartupError {
    #[error("hook `{hook}` failed in module `{module}`: {source}")]
    HookFailed {
        module: String,
        hook: &'static str,
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    /// A factory's `build` failed. `token` is the provider or controller it builds; `source` is
    /// what the build answered, a [`ResolutionError`] when a dependency could not be resolved,
    /// such as an execution-scoped one injected into a provider built once at startup.
    #[error("`{token}` in module `{module}` could not be built: {source}")]
    BuildFailed {
        module: String,
        token: String,
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    /// `transport` is one of `http`, `websocket`, `rpc`, `grpc`.
    #[error("{transport} adapter failed to start: {source}")]
    Adapter {
        transport: &'static str,
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    #[error("{0}")]
    Setup(Box<dyn Error + Send + Sync + 'static>),
}

/// A `StartupError` boxed on its way through the setup surface arrives as itself; anything else
/// is a [`StartupError::Setup`].
impl From<Box<dyn Error + Send + Sync + 'static>> for StartupError {
    fn from(source: Box<dyn Error + Send + Sync + 'static>) -> Self {
        match source.downcast::<StartupError>() {
            Ok(startup) => *startup,
            Err(source) => Self::Setup(source),
        }
    }
}

/// Errors from pulling a provider or a module handle out of the container.
///
/// Produced by the resolution methods on [`UloApplication`], [`UloApplicationContext`] and
/// [`ModuleRef`], and by [`Provider::resolve`], whose failure anything injecting that provider
/// passes on.
/// Every variant carries what a caller needs to act on it rather than only to
/// report it: [`ProviderNotFound`] says which module a lookup searched, or that it searched all of
/// them, [`AmbiguousModule`] hands back the full keys that [`get_module_by_id`] accepts, and
/// [`ExecutionRequired`] names the provider that needs an execution to be built in.
///
/// [`ProviderNotFound`]: ResolutionError::ProviderNotFound
/// [`AmbiguousModule`]: ResolutionError::AmbiguousModule
/// [`ExecutionRequired`]: ResolutionError::ExecutionRequired
/// [`UloApplication`]: crate::UloApplication
/// [`UloApplicationContext`]: crate::application_context::UloApplicationContext
/// [`ModuleRef`]: crate::di::internal::ModuleRef
/// [`get_module_by_id`]: crate::application_context::UloApplicationContext::get_module_by_id
/// [`Provider::resolve`]: crate::spi::Provider::resolve
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ResolutionError {
    /// `module` is the module searched, or `None` where every module was. A `ModuleRef` lookup
    /// with `.visible()` names its own module, its imports' exports and the global registry having
    /// been read after it.
    #[error("provider `{token}` not found {}", searched_in(.module))]
    ProviderNotFound {
        token: String,
        module: Option<String>,
    },

    /// No module carries `id` as an identity key or base.
    #[error(
        "no module has identity `{id}`. The module is not imported, or its identity base \
         differs — a `DynamicModule`'s base is the name its builder was given."
    )]
    ModuleNotFound { id: String },

    /// Two or more modules answer one lookup: modules sharing the identity base `base`, modules
    /// each holding the token `base` that a search across every module asked for, or imports each
    /// exporting the token `base` into the module a `.visible()` lookup ran in.
    /// `candidates` holds their full keys, each of which
    /// [`get_module_by_id`](crate::application_context::UloApplicationContext::get_module_by_id)
    /// resolves on its own.
    #[error(
        "`{base}` is ambiguous: {} each answer it. Pass one full key to `get_module_by_id` and \
         resolve through that module.",
        candidate_list(.candidates)
    )]
    AmbiguousModule {
        base: String,
        candidates: Vec<String>,
    },

    /// The provider registered under `token` is not the requested type.
    #[error("provider `{token}` is not the requested type")]
    TypeMismatch { token: String },

    /// The provider registered under `token` hands out one shared instance, and a field or
    /// parameter written as the plain type `wrote` would hold a copy of it.
    #[error(
        "provider `{token}` hands out one shared instance, which a field or parameter written \
         `{wrote}` cannot hold: write `Arc<{wrote}>`"
    )]
    SharedByValue { token: String, wrote: String },

    /// An execution-scoped provider lives in an execution's cache, and there is nowhere to put one
    /// without an execution. `token` names that provider, or the key a stand-in for it is bound
    /// under, whether it was asked for or reached through something injecting it: a transient
    /// resolved outside an execution, or a provider built once at startup. Resolve it with
    /// `resolve` on the application or on a [`ModuleRef`], passing [`Execution::standalone`] where
    /// the work arrived over no transport.
    ///
    /// [`ModuleRef`]: crate::di::internal::ModuleRef
    /// [`Execution::standalone`]: crate::di::Execution::standalone
    #[error(
        "provider `{token}` is execution-scoped and is built only inside an execution; a provider \
         built once at startup cannot inject it, and a lookup reaches it with `resolve` in an \
         execution (`Execution::standalone()` where there is no transport)."
    )]
    ExecutionRequired { token: String },
}

fn searched_in(module: &Option<String>) -> String {
    match module {
        Some(module) => format!("in module `{module}`"),
        None => "in any module".to_string(),
    }
}

fn candidate_list(candidates: &[String]) -> String {
    candidates
        .iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl From<ResolutionError> for StartupError {
    fn from(e: ResolutionError) -> Self {
        Self::Setup(Box::new(e))
    }
}
