use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

#[async_trait]
pub trait ProviderTrait: Send + Sync {
    fn get_token(&self) -> String;
    async fn execute(&self, params: Vec<Box<dyn Any + Send>>) -> Box<dyn Any + Send>;
    fn get_token_manager(&self) -> String;
}

pub trait Provider {
    fn get_all_providers(
        &self,
        dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> FxHashMap<String, Arc<Box<dyn ProviderTrait>>>;
    fn get_name(&self) -> String;
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String>;
}
