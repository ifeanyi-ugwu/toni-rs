use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    adapter::GrpcServiceSource,
    rpc::RpcControllerSource,
    traits_helpers::{
        GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry, HttpErrorHandlerArc,
        HttpGuardEntry, HttpInterceptorEntry, ProviderRole, RpcErrorHandlerArc, RpcGuardEntry,
        RpcInterceptorEntry, WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry,
        middleware::Middleware,
    },
    websocket::GatewayTrait,
};

pub(crate) struct RoleRegistry {
    pub http_guards: FxHashMap<String, HttpGuardEntry>,
    pub http_interceptors: FxHashMap<String, HttpInterceptorEntry>,
    pub http_error_handlers: FxHashMap<String, HttpErrorHandlerArc>,

    pub rpc_guards: FxHashMap<String, RpcGuardEntry>,
    pub rpc_interceptors: FxHashMap<String, RpcInterceptorEntry>,
    pub rpc_error_handlers: FxHashMap<String, RpcErrorHandlerArc>,

    pub ws_guards: FxHashMap<String, WsGuardEntry>,
    pub ws_interceptors: FxHashMap<String, WsInterceptorEntry>,
    pub ws_error_handlers: FxHashMap<String, WsErrorHandlerArc>,

    pub grpc_guards: FxHashMap<String, GrpcGuardEntry>,
    pub grpc_interceptors: FxHashMap<String, GrpcInterceptorEntry>,
    pub grpc_error_handlers: FxHashMap<String, GrpcErrorHandlerArc>,

    pub middleware: FxHashMap<String, Arc<dyn Middleware>>,
    /// Keyed by WS path (e.g. "/chat"), not by provider token.
    pub gateways: FxHashMap<String, Arc<Box<dyn GatewayTrait>>>,
    /// Keyed by the RPC controller's own token.
    pub rpc_controllers: FxHashMap<String, Arc<dyn RpcControllerSource>>,
    /// Keyed by the gRPC service's own token.
    pub grpc_services: FxHashMap<String, Arc<dyn GrpcServiceSource>>,
}

impl RoleRegistry {
    pub fn new() -> Self {
        Self {
            http_guards: FxHashMap::default(),
            http_interceptors: FxHashMap::default(),
            http_error_handlers: FxHashMap::default(),
            rpc_guards: FxHashMap::default(),
            rpc_interceptors: FxHashMap::default(),
            rpc_error_handlers: FxHashMap::default(),
            ws_guards: FxHashMap::default(),
            ws_interceptors: FxHashMap::default(),
            ws_error_handlers: FxHashMap::default(),
            grpc_guards: FxHashMap::default(),
            grpc_interceptors: FxHashMap::default(),
            grpc_error_handlers: FxHashMap::default(),
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

        if let Some(g) = self.http_guards.get(token) {
            roles.push(ProviderRole::HttpGuard(g.clone()));
        }
        if let Some(i) = self.http_interceptors.get(token) {
            roles.push(ProviderRole::HttpInterceptor(i.clone()));
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
        if let Some(eh) = self.rpc_error_handlers.get(token) {
            roles.push(ProviderRole::RpcErrorHandler(eh.clone()));
        }

        if let Some(g) = self.ws_guards.get(token) {
            roles.push(ProviderRole::WsGuard(g.clone()));
        }
        if let Some(i) = self.ws_interceptors.get(token) {
            roles.push(ProviderRole::WsInterceptor(i.clone()));
        }
        if let Some(eh) = self.ws_error_handlers.get(token) {
            roles.push(ProviderRole::WsErrorHandler(eh.clone()));
        }

        if let Some(g) = self.grpc_guards.get(token) {
            roles.push(ProviderRole::GrpcGuard(g.clone()));
        }
        if let Some(i) = self.grpc_interceptors.get(token) {
            roles.push(ProviderRole::GrpcInterceptor(i.clone()));
        }
        if let Some(eh) = self.grpc_error_handlers.get(token) {
            roles.push(ProviderRole::GrpcErrorHandler(eh.clone()));
        }

        if let Some(m) = self.middleware.get(token) {
            roles.push(ProviderRole::Middleware(m.clone()));
        }
        roles
    }
}
