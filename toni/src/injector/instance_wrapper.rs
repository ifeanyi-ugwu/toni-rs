use std::sync::Arc;

use crate::{
    async_trait,
    context::{HandlerContext, HttpContext},
    http_helpers::{HttpMethod, HttpRequest, HttpResponse, RouteMetadata},
    middleware::{Middleware, MiddlewareChain},
    structs_helpers::EnhancerMetadata,
    traits_helpers::{
        Controller, Guard, HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry, HttpPipeEntry,
        Interceptor, InterceptorNext, Pipe,
    },
};

/// The next step in the interceptor chain after factory entries are resolved.
struct ChainNext {
    interceptors: Vec<Arc<dyn Interceptor<HttpContext>>>,
    instance: Arc<Box<dyn Controller>>,
    pipes: Vec<Arc<dyn Pipe<HttpContext>>>,
    route_metadata: Arc<RouteMetadata>,
}

#[async_trait]
impl InterceptorNext<HttpContext> for ChainNext {
    async fn run(self: Box<Self>, context: &mut HttpContext) {
        InstanceWrapper::execute_with_interceptors(
            context,
            &self.interceptors,
            &self.instance,
            &self.pipes,
            &self.route_metadata,
        )
        .await;
    }
}

pub struct InstanceWrapper {
    instance: Arc<Box<dyn Controller>>,
    guards: Vec<HttpGuardEntry>,
    interceptors: Vec<HttpInterceptorEntry>,
    pipes: Vec<HttpPipeEntry>,
    middleware_chain: MiddlewareChain,
    error_handlers: Vec<HttpErrorHandlerArc>,
    route_metadata: Arc<RouteMetadata>,
}

impl InstanceWrapper {
    pub fn new(
        instance: Arc<Box<dyn Controller>>,
        enhancer_metadata: EnhancerMetadata,
        global_enhancers: EnhancerMetadata,
    ) -> Self {
        // Execution order: global → controller → method
        let mut guards = global_enhancers.guards;
        guards.extend(enhancer_metadata.guards);

        let mut interceptors = global_enhancers.interceptors;
        interceptors.extend(enhancer_metadata.interceptors);

        let mut pipes = global_enhancers.pipes;
        pipes.extend(enhancer_metadata.pipes);

        let mut error_handlers = global_enhancers.error_handlers;
        error_handlers.extend(enhancer_metadata.error_handlers);

        let route_metadata = instance.get_route_metadata();

        Self {
            instance,
            guards,
            interceptors,
            pipes,
            middleware_chain: MiddlewareChain::new(),
            error_handlers,
            route_metadata,
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

    pub fn get_instance(&self) -> Arc<Box<dyn Controller>> {
        self.instance.clone()
    }

    pub async fn handle_request(&self, req: HttpRequest) -> HttpResponse {
        let method = self.get_method();
        let path = self.get_path();
        tracing::debug!(method = %method.as_str(), path = %path, "incoming request");

        let instance = self.instance.clone();
        let guards = self.guards.clone();
        let interceptors = self.interceptors.clone();
        let pipes = self.pipes.clone();
        let error_handlers_for_controller = self.error_handlers.clone();
        let error_handlers_for_middleware = self.error_handlers.clone();
        let route_metadata = self.route_metadata.clone();

        let middleware_result = self
            .middleware_chain
            .execute(req, move |req| {
                let instance = instance.clone();
                let guards = guards.clone();
                let interceptors = interceptors.clone();
                let pipes = pipes.clone();
                let error_handlers = error_handlers_for_controller.clone();
                let route_metadata = route_metadata.clone();

                Box::pin(async move {
                    Self::execute_controller_logic(
                        req,
                        instance,
                        guards,
                        interceptors,
                        pipes,
                        error_handlers,
                        route_metadata,
                    )
                    .await
                })
            })
            .await;

        match middleware_result {
            Ok(response) => {
                tracing::debug!(method = %method.as_str(), path = %path, status = response.status, "request completed");
                response
            }
            Err(e) => {
                if let Some(http_err) = e.downcast_ref::<crate::errors::HttpError>() {
                    return http_err.to_response();
                }

                let error_msg = e.to_string();
                // Middleware failed before the request body could be split; we have no
                // parts to thread through to the handler context. Construct a stub
                // from a minimal request so error handlers still get a typed context.
                let stub = http::Request::builder().body(()).unwrap();
                let error_ctx = HttpContext::from_parts(stub.into_parts().0);
                let error =
                    std::io::Error::new(std::io::ErrorKind::Other, error_msg.clone());
                for handler in error_handlers_for_middleware.iter().rev() {
                    if let Some(response) = handler.handle_error(&error, &error_ctx).await {
                        return response;
                    }
                }

                let mut error_response = HttpResponse::new();
                error_response.status = 500;
                error_response.body = Some(crate::http_helpers::Body::json(serde_json::json!({
                    "error": "Internal Server Error",
                    "message": "An error occurred while processing the request"
                })));
                error_response
            }
        }
    }

    async fn resolve_guards(
        entries: &[HttpGuardEntry],
        parts: &crate::http_helpers::RequestPart,
    ) -> Vec<Arc<dyn Guard<HttpContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                HttpGuardEntry::Ready(g) => g.clone(),
                HttpGuardEntry::Factory(f) => f.create(Some(parts)).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(
        entries: &[HttpInterceptorEntry],
        parts: &crate::http_helpers::RequestPart,
    ) -> Vec<Arc<dyn Interceptor<HttpContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                HttpInterceptorEntry::Ready(i) => i.clone(),
                HttpInterceptorEntry::Factory(f) => f.create(Some(parts)).await,
            };
            out.push(i);
        }
        out
    }

    async fn resolve_pipes(
        entries: &[HttpPipeEntry],
        parts: &crate::http_helpers::RequestPart,
    ) -> Vec<Arc<dyn Pipe<HttpContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let p = match entry {
                HttpPipeEntry::Ready(p) => p.clone(),
                HttpPipeEntry::Factory(f) => f.create(Some(parts)).await,
            };
            out.push(p);
        }
        out
    }

    async fn execute_controller_logic(
        req: HttpRequest,
        instance: Arc<Box<dyn Controller>>,
        guards: Vec<HttpGuardEntry>,
        interceptors: Vec<HttpInterceptorEntry>,
        pipes: Vec<HttpPipeEntry>,
        error_handlers: Vec<HttpErrorHandlerArc>,
        route_metadata: Arc<RouteMetadata>,
    ) -> HttpResponse {
        // Split req so factory entries see parts before the context takes ownership.
        let (parts, body) = req.into_parts();
        let guards = Self::resolve_guards(&guards, &parts).await;
        let interceptors = Self::resolve_interceptors(&interceptors, &parts).await;
        let pipes = Self::resolve_pipes(&pipes, &parts).await;
        let req = HttpRequest::from_parts(parts, body);

        let mut context = HttpContext::new(req, route_metadata.clone());

        for (i, guard) in guards.iter().enumerate() {
            if !guard.can_activate(&context).await {
                tracing::debug!(guard_index = i, "guard rejected request");
                let guard_response = context.take_response().unwrap_or_else(|| {
                    let mut forbidden = HttpResponse::new();
                    forbidden.status = 403;
                    forbidden.body = Some(crate::Body::text("Forbidden"));
                    forbidden
                });

                return Self::handle_error_response(guard_response, &error_handlers, &context).await;
            }
        }

        if !interceptors.is_empty() {
            tracing::trace!(count = interceptors.len(), "entering interceptor chain");
        }
        Self::execute_with_interceptors(
            &mut context,
            &interceptors,
            &instance,
            &pipes,
            &route_metadata,
        )
        .await;

        context.into_response()
    }

    /// Route 4xx/5xx responses through error handlers. Most-specific
    /// (method > controller > global) is consulted first.
    async fn handle_error_response(
        response: HttpResponse,
        error_handlers: &[HttpErrorHandlerArc],
        ctx: &HttpContext,
    ) -> HttpResponse {
        if response.status >= 400 {
            let http_error = Self::response_to_http_error(&response);

            for handler in error_handlers.iter().rev() {
                if let Some(handled) = handler.handle_error(&http_error, ctx).await {
                    return handled;
                }
            }
        }
        response
    }

    /// Onion/Russian doll dispatch through the interceptor chain.
    async fn execute_with_interceptors(
        context: &mut HttpContext,
        interceptors: &[Arc<dyn Interceptor<HttpContext>>],
        instance: &Arc<Box<dyn Controller>>,
        pipes: &[Arc<dyn Pipe<HttpContext>>],
        route_metadata: &Arc<RouteMetadata>,
    ) {
        if interceptors.is_empty() {
            Self::execute_handler_with_error_handling(context, instance, pipes).await;
            return;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = ChainNext {
            interceptors: rest.to_vec(),
            instance: instance.clone(),
            pipes: pipes.to_vec(),
            route_metadata: route_metadata.clone(),
        };

        first.intercept(context, Box::new(next)).await;
    }

    async fn execute_handler(
        context: &mut HttpContext,
        instance: &Arc<Box<dyn Controller>>,
        pipes: &[Arc<dyn Pipe<HttpContext>>],
    ) {
        let dto = instance.get_body_dto(context.request());
        if let Some(dto) = dto {
            match dto.validate_dto() {
                Ok(()) => {
                    context.set_dto(dto);
                }
                Err(validation_errors) => {
                    let error_body = serde_json::json!({
                        "error": "Validation failed",
                        "details": validation_errors.to_string()
                    });
                    let response = crate::http_helpers::HttpResponse {
                        body: Some(crate::http_helpers::Body::json(error_body)),
                        status: 400,
                        headers: vec![],
                    };
                    context.set_response(response);
                    context.abort();
                    return;
                }
            }
        }

        for pipe in pipes {
            pipe.process(context);
            if context.should_abort() {
                return;
            }
        }

        tracing::trace!(pipe_count = pipes.len(), "executing controller handler");
        let req = context.take_request();
        let controller_response = instance.execute(req).await;
        context.set_response(controller_response);
    }

    /// User errors render via `AppError::into_http_response` at the macro
    /// boundary — the response that arrives here is already the user's
    /// final answer. The error-handler chain only runs on framework-
    /// generated errors (guard rejections, middleware failures), where it
    /// has a typed framework error to dispatch on.
    async fn execute_handler_with_error_handling(
        context: &mut HttpContext,
        instance: &Arc<Box<dyn Controller>>,
        pipes: &[Arc<dyn Pipe<HttpContext>>],
    ) {
        Self::execute_handler(context, instance, pipes).await;
    }

    fn response_to_http_error(response: &HttpResponse) -> crate::errors::HttpError {
        let message = if let Some(body) = &response.body {
            if let Some(bytes) = body.try_bytes() {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
                    v.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("HTTP Error")
                        .to_string()
                } else {
                    std::str::from_utf8(bytes)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|_| format!("HTTP {} Error", response.status))
                }
            } else {
                format!("HTTP {} Error", response.status)
            }
        } else {
            format!("HTTP {} Error", response.status)
        };

        match response.status {
            400 => crate::errors::HttpError::bad_request(message),
            401 => crate::errors::HttpError::unauthorized(message),
            403 => crate::errors::HttpError::forbidden(message),
            404 => crate::errors::HttpError::not_found(message),
            409 => crate::errors::HttpError::conflict(message),
            422 => crate::errors::HttpError::unprocessable_entity(message),
            500 => crate::errors::HttpError::internal_server_error(message),
            status => crate::errors::HttpError::custom(status, message),
        }
    }
}
