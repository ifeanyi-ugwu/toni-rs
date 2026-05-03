use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    adapter::GrpcServiceTrait,
    rpc::RpcControllerTrait,
    traits_helpers::{
        ErrorHandler, GuardEntry, HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry,
        HttpPipeEntry, InterceptorEntry, PipeEntry, ProviderRole, RpcErrorHandlerArc,
        RpcGuardEntry, RpcInterceptorEntry, RpcPipeEntry, WsErrorHandlerArc, WsGuardEntry,
        WsInterceptorEntry, WsPipeEntry, middleware::Middleware,
    },
    websocket::GatewayTrait,
};

pub(crate) struct RoleRegistry {
    // Legacy enum-shaped registries. TODO: remove once per-transport sub-registries
    // fully replace them.
    pub guards: FxHashMap<String, GuardEntry>,
    pub interceptors: FxHashMap<String, InterceptorEntry>,
    pub pipes: FxHashMap<String, PipeEntry>,
    pub error_handlers: FxHashMap<String, Arc<dyn ErrorHandler>>,

    // Per-transport typed registries.
    pub http_guards: FxHashMap<String, HttpGuardEntry>,
    pub http_interceptors: FxHashMap<String, HttpInterceptorEntry>,
    pub http_pipes: FxHashMap<String, HttpPipeEntry>,
    pub http_error_handlers: FxHashMap<String, HttpErrorHandlerArc>,

    pub rpc_guards: FxHashMap<String, RpcGuardEntry>,
    pub rpc_interceptors: FxHashMap<String, RpcInterceptorEntry>,
    pub rpc_pipes: FxHashMap<String, RpcPipeEntry>,
    pub rpc_error_handlers: FxHashMap<String, RpcErrorHandlerArc>,

    pub ws_guards: FxHashMap<String, WsGuardEntry>,
    pub ws_interceptors: FxHashMap<String, WsInterceptorEntry>,
    pub ws_pipes: FxHashMap<String, WsPipeEntry>,
    pub ws_error_handlers: FxHashMap<String, WsErrorHandlerArc>,

    pub middleware: FxHashMap<String, Arc<dyn Middleware>>,
    /// Keyed by WS path (e.g. "/chat"), not by provider token.
    pub gateways: FxHashMap<String, Arc<Box<dyn GatewayTrait>>>,
    /// Keyed by the RPC controller's own token.
    pub rpc_controllers: FxHashMap<String, Arc<Box<dyn RpcControllerTrait>>>,
    /// Keyed by the gRPC service's own token.
    pub grpc_services: FxHashMap<String, Arc<Box<dyn GrpcServiceTrait>>>,
}

impl RoleRegistry {
    pub fn new() -> Self {
        Self {
            guards: FxHashMap::default(),
            interceptors: FxHashMap::default(),
            pipes: FxHashMap::default(),
            error_handlers: FxHashMap::default(),
            http_guards: FxHashMap::default(),
            http_interceptors: FxHashMap::default(),
            http_pipes: FxHashMap::default(),
            http_error_handlers: FxHashMap::default(),
            rpc_guards: FxHashMap::default(),
            rpc_interceptors: FxHashMap::default(),
            rpc_pipes: FxHashMap::default(),
            rpc_error_handlers: FxHashMap::default(),
            ws_guards: FxHashMap::default(),
            ws_interceptors: FxHashMap::default(),
            ws_pipes: FxHashMap::default(),
            ws_error_handlers: FxHashMap::default(),
            middleware: FxHashMap::default(),
            gateways: FxHashMap::default(),
            rpc_controllers: FxHashMap::default(),
            grpc_services: FxHashMap::default(),
        }
    }

    /// Reconstruct all roles registered under `token` as a `Vec<ProviderRole>`.
    ///
    /// Used when building the deps map for a factory that needs to forward the
    /// roles of an already-built provider (e.g. alias targets from imported modules).
    pub fn get_roles_for_token(&self, token: &str) -> Vec<ProviderRole> {
        let mut roles = Vec::new();
        if let Some(g) = self.guards.get(token) {
            roles.push(ProviderRole::Guard(g.clone()));
        }
        if let Some(i) = self.interceptors.get(token) {
            roles.push(ProviderRole::Interceptor(i.clone()));
        }
        if let Some(p) = self.pipes.get(token) {
            roles.push(ProviderRole::Pipe(p.clone()));
        }
        if let Some(eh) = self.error_handlers.get(token) {
            roles.push(ProviderRole::ErrorHandler(eh.clone()));
        }

        if let Some(g) = self.http_guards.get(token) {
            roles.push(ProviderRole::HttpGuard(g.clone()));
        }
        if let Some(i) = self.http_interceptors.get(token) {
            roles.push(ProviderRole::HttpInterceptor(i.clone()));
        }
        if let Some(p) = self.http_pipes.get(token) {
            roles.push(ProviderRole::HttpPipe(p.clone()));
        }
        if let Some(eh) = self.http_error_handlers.get(token) {
            roles.push(ProviderRole::HttpErrorHandler(eh.clone()));
        }

        if let Some(g) = self.rpc_guards.get(token) {
            roles.push(ProviderRole::RpcGuard(g.clone()));
        }
        if let Some(i) = self.rpc_interceptors.get(token) {
            roles.push(ProviderRole::RpcInterceptor(i.clone()));
        }
        if let Some(p) = self.rpc_pipes.get(token) {
            roles.push(ProviderRole::RpcPipe(p.clone()));
        }
        if let Some(eh) = self.rpc_error_handlers.get(token) {
            roles.push(ProviderRole::RpcErrorHandler(eh.clone()));
        }

        if let Some(g) = self.ws_guards.get(token) {
            roles.push(ProviderRole::WsGuard(g.clone()));
        }
        if let Some(i) = self.ws_interceptors.get(token) {
            roles.push(ProviderRole::WsInterceptor(i.clone()));
        }
        if let Some(p) = self.ws_pipes.get(token) {
            roles.push(ProviderRole::WsPipe(p.clone()));
        }
        if let Some(eh) = self.ws_error_handlers.get(token) {
            roles.push(ProviderRole::WsErrorHandler(eh.clone()));
        }

        if let Some(m) = self.middleware.get(token) {
            roles.push(ProviderRole::Middleware(m.clone()));
        }
        roles
    }
}
