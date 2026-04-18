use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::{
    rpc::RpcControllerTrait,
    traits_helpers::{ErrorHandler, Guard, Interceptor, Pipe, middleware::Middleware},
    websocket::GatewayTrait,
};

pub(crate) struct RoleRegistry {
    pub guards: FxHashMap<String, Arc<dyn Guard>>,
    pub interceptors: FxHashMap<String, Arc<dyn Interceptor>>,
    pub pipes: FxHashMap<String, Arc<dyn Pipe>>,
    pub middleware: FxHashMap<String, Arc<dyn Middleware>>,
    pub error_handlers: FxHashMap<String, Arc<dyn ErrorHandler>>,
    /// Keyed by WS path (e.g. "/chat"), not by provider token.
    pub gateways: FxHashMap<String, Arc<Box<dyn GatewayTrait>>>,
    /// Keyed by the RPC controller's own token.
    pub rpc_controllers: FxHashMap<String, Arc<Box<dyn RpcControllerTrait>>>,
}

impl RoleRegistry {
    pub fn new() -> Self {
        Self {
            guards: FxHashMap::default(),
            interceptors: FxHashMap::default(),
            pipes: FxHashMap::default(),
            middleware: FxHashMap::default(),
            error_handlers: FxHashMap::default(),
            gateways: FxHashMap::default(),
            rpc_controllers: FxHashMap::default(),
        }
    }

    /// Reconstruct all roles registered under `token` as a `Vec<ProviderRole>`.
    ///
    /// Used when building the deps map for a factory that needs to forward the
    /// roles of an already-built provider (e.g. alias targets from imported modules).
    pub fn get_roles_for_token(&self, token: &str) -> Vec<crate::traits_helpers::ProviderRole> {
        let mut roles = Vec::new();
        if let Some(g) = self.guards.get(token) {
            roles.push(crate::traits_helpers::ProviderRole::Guard(g.clone()));
        }
        if let Some(i) = self.interceptors.get(token) {
            roles.push(crate::traits_helpers::ProviderRole::Interceptor(i.clone()));
        }
        if let Some(p) = self.pipes.get(token) {
            roles.push(crate::traits_helpers::ProviderRole::Pipe(p.clone()));
        }
        if let Some(m) = self.middleware.get(token) {
            roles.push(crate::traits_helpers::ProviderRole::Middleware(m.clone()));
        }
        if let Some(eh) = self.error_handlers.get(token) {
            roles.push(crate::traits_helpers::ProviderRole::ErrorHandler(eh.clone()));
        }
        roles
    }
}
