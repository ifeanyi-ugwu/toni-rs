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
}
