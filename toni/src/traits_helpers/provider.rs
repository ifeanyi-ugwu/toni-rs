use std::{any::Any, future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use super::{ErrorHandler, Guard, Interceptor, Pipe, ProviderContext, middleware::Middleware};
use crate::{
    ProviderScope,
    context::{GrpcContext, HttpContext, RpcContext, WsContext},
    http_helpers::HttpResponse,
    rpc::RpcData,
    websocket::WsMessage,
};

#[allow(unused_imports)]
use std::marker::PhantomData;

#[async_trait]
pub trait Provider: Send + Sync {
    fn get_token(&self) -> String;
    async fn execute(
        &self,
        params: Vec<Box<dyn Any + Send>>,
        ctx: ProviderContext,
    ) -> Box<dyn Any + Send>;
    fn get_token_factory(&self) -> String;
    fn get_scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }

    fn get_multi_base_token(&self) -> Option<String> {
        None
    }
    fn as_multi_item(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    // Lifecycle hooks — overridden by the macro when the user annotates a method.
    // Default implementations are no-ops so providers without hooks incur no overhead.
    async fn on_module_init(&self) -> crate::InitResult {
        Ok(())
    }
    async fn on_application_bootstrap(&self) -> crate::InitResult {
        Ok(())
    }
    async fn on_module_destroy(&self) {}
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}

// ---- Per-transport entry / factory types (typed registries) ----------------

macro_rules! transport_factory_types {
    (
        $context:ty, $answer:ty,
        $guard_factory:ident, $guard_entry:ident,
        $interceptor_factory:ident, $interceptor_entry:ident,
        $pipe_factory:ident, $pipe_entry:ident
    ) => {
        pub trait $guard_factory: Send + Sync {
            fn create<'a>(
                &'a self,
                ctx: &'a $context,
            ) -> Pin<Box<dyn Future<Output = Arc<dyn Guard<$context> + Send + Sync>> + Send + 'a>>;
        }

        #[derive(Clone)]
        pub enum $guard_entry {
            Ready(Arc<dyn Guard<$context>>),
            Factory(Arc<dyn $guard_factory>),
        }

        pub trait $interceptor_factory: Send + Sync {
            fn create<'a>(
                &'a self,
                ctx: &'a $context,
            ) -> Pin<
                Box<
                    dyn Future<Output = Arc<dyn Interceptor<$context, $answer> + Send + Sync>>
                        + Send
                        + 'a,
                >,
            >;
        }

        #[derive(Clone)]
        pub enum $interceptor_entry {
            Ready(Arc<dyn Interceptor<$context, $answer>>),
            Factory(Arc<dyn $interceptor_factory>),
        }

        pub trait $pipe_factory: Send + Sync {
            fn create<'a>(
                &'a self,
                ctx: &'a $context,
            ) -> Pin<
                Box<
                    dyn Future<Output = Arc<dyn Pipe<$context, $answer> + Send + Sync>> + Send + 'a,
                >,
            >;
        }

        #[derive(Clone)]
        pub enum $pipe_entry {
            Ready(Arc<dyn Pipe<$context, $answer>>),
            Factory(Arc<dyn $pipe_factory>),
        }
    };
}

transport_factory_types!(
    HttpContext,
    HttpResponse,
    DynHttpGuardFactory,
    HttpGuardEntry,
    DynHttpInterceptorFactory,
    HttpInterceptorEntry,
    DynHttpPipeFactory,
    HttpPipeEntry
);

transport_factory_types!(
    RpcContext,
    crate::rpc::RpcHandlerResult,
    DynRpcGuardFactory,
    RpcGuardEntry,
    DynRpcInterceptorFactory,
    RpcInterceptorEntry,
    DynRpcPipeFactory,
    RpcPipeEntry
);

transport_factory_types!(
    WsContext,
    crate::websocket::WsHandlerResult,
    DynWsGuardFactory,
    WsGuardEntry,
    DynWsInterceptorFactory,
    WsInterceptorEntry,
    DynWsPipeFactory,
    WsPipeEntry
);

transport_factory_types!(
    GrpcContext,
    crate::grpc_status::GrpcHandlerResult,
    DynGrpcGuardFactory,
    GrpcGuardEntry,
    DynGrpcInterceptorFactory,
    GrpcInterceptorEntry,
    DynGrpcPipeFactory,
    GrpcPipeEntry
);

pub type HttpErrorHandlerArc = Arc<dyn ErrorHandler<HttpContext, HttpResponse>>;
pub type RpcErrorHandlerArc = Arc<dyn ErrorHandler<RpcContext, RpcData>>;
pub type WsErrorHandlerArc = Arc<dyn ErrorHandler<WsContext, WsMessage>>;
pub type GrpcErrorHandlerArc = Arc<dyn ErrorHandler<GrpcContext, crate::grpc_status::GrpcStatus>>;

/// Role trait-objects a provider may contribute to the registry.
///
/// Returned as the second element of `ProviderFactory::build`. The container
/// inserts each variant into the matching slot of `RoleRegistry` keyed by the
/// provider token (or, for gateways, by WS path; for RPC controllers, by
/// controller token).
#[derive(Clone)]
pub enum ProviderRole {
    HttpGuard(HttpGuardEntry),
    HttpInterceptor(HttpInterceptorEntry),
    HttpPipe(HttpPipeEntry),
    HttpErrorHandler(HttpErrorHandlerArc),

    RpcGuard(RpcGuardEntry),
    RpcInterceptor(RpcInterceptorEntry),
    RpcPipe(RpcPipeEntry),
    RpcErrorHandler(RpcErrorHandlerArc),

    WsGuard(WsGuardEntry),
    WsInterceptor(WsInterceptorEntry),
    WsPipe(WsPipeEntry),
    WsErrorHandler(WsErrorHandlerArc),

    GrpcGuard(GrpcGuardEntry),
    GrpcInterceptor(GrpcInterceptorEntry),
    GrpcErrorHandler(GrpcErrorHandlerArc),

    Middleware(Arc<dyn Middleware>),
    Gateway(Arc<Box<dyn crate::websocket::GatewayTrait>>),
    RpcController(Arc<dyn crate::rpc::RpcControllerSource>),
    GrpcService(Arc<dyn crate::adapter::GrpcServiceSource>),
}

/// A fully-built, ready-to-inject provider with its role registrations.
///
/// Returned from `ProviderFactory::build` and passed as dep values so
/// wrapper factories (e.g. `provider_alias!`) can forward roles without
/// a downcast.
#[derive(Clone)]
pub struct Injectable {
    pub instance: Arc<Box<dyn Provider>>,
    pub roles: Vec<ProviderRole>,
}

impl Injectable {
    pub fn new(instance: Arc<Box<dyn Provider>>, roles: Vec<ProviderRole>) -> Self {
        Self { instance, roles }
    }

    /// Whether this is reached only by its transport's dispatch — an RPC controller. Such an
    /// instance is not resolvable as a dependency, so the injector skips it rather than handing
    /// it to whatever asked.
    pub fn is_dispatch_target(&self) -> bool {
        self.roles
            .iter()
            .any(|role| matches!(role, ProviderRole::RpcController(_)))
    }
}

#[async_trait]
pub trait ProviderFactory {
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String> {
        vec![]
    }
    fn get_multi_base_token(&self) -> Option<String> {
        None
    }

    /// A fingerprint of this factory's runtime configuration, folded into the identity of the
    /// `DynamicModule` that carries it.
    ///
    /// Two dynamic modules built from the same maker (e.g. `SeaOrmModule::for_root`) share a base
    /// name but must be distinguished by what they were configured with — a database URL, a pool
    /// size. Return a value derived from that config so identical registrations dedup (the same
    /// module reached through two import paths) while different ones stay distinct. `None` (the
    /// default) leaves identity keyed on the base name alone: two such modules with differing
    /// config collapse silently, as before. Integrations that support multiple instances should
    /// override this.
    fn identity_hint(&self) -> Option<String> {
        None
    }

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable;
}
