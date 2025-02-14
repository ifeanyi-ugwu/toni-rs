use std::sync::Arc;

use anyhow::Result;

use crate::traits_helpers::ControllerTrait;
use crate::http_helpers::{HttpRequest, HttpResponse, IntoResponse};

pub trait RouteAdapter {
    type Request;
    type Response;
    
    fn adapt_request(request: Self::Request)  -> impl Future<Output = Result<HttpRequest>>;
    
    fn adapt_response(response: Box<dyn IntoResponse<Response = HttpResponse>>) -> Result<Self::Response>;
    
    fn handle_request(
        request: Self::Request,
        controller: Arc<Box<dyn ControllerTrait>>,
    ) -> impl Future<Output = Result<Self::Response>> {
        async move {
            let http_request = Self::adapt_request(request).await?;
            let http_response = controller.execute(http_request);
            Self::adapt_response(http_response)
        }
    }}
