use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::{ProviderScope, http_helpers::HttpRequest};

#[async_trait]
pub trait ProviderTrait: Send + Sync {
    fn get_token(&self) -> String;
    async fn execute(
        &self,
        params: Vec<Box<dyn Any + Send>>,
        req: Option<&HttpRequest>,
    ) -> Box<dyn Any + Send>;
    fn get_token_manager(&self) -> String;
    fn get_scope(&self) -> ProviderScope {
        ProviderScope::Singleton // Default to singleton
    }
}

#[async_trait]
pub trait Provider {
    async fn get_all_providers(
        &self,
        dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> FxHashMap<String, Arc<Box<dyn ProviderTrait>>>;
    fn get_name(&self) -> String;
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String>;
}
