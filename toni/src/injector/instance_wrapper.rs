use std::sync::Arc;

use crate::{
    http_helpers::{HttpMethod, HttpRequest, HttpResponse, IntoResponse},
    structs_helpers::EnhancerMetadata,
    traits_helpers::{ControllerTrait, Guard, Interceptor, Pipe},
};

use super::Context;

pub struct InstanceWrapper {
    instance: Arc<Box<dyn ControllerTrait>>,
    guards: Vec<Arc<dyn Guard>>,
    interceptors: Vec<Arc<dyn Interceptor>>,
    pipes: Vec<Arc<dyn Pipe>>,
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
        }
    }

    pub fn get_path(&self) -> String {
        self.instance.get_path()
    }

    pub fn get_method(&self) -> HttpMethod {
        self.instance.get_method()
    }

    pub async fn handle_request(
        &self,
        req: HttpRequest,
    ) -> Box<dyn IntoResponse<Response = HttpResponse> + Send> {
        let mut context = Context::from_request(req);

        for guard in &self.guards {
            if !guard.can_activate(&context) {
                return context.get_response();
            }
        }

        for interceptor in &self.interceptors {
            interceptor.before_execute(&mut context);
        }

        let dto = self.instance.get_body_dto(context.take_request());
        if let Some(dto) = dto {
            context.set_dto(dto);
        }

        for pipe in &self.pipes {
            pipe.process(&mut context);
            if context.should_abort() {
                return context.get_response();
            }
        }

        let req = context.take_request().clone();

        let controller_response = self.instance.execute(req).await;

        context.set_response(controller_response);

        for interceptor in &self.interceptors {
            interceptor.after_execute(&mut context);
        }

        context.get_response()
    }
}
