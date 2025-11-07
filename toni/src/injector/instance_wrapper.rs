use std::sync::Arc;

use crate::{
    http_helpers::{HttpMethod, HttpRequest, HttpResponse, IntoResponse},
    middleware::{Middleware, MiddlewareChain},
    structs_helpers::EnhancerMetadata,
    traits_helpers::{ControllerTrait, Guard, Interceptor, Pipe},
};

use super::Context;

pub struct InstanceWrapper {
    instance: Arc<Box<dyn ControllerTrait>>,
    guards: Vec<Arc<dyn Guard>>,
    interceptors: Vec<Arc<dyn Interceptor>>,
    pipes: Vec<Arc<dyn Pipe>>,
    middleware_chain: MiddlewareChain,
}

impl InstanceWrapper {
    pub fn new(
        instance: Arc<Box<dyn ControllerTrait>>,
        enhancer_metadata: EnhancerMetadata,
    ) -> Self {
        Self {
            instance,
            guards: enhancer_metadata.guards,
            interceptors: enhancer_metadata.interceptors,
            pipes: enhancer_metadata.pipes,
            middleware_chain: MiddlewareChain::new(),
        }
    }

    pub fn get_path(&self) -> String {
        self.instance.get_path()
    }

    pub fn get_method(&self) -> HttpMethod {
        self.instance.get_method()
    }

    pub fn add_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middleware_chain.use_middleware(middleware);
    }

    pub fn set_middleware(&mut self, middleware: Vec<Arc<dyn Middleware>>) {
        for m in middleware {
            self.middleware_chain.use_middleware(m);
        }
    }

    pub async fn handle_request(
        &self,
        req: HttpRequest,
    ) -> Box<dyn IntoResponse<Response = HttpResponse> + Send> {
        let instance = self.instance.clone();
        let guards = self.guards.clone();
        let interceptors = self.interceptors.clone();
        let pipes = self.pipes.clone();

        // Execute middleware chain with controller as the final handler
        let middleware_result = self
            .middleware_chain
            .execute(req, move |req| {
                let instance = instance.clone();
                let guards = guards.clone();
                let interceptors = interceptors.clone();
                let pipes = pipes.clone();

                Box::pin(async move {
                    Self::execute_controller_logic(req, instance, guards, interceptors, pipes).await
                })
            })
            .await;

        // Handle the result from middleware chain
        match middleware_result {
            Ok(response) => Box::new(response),
            Err(e) => {
                // Convert error to HTTP response
                eprintln!("❌ Middleware error: {}", e);
                let mut error_response = HttpResponse::new();
                error_response.status = 500;
                error_response.body = Some(crate::http_helpers::Body::Json(serde_json::json!({
                    "error": "Internal Server Error",
                    "message": "An error occurred while processing the request"
                })));
                Box::new(error_response)
            }
        }
    }

    /// Execute the controller logic with guards, interceptors, and pipes
    async fn execute_controller_logic(
        req: HttpRequest,
        instance: Arc<Box<dyn ControllerTrait>>,
        guards: Vec<Arc<dyn Guard>>,
        interceptors: Vec<Arc<dyn Interceptor>>,
        pipes: Vec<Arc<dyn Pipe>>,
    ) -> HttpResponse {
        let mut context = Context::from_request(req);

        // Execute guards
        for guard in &guards {
            if !guard.can_activate(&context) {
                return context.get_response().to_response();
            }
        }

        // Execute before interceptors
        for interceptor in &interceptors {
            interceptor.before_execute(&mut context);
        }

        // Get and validate DTO
        let dto = instance.get_body_dto(context.take_request());
        if let Some(dto) = dto {
            context.set_dto(dto);
        }

        // Execute pipes
        for pipe in &pipes {
            pipe.process(&mut context);
            if context.should_abort() {
                return context.get_response().to_response();
            }
        }

        // Execute controller
        let req = context.take_request().clone();
        let controller_response = instance.execute(req).await;
        context.set_response(controller_response);

        // Execute after interceptors
        for interceptor in &interceptors {
            interceptor.after_execute(&mut context);
        }

        context.get_response().to_response()
    }
}
