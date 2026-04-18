use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use super::{
    ErrorHandler, Guard, Interceptor, Pipe, ProviderContext, middleware::Middleware,
};
use crate::ProviderScope;

#[async_trait]
pub trait Provider: Send + Sync {
    fn get_token(&self) -> String;
    async fn execute(
        &self,
        params: Vec<Box<dyn Any + Send>>,
        ctx: ProviderContext<'_>,
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
    async fn on_module_init(&self) {}
    async fn on_application_bootstrap(&self) {}
    async fn on_module_destroy(&self) {}
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}

/// Role trait-objects a provider may contribute to the registry.
///
/// Returned as the second element of `ProviderFactory::build`. The container
/// inserts each variant into the matching slot of `RoleRegistry` keyed by the
/// provider token (or, for gateways, by WS path; for RPC controllers, by
/// controller token).
#[derive(Clone)]
pub enum ProviderRole {
    Guard(Arc<dyn Guard>),
    Interceptor(Arc<dyn Interceptor>),
    Pipe(Arc<dyn Pipe>),
    Middleware(Arc<dyn Middleware>),
    ErrorHandler(Arc<dyn ErrorHandler>),
    Gateway(Arc<Box<dyn crate::websocket::GatewayTrait>>),
    RpcController(Arc<Box<dyn crate::rpc::RpcControllerTrait>>),
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

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable;
}
