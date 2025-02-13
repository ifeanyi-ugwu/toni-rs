use std::{any::Any, sync::Arc};

use rustc_hash::FxHashMap;

pub trait ProviderTrait: Send + Sync {
    fn get_token(&self) -> String;
    fn execute(&self, params: Vec<Box<dyn Any>>) -> Box<dyn Any>;
    fn get_token_manager(&self) -> String;
}

pub trait Provider {
    fn get_all_providers(
        &self,
        dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>
    ) -> FxHashMap<String, Arc<Box<dyn ProviderTrait>>>;
    fn get_name(&self) -> String;
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String>;
}
