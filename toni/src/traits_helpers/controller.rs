use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::http_helpers::{ExecutionResult, HttpMethod, HttpRequest, RequestPart, RouteMetadata};

use crate::context::HttpContext;

use super::{
    Guard, HttpErrorHandlerArc, Interceptor, Pipe, provider::Provider, validate::Validatable,
};

/// Per-route enhancer manifest — both DI-resolved tokens and direct-instantiation arcs.
///
/// `*_tokens` come from `#[use_guards(MyGuard)]`-style attributes that resolve via
/// the DI container; `guards` / `interceptors` / `pipes` / `error_handlers` come
/// from `#[use_guards(MyGuard{})]`-style attributes that bypass DI and instantiate
/// the enhancer inline.
#[derive(Default)]
pub struct ControllerEnhancers {
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub pipe_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub guards: Vec<Arc<dyn Guard<HttpContext>>>,
    pub interceptors: Vec<Arc<dyn Interceptor<HttpContext>>>,
    pub pipes: Vec<Arc<dyn Pipe<HttpContext>>>,
    pub error_handlers: Vec<HttpErrorHandlerArc>,
}

#[async_trait]
pub trait Controller: Send + Sync {
    fn get_token(&self) -> String;
    /// Run the user handler and return either the rendered success response
    /// or the user's typed error preserved for the dispatcher's observer +
    /// chain pipeline.
    async fn execute(&self, req: HttpRequest) -> ExecutionResult;
    fn get_path(&self) -> String;
    fn get_method(&self) -> HttpMethod;

    fn enhancers(&self) -> ControllerEnhancers {
        ControllerEnhancers::default()
    }

    /// Get route metadata (roles, permissions, custom config)
    fn get_route_metadata(&self) -> Arc<RouteMetadata> {
        Arc::new(RouteMetadata::new())
    }

    fn get_body_dto(&self, _req: &RequestPart) -> Option<Box<dyn Validatable>>;

    // Lifecycle Hooks

    /// Returns the controller struct's type name, used to deduplicate lifecycle hook calls
    /// across per-route wrapper structs that share the same underlying controller instance.
    /// Returns an empty string for wrappers that have no lifecycle hooks.
    fn get_controller_type_name(&self) -> &'static str {
        ""
    }

    async fn on_module_init(&self) -> crate::InitResult { Ok(()) }
    async fn on_application_bootstrap(&self) -> crate::InitResult { Ok(()) }
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_module_destroy(&self) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}
#[async_trait]
pub trait ControllerFactory {
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String> {
        vec![]
    }
    async fn build(
        &self,
        deps: FxHashMap<String, Arc<Box<dyn Provider>>>,
    ) -> Vec<Arc<Box<dyn Controller>>>;
}
