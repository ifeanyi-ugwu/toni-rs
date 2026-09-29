use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    dispatch::transport::{EnhancerRegistry, Grpc, Http, Rpc, Ws},
    grpc::{GrpcServiceSource, ResolvedGrpcEnhancers},
    http::middleware::Middleware,
    rpc::RpcControllerWrapper,
    spi::ProviderRole,
    ws::Gateway,
};

pub(crate) struct RoleRegistry {
    pub http: EnhancerRegistry<Http>,
    pub rpc: EnhancerRegistry<Rpc>,
    pub ws: EnhancerRegistry<Ws>,
    pub grpc: EnhancerRegistry<Grpc>,

    pub middleware: FxHashMap<String, Arc<dyn Middleware>>,
    /// Keyed by WS path (e.g. "/chat"), not by provider token.
    pub gateways: FxHashMap<String, Arc<dyn Gateway>>,
    /// Keyed by the RPC controller's own token. Enhancer tokens are already resolved — the
    /// wrapper is stored ready to serve, and bind only hands it to the adapter.
    pub rpc_controllers: FxHashMap<String, Arc<RpcControllerWrapper>>,
    /// Keyed by the gRPC service's own token, with its enhancer bundle already resolved.
    pub grpc_services: FxHashMap<String, (Arc<dyn GrpcServiceSource>, Arc<ResolvedGrpcEnhancers>)>,
}

impl RoleRegistry {
    pub(crate) fn new() -> Self {
        Self {
            http: EnhancerRegistry::default(),
            rpc: EnhancerRegistry::default(),
            ws: EnhancerRegistry::default(),
            grpc: EnhancerRegistry::default(),
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
    pub(crate) fn get_roles_for_token(&self, token: &str) -> Vec<ProviderRole> {
        let mut roles = Vec::new();

        if let Some(g) = self.http.guards.get(token) {
            roles.push(ProviderRole::HttpGuard(g.clone()));
        }
        if let Some(i) = self.http.interceptors.get(token) {
            roles.push(ProviderRole::HttpInterceptor(i.clone()));
        }
        if let Some(eh) = self.http.error_handlers.get(token) {
            roles.push(ProviderRole::HttpErrorHandler(eh.clone()));
        }

        if let Some(g) = self.rpc.guards.get(token) {
            roles.push(ProviderRole::RpcGuard(g.clone()));
        }
        if let Some(i) = self.rpc.interceptors.get(token) {
            roles.push(ProviderRole::RpcInterceptor(i.clone()));
        }
        if let Some(eh) = self.rpc.error_handlers.get(token) {
            roles.push(ProviderRole::RpcErrorHandler(eh.clone()));
        }

        if let Some(g) = self.ws.guards.get(token) {
            roles.push(ProviderRole::WsGuard(g.clone()));
        }
        if let Some(i) = self.ws.interceptors.get(token) {
            roles.push(ProviderRole::WsInterceptor(i.clone()));
        }
        if let Some(eh) = self.ws.error_handlers.get(token) {
            roles.push(ProviderRole::WsErrorHandler(eh.clone()));
        }

        if let Some(g) = self.grpc.guards.get(token) {
            roles.push(ProviderRole::GrpcGuard(g.clone()));
        }
        if let Some(i) = self.grpc.interceptors.get(token) {
            roles.push(ProviderRole::GrpcInterceptor(i.clone()));
        }
        if let Some(eh) = self.grpc.error_handlers.get(token) {
            roles.push(ProviderRole::GrpcErrorHandler(eh.clone()));
        }

        if let Some(m) = self.middleware.get(token) {
            roles.push(ProviderRole::Middleware(m.clone()));
        }
        roles
    }
}
