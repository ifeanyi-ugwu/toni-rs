use std::sync::Arc;

use crate::{
    async_trait,
    context::{HandlerContext, HttpContext},
    errors::{AppError, GuardRejection, MiddlewareFailure},
    http_helpers::{HttpMethod, HttpRequest, HttpResponse, RouteMetadata},
    middleware::{Middleware, MiddlewareChain},
    structs_helpers::EnhancerMetadata,
    traits_helpers::{
        Controller, ErrorObserver, Guard, HttpErrorHandlerArc, HttpGuardEntry,
        HttpInterceptorEntry, HttpPipeEntry, Interceptor, InterceptorNext, Pipe,
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
    error_observers: Vec<Arc<dyn ErrorObserver>>,
    route_metadata: Arc<RouteMetadata>,
}

impl InstanceWrapper {
    pub fn new(
        instance: Arc<Box<dyn Controller>>,
        enhancer_metadata: EnhancerMetadata,
        global_enhancers: EnhancerMetadata,
        error_observers: Vec<Arc<dyn ErrorObserver>>,
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
            error_observers,
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
        let observers_for_controller = self.error_observers.clone();
        let observers_for_middleware = self.error_observers.clone();
        let route_metadata = self.route_metadata.clone();

        let middleware_result = self
            .middleware_chain
            .execute(req, move |req| {
                let instance = instance.clone();
                let guards = guards.clone();
                let interceptors = interceptors.clone();
                let pipes = pipes.clone();
                let error_handlers = error_handlers_for_controller.clone();
                let observers = observers_for_controller.clone();
                let route_metadata = route_metadata.clone();

                Box::pin(async move {
                    Self::execute_controller_logic(
                        req,
                        instance,
                        guards,
                        interceptors,
                        pipes,
                        error_handlers,
                        observers,
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
                // If the middleware bubbled an `HttpError`, that's an
                // `AppError`-implementing user value — render it directly
                // without going through the chain.
                if let Some(http_err) = e.downcast_ref::<crate::errors::HttpError>() {
                    return http_err.into_http_response();
                }

                // Middleware failed before the request body could be split; we have no
                // parts to thread through to the handler context. Construct a stub
                // from a minimal request so error handlers still get a typed context.
                let stub = http::Request::builder().body(()).unwrap();
                let error_ctx = HttpContext::from_parts(stub.into_parts().0);
                let event = MiddlewareFailure::new(e.to_string());
                Self::fan_out_observers(&observers_for_middleware, &event, &error_ctx).await;
                for handler in error_handlers_for_middleware.iter().rev() {
                    if let Some(response) = handler.handle_error(&event, &error_ctx).await {
                        return response;
                    }
                }

                event.into_http_response()
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
        observers: Vec<Arc<dyn ErrorObserver>>,
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
                let event = GuardRejection::new(i);
                let claimed_response = context.take_response();
                return Self::handle_framework_event(
                    event,
                    claimed_response,
                    &error_handlers,
                    &observers,
                    &context,
                )
                .await;
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

    /// Run the chain on a typed framework event. Observers fan out first so
    /// they see every framework-generated error regardless of whether a chain
    /// handler claims it; if no handler claims, the event renders itself
    /// through its `AppError` impl. A `claimed_response` (for example, a
    /// custom response set by the rejecting guard) takes precedence over the
    /// canonical envelope.
    async fn handle_framework_event<E>(
        event: E,
        claimed_response: Option<HttpResponse>,
        error_handlers: &[HttpErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        ctx: &HttpContext,
    ) -> HttpResponse
    where
        E: AppError,
    {
        Self::fan_out_observers(observers, &event, ctx).await;

        for handler in error_handlers.iter().rev() {
            if let Some(handled) = handler.handle_error(&event, ctx).await {
                return handled;
            }
        }

        claimed_response.unwrap_or_else(|| event.into_http_response())
    }

    async fn fan_out_observers(
        observers: &[Arc<dyn ErrorObserver>],
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &dyn HandlerContext,
    ) {
        for observer in observers {
            observer.observe(error, ctx).await;
        }
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

}
