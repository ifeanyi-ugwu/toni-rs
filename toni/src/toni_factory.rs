use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;

use crate::application_context::ToniApplicationContext;
use crate::context::Metadata;
use crate::context::{HttpContext, RpcContext, WsContext};
use crate::error::StartupError;
use crate::http_helpers::HttpResponse;
use crate::injector::{ToniContainer, ToniInstanceLoader};
use crate::middleware::Middleware;
use crate::rpc::RpcData;
use crate::scanner::ToniDependenciesScanner;
use crate::toni_application::ToniApplication;
use crate::traits_helpers::{
    ErrorHandler, ErrorObserver, Guard, HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry,
    HttpPipeEntry, Interceptor, ModuleMetadata, Pipe, RpcErrorHandlerArc, RpcGuardEntry,
    RpcInterceptorEntry, RpcPipeEntry, WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry,
    WsPipeEntry,
};
use crate::websocket::WsMessage;

/// Entry point for building a toni application: registers global middleware,
/// enhancers, and observers, then constructs the DI container from a root
/// module via [`create_with`](Self::create_with) or
/// [`create_application_context_with`](Self::create_application_context_with).
///
/// # Logging
///
/// Application creation installs a default logging subscriber unless a global
/// `tracing` subscriber is already set — a subscriber installed before the
/// `create` call always wins. The default writes to stderr, keeping stdout
/// free for program output, and is filtered by `RUST_LOG` with an `info`
/// fallback; `RUST_LOG=off` silences it at runtime. Disabling the crate's
/// default `logger` feature compiles it out.
#[derive(Default)]
pub struct ToniFactory {
    global_middleware: Vec<Arc<dyn Middleware>>,
    global_http_guards: Vec<HttpGuardEntry>,
    global_http_interceptors: Vec<HttpInterceptorEntry>,
    global_http_pipes: Vec<HttpPipeEntry>,
    global_http_error_handlers: Vec<HttpErrorHandlerArc>,
    global_rpc_guards: Vec<RpcGuardEntry>,
    global_rpc_interceptors: Vec<RpcInterceptorEntry>,
    global_rpc_pipes: Vec<RpcPipeEntry>,
    global_rpc_error_handlers: Vec<RpcErrorHandlerArc>,
    global_ws_guards: Vec<WsGuardEntry>,
    global_ws_interceptors: Vec<WsInterceptorEntry>,
    global_ws_pipes: Vec<WsPipeEntry>,
    global_ws_error_handlers: Vec<WsErrorHandlerArc>,
    global_error_observers: Vec<Arc<dyn ErrorObserver>>,
}

impl ToniFactory {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn use_global_middleware(&mut self, middleware: Arc<dyn Middleware>) -> &mut Self {
        self.global_middleware.push(middleware);
        self
    }

    /// Register a global guard that runs on every HTTP route.
    pub fn use_global_http_guards(&mut self, guard: Arc<dyn Guard<HttpContext>>) -> &mut Self {
        self.global_http_guards.push(HttpGuardEntry::Ready(guard));
        self
    }

    /// Register a global interceptor that wraps every HTTP route handler.
    pub fn use_global_http_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<HttpContext, crate::http_helpers::HttpResponse>>,
    ) -> &mut Self {
        self.global_http_interceptors
            .push(HttpInterceptorEntry::Ready(interceptor));
        self
    }

    /// Register a global pipe that runs on every HTTP route.
    pub fn use_global_http_pipes(
        &mut self,
        pipe: Arc<dyn Pipe<HttpContext, crate::http_helpers::HttpResponse>>,
    ) -> &mut Self {
        self.global_http_pipes.push(HttpPipeEntry::Ready(pipe));
        self
    }

    /// Register a global HTTP error handler. Stacks with controller- and
    /// method-level handlers — the most specific is consulted first.
    pub fn use_global_http_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<HttpContext, HttpResponse>>,
    ) -> &mut Self {
        self.global_http_error_handlers.push(handler);
        self
    }

    pub fn use_global_rpc_guards(&mut self, guard: Arc<dyn Guard<RpcContext>>) -> &mut Self {
        self.global_rpc_guards.push(RpcGuardEntry::Ready(guard));
        self
    }

    pub fn use_global_rpc_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<RpcContext, crate::rpc::RpcHandlerResult>>,
    ) -> &mut Self {
        self.global_rpc_interceptors
            .push(RpcInterceptorEntry::Ready(interceptor));
        self
    }

    pub fn use_global_rpc_pipes(
        &mut self,
        pipe: Arc<dyn Pipe<RpcContext, crate::rpc::RpcHandlerResult>>,
    ) -> &mut Self {
        self.global_rpc_pipes.push(RpcPipeEntry::Ready(pipe));
        self
    }

    pub fn use_global_rpc_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<RpcContext, RpcData>>,
    ) -> &mut Self {
        self.global_rpc_error_handlers.push(handler);
        self
    }

    pub fn use_global_ws_guards(&mut self, guard: Arc<dyn Guard<WsContext>>) -> &mut Self {
        self.global_ws_guards.push(WsGuardEntry::Ready(guard));
        self
    }

    pub fn use_global_ws_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<WsContext, crate::websocket::WsHandlerResult>>,
    ) -> &mut Self {
        self.global_ws_interceptors
            .push(WsInterceptorEntry::Ready(interceptor));
        self
    }

    pub fn use_global_ws_pipes(
        &mut self,
        pipe: Arc<dyn Pipe<WsContext, crate::websocket::WsHandlerResult>>,
    ) -> &mut Self {
        self.global_ws_pipes.push(WsPipeEntry::Ready(pipe));
        self
    }

    pub fn use_global_ws_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<WsContext, WsMessage>>,
    ) -> &mut Self {
        self.global_ws_error_handlers.push(handler);
        self
    }

    /// Register a transport-agnostic observer that fires whenever a
    /// framework-generated error reaches the chain (guard rejections,
    /// missing routes, panic recovery). Observers are fire-and-forget
    /// — they don't shape the response.
    ///
    /// User-handler errors render directly through the active transport and
    /// don't pass through observers; if you need to log those, override
    /// the rendering method on your error type.
    pub fn use_global_error_observer(&mut self, observer: Arc<dyn ErrorObserver>) -> &mut Self {
        self.global_error_observers.push(observer);
        self
    }

    /// Shorthand for `ToniFactory::new().create_with(...)` when no factory config is needed
    ///
    /// # Errors
    ///
    /// See [`create_with`](Self::create_with).
    pub async fn create(
        module: impl ModuleMetadata + 'static,
    ) -> Result<ToniApplication, StartupError> {
        Self::new().create_with(module).await
    }

    /// Builds the application from the root module, installing the
    /// [default logger](ToniFactory#logging) first.
    ///
    /// # Errors
    ///
    /// [`StartupError::Setup`] when the module graph does not resolve: an
    /// unresolvable dependency, a provider cycle, or a global-export clash
    /// between two modules. [`StartupError::HookFailed`] when an
    /// `on_module_init` hook returns an error, naming the module and hook.
    ///
    /// A provider whose factory cannot build its instance panics instead —
    /// `ProviderFactory::build` returns the instance directly and has nowhere
    /// to put an error, so a database module that cannot connect ends the
    /// process here rather than returning.
    pub async fn create_with(
        &self,
        module: impl ModuleMetadata + 'static,
    ) -> Result<ToniApplication, StartupError> {
        let container = Rc::new(RefCell::new(ToniContainer::new()));

        self.initialize(Box::new(module), container.clone()).await?;

        Ok(ToniApplication::new(container))
    }

    /// Standalone DI container for CLI tools, cron jobs, and background
    /// workers. Installs the [default logger](ToniFactory#logging).
    ///
    /// # Errors
    ///
    /// See [`create_application_context_with`](Self::create_application_context_with).
    pub async fn create_application_context(
        module: impl ModuleMetadata + 'static,
    ) -> Result<ToniApplicationContext, StartupError> {
        Self::new().create_application_context_with(module).await
    }

    /// # Errors
    ///
    /// Everything [`create_with`](Self::create_with) reports, plus
    /// [`StartupError::HookFailed`] for an `on_application_bootstrap` hook.
    /// An application binds its adapters to reach that phase; a standalone
    /// context has no bind, so it runs those hooks here.
    pub async fn create_application_context_with(
        &self,
        module: impl ModuleMetadata + 'static,
    ) -> Result<ToniApplicationContext, StartupError> {
        let container = Rc::new(RefCell::new(ToniContainer::new()));

        self.initialize(Box::new(module), container.clone()).await?;

        // HTTP adapters trigger bootstrap through their own init; standalone needs it explicitly
        {
            let mut scanner = crate::scanner::ToniDependenciesScanner::new(container.clone());
            scanner.call_bootstrap_hooks().await?;
        }

        Ok(ToniApplicationContext::new(container))
    }

    /// Returns `StartupError` rather than `anyhow::Error` so that a failing
    /// `on_module_init` hook keeps its `HookFailed` variant — `?` into an
    /// `anyhow::Error` would erase the module and hook names the scanner
    /// attached.
    async fn initialize(
        &self,
        module: Box<dyn ModuleMetadata>,
        container: Rc<RefCell<ToniContainer>>,
    ) -> Result<(), StartupError> {
        init_default_logger();

        tracing::debug!("Scanning module graph");
        let mut scanner = ToniDependenciesScanner::new(container.clone());

        // Register built-in global module
        scanner.scan(Box::new(crate::builtin_module::BuiltinModule))?;

        // Scan user's root module
        scanner.scan(module)?;

        // Register global middleware
        {
            let mut container_mut = container.borrow_mut();
            if let Some(middleware_manager) = container_mut.get_middleware_manager_mut() {
                for middleware in &self.global_middleware {
                    middleware_manager.add_global(middleware.clone());
                }
            }
        }

        // Register global enhancers
        {
            let mut container_mut = container.borrow_mut();
            for guard in &self.global_http_guards {
                container_mut.add_global_http_guard(guard.clone());
            }
            for interceptor in &self.global_http_interceptors {
                container_mut.add_global_http_interceptor(interceptor.clone());
            }
            for pipe in &self.global_http_pipes {
                container_mut.add_global_http_pipe(pipe.clone());
            }
            for handler in &self.global_http_error_handlers {
                container_mut.add_global_http_error_handler(handler.clone());
            }
            for guard in &self.global_rpc_guards {
                container_mut.add_global_rpc_guard(guard.clone());
            }
            for interceptor in &self.global_rpc_interceptors {
                container_mut.add_global_rpc_interceptor(interceptor.clone());
            }
            for pipe in &self.global_rpc_pipes {
                container_mut.add_global_rpc_pipe(pipe.clone());
            }
            for handler in &self.global_rpc_error_handlers {
                container_mut.add_global_rpc_error_handler(handler.clone());
            }
            for guard in &self.global_ws_guards {
                container_mut.add_global_ws_guard(guard.clone());
            }
            for interceptor in &self.global_ws_interceptors {
                container_mut.add_global_ws_interceptor(interceptor.clone());
            }
            for pipe in &self.global_ws_pipes {
                container_mut.add_global_ws_pipe(pipe.clone());
            }
            for handler in &self.global_ws_error_handlers {
                container_mut.add_global_ws_error_handler(handler.clone());
            }
            for observer in &self.global_error_observers {
                container_mut.add_global_error_observer(observer.clone());
            }
        }

        scanner.scan_middleware()?;

        tracing::debug!("Instantiating dependencies");
        // Create instances of all dependencies (providers, controllers)
        ToniInstanceLoader::new(container.clone())
            .create_instances_of_dependencies()
            .await?;

        tracing::debug!("Running module lifecycle hooks");
        // Hooks run after all providers are instantiated, not during scanning
        scanner.call_lifecycle_hooks().await?;

        Ok(())
    }
}

/// Runs before the module graph is touched so init failures are visible even
/// when the application configures no logging of its own.
fn init_default_logger() {
    #[cfg(feature = "logger")]
    {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        // try_init fails when a global subscriber is already installed —
        // the application's subscriber wins.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    }
}
