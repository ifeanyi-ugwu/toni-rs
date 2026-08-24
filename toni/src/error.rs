use std::error::Error;

/// Return type for lifecycle startup hooks (`on_module_init`, `on_application_bootstrap`).
///
/// Any error type implementing `std::error::Error + Send + Sync` can be returned with `?`.
/// The framework wraps failures into [`StartupError::HookFailed`] with module and hook name context
/// at the scanner layer, where that information is in scope.
pub type InitResult = Result<(), Box<dyn Error + Send + Sync + 'static>>;

/// Errors from the startup phases: building the application
/// ([`ToniFactory::create`]) and acquiring its sockets ([`ToniApplication::bind`]).
///
/// [`HookFailed`] carries the module name and hook name so callers can identify which startup
/// hook failed without inspecting the error message. [`Adapter`] names the transport that could
/// not start, so a caller can report which half of a multi-transport application is unavailable;
/// only `bind` produces it. [`Setup`] covers framework-level failures (a module graph that does
/// not resolve, no adapter registered for something the application declares, wrong call order)
/// that are typically fatal and not worth pattern-matching on.
///
/// A provider whose construction fails panics rather than arriving here: [`ProviderFactory::build`]
/// returns the instance directly, so a factory that cannot build one has nowhere to put an error.
///
/// [`HookFailed`]: StartupError::HookFailed
/// [`Adapter`]: StartupError::Adapter
/// [`Setup`]: StartupError::Setup
/// [`ToniFactory::create`]: crate::ToniFactory::create
/// [`ToniApplication::bind`]: crate::ToniApplication::bind
/// [`ProviderFactory::build`]: crate::traits_helpers::ProviderFactory::build
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

impl From<anyhow::Error> for StartupError {
    fn from(e: anyhow::Error) -> Self {
        Self::Setup(e.into())
    }
}
