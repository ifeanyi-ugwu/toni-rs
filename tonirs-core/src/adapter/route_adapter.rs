use std::sync::Arc;

use crate::traits_helpers::ControllerTrait;
use crate::http_helpers::{HttpRequest, HttpResponse, IntoResponse};

pub trait RouteAdapter {
    type Request;
    type Response;
    
    async fn adapt_request(request: Self::Request) -> HttpRequest;
    
    fn adapt_response(response: Box<dyn IntoResponse<Response = HttpResponse>>) -> Self::Response;
    
    async fn handle_request(
        request: Self::Request,
        controller: Arc<Box<dyn ControllerTrait>>,
    ) -> Self::Response {
        let http_request = Self::adapt_request(request).await;
        let http_response = controller.execute(http_request);
        Self::adapt_response(http_response)
    }
}
