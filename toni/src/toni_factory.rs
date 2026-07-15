use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;

use crate::application_context::ToniApplicationContext;
use crate::context::{HttpContext, RpcContext, WsContext};
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
        interceptor: Arc<dyn Interceptor<HttpContext>>,
    ) -> &mut Self {
        self.global_http_interceptors
            .push(HttpInterceptorEntry::Ready(interceptor));
        self
    }

    /// Register a global pipe that runs on every HTTP route.
    pub fn use_global_http_pipes(&mut self, pipe: Arc<dyn Pipe<HttpContext>>) -> &mut Self {
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
        interceptor: Arc<dyn Interceptor<RpcContext>>,
    ) -> &mut Self {
        self.global_rpc_interceptors
            .push(RpcInterceptorEntry::Ready(interceptor));
        self
    }

    pub fn use_global_rpc_pipes(&mut self, pipe: Arc<dyn Pipe<RpcContext>>) -> &mut Self {
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
        interceptor: Arc<dyn Interceptor<WsContext>>,
    ) -> &mut Self {
        self.global_ws_interceptors
            .push(WsInterceptorEntry::Ready(interceptor));
        self
    }

    pub fn use_global_ws_pipes(&mut self, pipe: Arc<dyn Pipe<WsContext>>) -> &mut Self {
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
    pub async fn create(module: impl ModuleMetadata + 'static) -> ToniApplication {
        Self::new().create_with(module).await
    }

    pub async fn create_with(&self, module: impl ModuleMetadata + 'static) -> ToniApplication {
        let container = Rc::new(RefCell::new(ToniContainer::new()));

        match self.initialize(Box::new(module), container.clone()).await {
            Ok(_) => (),
            Err(e) => {
                tracing::error!(error = %e, "Critical error during module initialization");
                std::process::exit(1);
            }
        };

        ToniApplication::new(container)
    }

    /// Standalone DI container for CLI tools, cron jobs, and background workers
    pub async fn create_application_context(
        module: impl ModuleMetadata + 'static,
    ) -> ToniApplicationContext {
        Self::new().create_application_context_with(module).await
    }

    pub async fn create_application_context_with(
        &self,
        module: impl ModuleMetadata + 'static,
    ) -> ToniApplicationContext {
        let container = Rc::new(RefCell::new(ToniContainer::new()));

        match self.initialize(Box::new(module), container.clone()).await {
            Ok(_) => (),
            Err(e) => {
                tracing::error!(error = %e, "Critical error during module initialization");
                std::process::exit(1);
            }
        };

        // HTTP adapters trigger bootstrap through their own init; standalone needs it explicitly
        {
            let mut scanner = crate::scanner::ToniDependenciesScanner::new(container.clone());
            if let Err(e) = scanner.call_bootstrap_hooks().await {
                tracing::error!(error = %e, "Bootstrap hooks failed");
            }
        }

        ToniApplicationContext::new(container)
    }

    async fn initialize(
        &self,
        module: Box<dyn ModuleMetadata>,
        container: Rc<RefCell<ToniContainer>>,
    ) -> Result<()> {
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
