use std::sync::Arc;

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::http_helpers::{HttpMethod, HttpRequest, HttpResponse, IntoResponse};

use super::{Guard, Interceptor, Pipe, provider::ProviderTrait, validate::Validatable};

#[async_trait]
pub trait ControllerTrait: Send + Sync {
    fn get_token(&self) -> String;
    async fn execute(
        &self,
        req: HttpRequest,
    ) -> Box<dyn IntoResponse<Response = HttpResponse> + Send>;
    fn get_path(&self) -> String;
    fn get_method(&self) -> HttpMethod;
    fn get_guards(&self) -> Vec<Arc<dyn Guard>>;
    fn get_pipes(&self) -> Vec<Arc<dyn Pipe>>;
    fn get_interceptors(&self) -> Vec<Arc<dyn Interceptor>>;
    fn get_body_dto(&self, req: &HttpRequest) -> Option<Box<dyn Validatable>>;
}
#[async_trait]
pub trait Controller {
    async fn get_all_controllers(
        &self,
        dependencies: &FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> FxHashMap<String, Arc<Box<dyn ControllerTrait>>>;
    fn get_name(&self) -> String;
    fn get_token(&self) -> String;
    fn get_dependencies(&self) -> Vec<String>;
}
