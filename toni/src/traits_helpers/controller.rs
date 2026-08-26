use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::context::Metadata;
use crate::errors::HttpError;
use crate::http_helpers::{ExecutionResult, HttpMethod, HttpResponse};

use crate::context::HttpContext;

use super::{Guard, HttpErrorHandlerArc, Interceptor, provider::Provider};

/// Per-route enhancer manifest — both DI-resolved tokens and direct-instantiation arcs.
///
/// `*_tokens` come from `#[use_guards(MyGuard)]`-style attributes that resolve via
/// the DI container; `guards` / `interceptors` / `error_handlers` come
/// from `#[use_guards(MyGuard{})]`-style attributes that bypass DI and instantiate
/// the enhancer inline.
#[derive(Default)]
pub struct ControllerEnhancers {
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub guards: Vec<Arc<dyn Guard<HttpContext>>>,
    pub interceptors: Vec<Arc<dyn Interceptor<HttpContext, HttpResponse>>>,
    pub error_handlers: Vec<HttpErrorHandlerArc>,
}

/// How a controller's instance is held for the lifetime of its routes.
///
/// `Singleton` carries the instance built once at startup; every route shares it.
/// `Request` carries the resolved dependency map and the controller is rebuilt on
/// each request — used when an (implicit or explicit) singleton controller depends
/// on request-scoped providers, or when the controller is explicitly request-scoped.
pub enum ControllerInstance {
    Singleton(Arc<dyn Any + Send + Sync>),
    Request(FxHashMap<String, Arc<Box<dyn Provider>>>),
}

/// What a controller hands over to be dispatched on.
///
/// The one place the transports differ. HTTP dispatches on routes keyed by path and method, RPC on a
/// set of patterns, gRPC on a registration with the tonic router. Everything else a controller
/// carries — its token, its dependencies, its lifecycle, the scope it is built at — is common to all
/// three and lives on [`Controller`] itself.
pub enum Dispatch {
    Http(Vec<Arc<dyn Route>>),
    Rpc(Arc<dyn crate::rpc::RpcControllerSource>),
    Grpc(Arc<dyn crate::adapter::GrpcServiceSource>),
}

/// One dispatchable route: the handler plus the routing facts and enhancers the
/// dispatcher needs to register and run it. A `Controller` yields one `Route` per
/// handler method.
#[async_trait]
pub trait Route: Send + Sync {
    /// Run the user handler and return either the rendered success response
    /// or the user's typed error preserved for the dispatcher's observer +
    /// chain pipeline.
    ///
    /// `ctx` is shared: a context is a handle several participants in one
    /// execution hold at once, and the request body — the only part needing
    /// exclusive access — sits behind a lock.
    ///
    /// Returning is the only way to answer. A response is not a field on the
    /// context, so there is no off-phase write to overrule; an enhancer
    /// short-circuits by returning too.
    async fn execute(&self, ctx: &HttpContext) -> ExecutionResult<HttpResponse, HttpError>;
    fn get_path(&self) -> String;
    fn get_method(&self) -> HttpMethod;

    fn enhancers(&self) -> ControllerEnhancers {
        ControllerEnhancers::default()
    }

    /// Get route metadata (roles, permissions, custom config)
    fn metadata(&self) -> Arc<Metadata> {
        Arc::new(Metadata::new())
    }
}

/// A controller: one DI instance exposing its routes and lifecycle hooks.
///
/// Built once per controller struct by its [`ControllerFactory`]. `routes()` yields
/// one [`Route`] per handler method; the lifecycle hooks fire once per controller,
/// not once per route.
#[async_trait]
pub trait Controller: Send + Sync {
    fn get_token(&self) -> String;
    fn dispatch(&self) -> Dispatch;

    // Lifecycle Hooks

    async fn on_module_init(&self) -> crate::InitResult {
        Ok(())
    }
    async fn on_application_bootstrap(&self) -> crate::InitResult {
        Ok(())
    }
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
    async fn build(&self, deps: FxHashMap<String, Arc<Box<dyn Provider>>>) -> Arc<dyn Controller>;
}
