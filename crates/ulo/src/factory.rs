use parking_lot::RwLock;
use std::sync::Arc;

use crate::application::UloApplication;
use crate::application_context::UloApplicationContext;
use crate::di::ModuleMetadata;
use crate::di::internal::scanner::DependencyScanner;
use crate::di::internal::{Container, InstanceLoader};
use crate::dispatch::transport::{EnhancerSet, Grpc, GuardEntry, Http, InterceptorEntry, Rpc, Ws};
use crate::enhancer::{ErrorHandler, Guard, Interceptor};
use crate::error::StartupError;
use crate::grpc::GrpcContext;
use crate::http::HttpContext;
use crate::http::middleware::Middleware;
use crate::rpc::RpcContext;
use crate::ws::WsContext;

/// Entry point for building a ulo application: registers global middleware
/// and enhancers, then constructs the DI container from a root
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
pub struct UloFactory {
    global_middleware: Vec<Arc<dyn Middleware>>,
    global_http: EnhancerSet<Http>,
    global_rpc: EnhancerSet<Rpc>,
    global_ws: EnhancerSet<Ws>,
    global_grpc: EnhancerSet<Grpc>,
}

impl UloFactory {
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
        self.global_http
            .guards
            .push(GuardEntry::<Http>::Ready(guard));
        self
    }

    /// Register a global interceptor that wraps every HTTP route handler.
    pub fn use_global_http_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<HttpContext, crate::http::HttpHandlerResult>>,
    ) -> &mut Self {
        self.global_http
            .interceptors
            .push(InterceptorEntry::<Http>::Ready(interceptor));
        self
    }

    /// Register a global HTTP error handler. Stacks with controller- and
    /// method-level handlers — the most specific is consulted first.
    pub fn use_global_http_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<HttpContext, crate::http::HttpHandlerResult>>,
    ) -> &mut Self {
        self.global_http.error_handlers.push(handler);
        self
    }

    pub fn use_global_rpc_guards(&mut self, guard: Arc<dyn Guard<RpcContext>>) -> &mut Self {
        self.global_rpc.guards.push(GuardEntry::<Rpc>::Ready(guard));
        self
    }

    pub fn use_global_rpc_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<RpcContext, crate::rpc::RpcHandlerResult>>,
    ) -> &mut Self {
        self.global_rpc
            .interceptors
            .push(InterceptorEntry::<Rpc>::Ready(interceptor));
        self
    }

    pub fn use_global_rpc_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<RpcContext, crate::rpc::RpcHandlerResult>>,
    ) -> &mut Self {
        self.global_rpc.error_handlers.push(handler);
        self
    }

    pub fn use_global_ws_guards(&mut self, guard: Arc<dyn Guard<WsContext>>) -> &mut Self {
        self.global_ws.guards.push(GuardEntry::<Ws>::Ready(guard));
        self
    }

    pub fn use_global_ws_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<WsContext, crate::ws::WsHandlerResult>>,
    ) -> &mut Self {
        self.global_ws
            .interceptors
            .push(InterceptorEntry::<Ws>::Ready(interceptor));
        self
    }

    pub fn use_global_ws_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<WsContext, crate::ws::WsHandlerResult>>,
    ) -> &mut Self {
        self.global_ws.error_handlers.push(handler);
        self
    }

    /// Register a global guard that runs on every gRPC method, ahead of the
    /// service's own and its methods'.
    pub fn use_global_grpc_guards(&mut self, guard: Arc<dyn Guard<GrpcContext>>) -> &mut Self {
        self.global_grpc
            .guards
            .push(GuardEntry::<Grpc>::Ready(guard));
        self
    }

    /// Register a global interceptor that wraps every gRPC method.
    pub fn use_global_grpc_interceptors(
        &mut self,
        interceptor: Arc<dyn Interceptor<GrpcContext, crate::grpc::GrpcHandlerResult>>,
    ) -> &mut Self {
        self.global_grpc
            .interceptors
            .push(InterceptorEntry::<Grpc>::Ready(interceptor));
        self
    }

    /// Register a global gRPC error handler. Stacks with service- and
    /// method-level handlers — the most specific is consulted first.
    pub fn use_global_grpc_error_handler(
        &mut self,
        handler: Arc<dyn ErrorHandler<GrpcContext, crate::grpc::GrpcHandlerResult>>,
    ) -> &mut Self {
        self.global_grpc.error_handlers.push(handler);
        self
    }

    /// Shorthand for `UloFactory::new().create_with(...)` when no factory config is needed
    ///
    /// # Errors
    ///
    /// See [`create_with`](Self::create_with).
    pub async fn create(
        module: impl ModuleMetadata + 'static,
    ) -> Result<UloApplication, StartupError> {
        Self::new().create_with(module).await
    }

    /// Builds the application from the root module, installing the
    /// [default logger](UloFactory#logging) first.
    ///
    /// The first of three startup phases: this one resolves the graph and confirms the outbound
    /// dependencies it declares. See [`UloApplication`] for where the
    /// other two begin.
    ///
    /// # Errors
    ///
    /// [`StartupError::Setup`] when the module graph does not resolve: an
    /// unresolvable dependency, a provider cycle, or a global-export clash
    /// between two modules. [`StartupError::BuildFailed`] when a provider or
    /// controller cannot be built, such as a singleton injecting an
    /// execution-scoped provider, naming it and its module.
    /// [`StartupError::HookFailed`] when an `on_module_init` hook returns an
    /// error, naming the module and hook.
    pub async fn create_with(
        &self,
        module: impl ModuleMetadata + 'static,
    ) -> Result<UloApplication, StartupError> {
        let container = Arc::new(RwLock::new(Container::new()));

        self.initialize(Box::new(module), container.clone()).await?;

        Ok(UloApplication::new(container))
    }

    /// Standalone DI container for CLI tools, cron jobs, and background
    /// workers. Installs the [default logger](UloFactory#logging).
    ///
    /// # Errors
    ///
    /// See [`create_application_context_with`](Self::create_application_context_with).
    pub async fn create_application_context(
        module: impl ModuleMetadata + 'static,
    ) -> Result<UloApplicationContext, StartupError> {
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
    ) -> Result<UloApplicationContext, StartupError> {
        let container = Arc::new(RwLock::new(Container::new()));

        self.initialize(Box::new(module), container.clone()).await?;

        // HTTP adapters trigger bootstrap through their own init; standalone needs it explicitly
        {
            let mut scanner =
                crate::di::internal::scanner::DependencyScanner::new(container.clone());
            scanner.call_bootstrap_hooks().await?;
        }

        Ok(UloApplicationContext::new(container))
    }

    /// A failing `on_module_init` hook arrives as [`StartupError::HookFailed`], carrying the
    /// module and hook names the scanner attached where they were in scope.
    async fn initialize(
        &self,
        module: Box<dyn ModuleMetadata>,
        container: Arc<RwLock<Container>>,
    ) -> Result<(), StartupError> {
        init_default_logger();

        tracing::debug!("Scanning module graph");
        let mut scanner = DependencyScanner::new(container.clone());

        // Register built-in global module
        scanner.scan(Box::new(crate::di::internal::builtin_module::BuiltinModule))?;

        // Scan user's root module
        scanner.scan(module)?;

        // Register global middleware
        {
            let mut container_mut = container.write();
            if let Some(middleware_manager) = container_mut.middleware_manager_mut() {
                for middleware in &self.global_middleware {
                    middleware_manager.add_global(middleware.clone());
                }
            }
        }

        // Register global enhancers
        {
            let mut container_mut = container.write();
            container_mut.global_http.extend_from(&self.global_http);
            container_mut.global_rpc.extend_from(&self.global_rpc);
            container_mut.global_ws.extend_from(&self.global_ws);
            container_mut.global_grpc.extend_from(&self.global_grpc);
        }

        scanner.scan_middleware()?;

        tracing::debug!("Instantiating dependencies");
        // Create instances of all dependencies (providers, controllers)
        InstanceLoader::new(container.clone())
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
