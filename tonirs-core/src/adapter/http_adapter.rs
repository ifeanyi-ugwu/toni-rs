use std::sync::Arc;

use anyhow::Result;

use crate::http_helpers::HttpMethod;
use crate::traits_helpers::ControllerTrait;

pub trait HttpAdapter: Clone + Send + Sync {
    fn new() -> Self;
    fn add_route(&mut self, path: &String, method: HttpMethod, handler: Arc<Box<dyn ControllerTrait>>);
    fn listen(self, port: u16, hostname: &str) -> impl Future<Output = Result<()>> + Send;
}
