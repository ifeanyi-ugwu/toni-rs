use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::http_helpers::{HttpMethod, HttpRequest, HttpResponse, IntoResponse};

use super::provider::ProviderTrait;

#[async_trait]
pub trait ControllerTrait: Send + Sync {
    fn get_token(&self) -> String;
    async fn execute(&self, req: HttpRequest) -> Box<dyn IntoResponse<Response = HttpResponse>>;
    fn get_path(&self) -> String;
    fn get_method(&self) -> HttpMethod;
}
pub trait Controller {
    fn get_all_controllers(
        &self,
        dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> FxHashMap<String, Arc<Box<dyn ControllerTrait>>>;
    fn get_name(&self) -> String;
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String>;
}
