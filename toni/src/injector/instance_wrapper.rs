use std::sync::Arc;

use crate::{
    async_trait,
    context::{HandlerContext, HttpContext},
    errors::{
        Error, GuardRejection, HttpError, MiddlewareFailure, PanicRecovered, PipelineSegment,
    },
    http_helpers::{ExecutionResult, HttpMethod, HttpRequest, HttpResponse, RouteMetadata},
    middleware::{Middleware, MiddlewareChain},
    structs_helpers::EnhancerMetadata,
    traits_helpers::{
        ErrorObserver, Guard, HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry,
        HttpPipeEntry, Interceptor, InterceptorNext, Pipe, Route,
    },
};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// The next step in the interceptor chain after factory entries are resolved.
struct ChainNext {
    interceptors: Vec<Arc<dyn Interceptor<HttpContext>>>,
    instance: Arc<dyn Route>,
    pipes: Vec<Arc<dyn Pipe<HttpContext>>>,
    error_handlers: Vec<HttpErrorHandlerArc>,
    observers: Vec<Arc<dyn ErrorObserver>>,
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
            &self.error_handlers,
            &self.observers,
            &self.route_metadata,
        )
        .await;
    }
}

pub struct InstanceWrapper {
    instance: Arc<dyn Route>,
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
        instance: Arc<dyn Route>,
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

    pub async fn handle_request(&self, req: HttpRequest) -> HttpResponse {
        let method = self.get_method();
        let path = self.get_path();
        tracing::debug!(method = %method.as_str(), path = %path, "incoming request");

        let instance = self.instance.clone();
        let guards = self.guards.clone();
        let interceptors = self.interceptors.clone();
        let pipes = self.pipes.clone();
        let error_handlers = self.error_handlers.clone();
        let observers = self.error_observers.clone();
        let route_metadata = self.route_metadata.clone();

        let middleware_result = self
            .middleware_chain
            .execute(req, move |req| {
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
                // If the middleware bubbled an `HttpError`, render it directly
                // without going through the chain — the user constructed the
                // wire shape themselves.
                if let Some(http_err) = e.downcast_ref::<HttpError>() {
                    // Synthesize a stub context so observers still get a
                    // typed `HandlerContext` if the render panics — we
                    // haven't built the real one yet at this point in
                    // the pipeline.
                    let stub = http::Request::builder().body(()).unwrap();
                    let mut stub_ctx = HttpContext::from_parts(stub.into_parts().0);
                    return Self::safe_render(
                        || http_err.to_response(),
                        &self.error_observers,
                        &mut stub_ctx,
                    )
                    .await;
                }

                // Middleware failed before the request body could be split; we have no
                // parts to thread through to the handler context. Construct a stub
                // from a minimal request so error handlers still get a typed context.
                let stub = http::Request::builder().body(()).unwrap();
                let mut error_ctx = HttpContext::from_parts(stub.into_parts().0);
                let event = MiddlewareFailure::new(e.to_string());
                Self::fan_out_observers(&self.error_observers, &event, &mut error_ctx).await;
                for handler in self.error_handlers.iter().rev() {
                    if let Some(response) = Self::try_chain_handler(
                        handler,
                        &event,
                        &mut error_ctx,
                        &self.error_observers,
                    )
                    .await
                    {
                        return response;
                    }
                }

                Self::safe_render(
                    || crate::errors::http_error::render_error(&event),
                    &self.error_observers,
                    &mut error_ctx,
                )
                .await
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
        instance: Arc<dyn Route>,
        guards: Vec<HttpGuardEntry>,
        interceptors: Vec<HttpInterceptorEntry>,
        pipes: Vec<HttpPipeEntry>,
        error_handlers: Vec<HttpErrorHandlerArc>,
        observers: Vec<Arc<dyn ErrorObserver>>,
        route_metadata: Arc<RouteMetadata>,
    ) -> HttpResponse {
        // Split req so factory entries see parts before the context takes ownership.
        let (mut parts, body) = req.into_parts();
        // Install the request's provider cache before anything resolves against it: the
        // enhancer factories below and the controller build further down both reach it
        // through the parts, so a request-scoped provider injected into a guard and into
        // the controller is constructed once.
        crate::traits_helpers::RequestCache::install(&mut parts);
        let guards = Self::resolve_guards(&guards, &parts).await;
        let interceptors = Self::resolve_interceptors(&interceptors, &parts).await;
        let pipes = Self::resolve_pipes(&pipes, &parts).await;
        let req = HttpRequest::from_parts(parts, body);

        let mut context = HttpContext::new(req, route_metadata.clone());

        for (i, guard) in guards.iter().enumerate() {
            // `can_activate` is user code — catch panics so the request
            // doesn't tear down. A panicking guard is treated as a hard
            // rejection: observers see `PanicRecovered { during: Guard }`,
            // the chain renders the normal forbidden envelope.
            let activated = match crate::panic_recovery::catch_async(
                PipelineSegment::Guard,
                guard.can_activate(&mut context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    tracing::debug!(guard_index = i, "guard panicked");
                    let claimed_response = context.take_response();
                    return Self::handle_framework_event(
                        event,
                        claimed_response,
                        &error_handlers,
                        &observers,
                        &mut context,
                    )
                    .await;
                }
            };
            if !activated {
                tracing::debug!(guard_index = i, "guard rejected request");
                let event = GuardRejection::new(i);
                let claimed_response = context.take_response();
                return Self::handle_framework_event(
                    event,
                    claimed_response,
                    &error_handlers,
                    &observers,
                    &mut context,
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
            &error_handlers,
            &observers,
            &route_metadata,
        )
        .await;

        context.into_response()
    }

    /// Run the chain on a typed framework event. Observers fan out first so
    /// they see every framework-generated error regardless of whether a chain
    /// handler claims it; if no handler claims, the event renders itself
    /// through the active transport rendering. A `claimed_response` (for example, a
    /// custom response set by the rejecting guard) takes precedence over the
    /// canonical envelope.
    async fn handle_framework_event<E>(
        event: E,
        claimed_response: Option<HttpResponse>,
        error_handlers: &[HttpErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        ctx: &mut HttpContext,
    ) -> HttpResponse
    where
        E: Error,
    {
        Self::fan_out_observers(observers, &event, &mut *ctx).await;

        for handler in error_handlers.iter().rev() {
            if let Some(handled) =
                Self::try_chain_handler(handler, &event, &mut *ctx, observers).await
            {
                return handled;
            }
        }

        if let Some(claimed) = claimed_response {
            return claimed;
        }
        Self::safe_render(
            || crate::errors::http_error::render_error(&event),
            observers,
            ctx,
        )
        .await
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` fans `PanicRecovered { during: ErrorHandler }` to
    /// observers and returns `None` so the caller continues to the next
    /// handler. Without this, a single bad chain handler would kill the
    /// whole error-recovery path and the original error would never
    /// reach the fallback rendering.
    /// Drive the transport's error renderer with panic recovery. A panic
    /// inside `HttpError::to_response` (or the free-function
    /// `render_error`) would otherwise tear the dispatcher down — the
    /// renderer is the last thing standing between the framework and the
    /// wire, so there's nothing left to remap if it fails. Policy: fan
    /// `PanicRecovered { during: ResponseRendering }` to observers, then
    /// substitute a minimal hardcoded 500 envelope so the client still
    /// gets a structured reply.
    async fn safe_render<F>(
        render: F,
        observers: &[Arc<dyn ErrorObserver>],
        ctx: &mut HttpContext,
    ) -> HttpResponse
    where
        F: FnOnce() -> HttpResponse,
    {
        match crate::panic_recovery::catch_sync(PipelineSegment::ResponseRendering, render) {
            Ok(resp) => resp,
            Err(panic_event) => {
                Self::fan_out_observers(observers, &panic_event, ctx).await;
                Self::fallback_500_response()
            }
        }
    }

    /// Minimal hardcoded 500 used when the regular renderer panics.
    /// Built with simple constructors that don't themselves render
    /// user-supplied data, so a recursive panic here is structurally
    /// impossible.
    fn fallback_500_response() -> HttpResponse {
        HttpResponse {
            body: Some(crate::http_helpers::Body::text("Internal Server Error")),
            status: 500,
            headers: vec![],
        }
    }

    async fn try_chain_handler(
        handler: &HttpErrorHandlerArc,
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &mut HttpContext,
        observers: &[Arc<dyn ErrorObserver>],
    ) -> Option<HttpResponse> {
        match crate::panic_recovery::catch_async(
            PipelineSegment::ErrorHandler,
            handler.handle_error(error, &mut *ctx),
        )
        .await
        {
            Ok(opt) => opt,
            Err(panic_event) => {
                Self::fan_out_observers(observers, &panic_event, ctx).await;
                None
            }
        }
    }

    async fn fan_out_observers(
        observers: &[Arc<dyn ErrorObserver>],
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &mut dyn HandlerContext,
    ) {
        for observer in observers {
            // A panicking observer must not corrupt the dispatch path.
            // Catch the unwind, log via tracing (the observer system itself
            // is the thing that just failed, so we can't route it back
            // through observers), and continue to the next observer.
            let observe = AssertUnwindSafe(observer.observe(error, &mut *ctx));
            if let Err(payload) = observe.catch_unwind().await {
                let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                    *s
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.as_str()
                } else {
                    "<panic payload was not a string>"
                };
                tracing::error!(error = %error, panic = %msg, "error observer panicked");
            }
        }
    }

    /// Onion/Russian doll dispatch through the interceptor chain.
    async fn execute_with_interceptors(
        context: &mut HttpContext,
        interceptors: &[Arc<dyn Interceptor<HttpContext>>],
        instance: &Arc<dyn Route>,
        pipes: &[Arc<dyn Pipe<HttpContext>>],
        error_handlers: &[HttpErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        route_metadata: &Arc<RouteMetadata>,
    ) {
        if interceptors.is_empty() {
            Self::execute_handler(context, instance, pipes, error_handlers, observers).await;
            return;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = ChainNext {
            interceptors: rest.to_vec(),
            instance: instance.clone(),
            pipes: pipes.to_vec(),
            error_handlers: error_handlers.to_vec(),
            observers: observers.to_vec(),
            route_metadata: route_metadata.clone(),
        };

        if let Err(event) = crate::panic_recovery::catch_async(
            PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Self::record_pipeline_panic(context, error_handlers, observers, event).await;
        }
    }

    /// Surface a panicking pre-handler segment (interceptor or pipe)
    /// through the existing observer + chain pipeline: lift
    /// `PanicRecovered` into an `HttpError`, run observers, give error
    /// handlers first claim, and fall back to the default rendering. The
    /// panic never silently corrupts the response — it either gets
    /// remapped by a chain handler or rendered as a 500.
    async fn record_pipeline_panic(
        context: &mut HttpContext,
        error_handlers: &[HttpErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        event: PanicRecovered,
    ) {
        Self::fan_out_observers(observers, &event, &mut *context).await;
        for handler in error_handlers.iter().rev() {
            if let Some(claimed) =
                Self::try_chain_handler(handler, &event, &mut *context, observers).await
            {
                context.set_response(claimed);
                return;
            }
        }
        let rendered = Self::safe_render(
            || HttpError::from(event).to_response(),
            observers,
            &mut *context,
        )
        .await;
        context.set_response(rendered);
    }

    /// Run pipes + handler, then route the result.
    ///
    /// `Ok` goes straight to the context. On `Err`, observers fan out on the
    /// underlying error, the chain's most-specific handler gets first claim,
    /// and `HttpError::to_response` is the fallback envelope when none claims.
    async fn execute_handler(
        context: &mut HttpContext,
        instance: &Arc<dyn Route>,
        pipes: &[Arc<dyn Pipe<HttpContext>>],
        error_handlers: &[HttpErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
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
            // `pipe.process` is sync — `catch_sync` wraps it the same way
            // `catch_async` wraps async segments. A panic here routes
            // through the observer + chain pipeline; remaining pipes and
            // the handler are skipped.
            if let Err(event) =
                crate::panic_recovery::catch_sync(PipelineSegment::Pipe, || pipe.process(context))
            {
                Self::record_pipeline_panic(context, error_handlers, observers, event).await;
                return;
            }
            if context.should_abort() {
                return;
            }
        }

        tracing::trace!(pipe_count = pipes.len(), "executing controller handler");
        // An enhancer that read the body leaves none for the handler; it still
        // gets the request, with nothing in it.
        let req = match context.take_request() {
            Some(req) => req,
            None => crate::http_helpers::HttpRequest::from_parts(
                context.request().clone(),
                crate::http_helpers::RequestBody::empty(),
            ),
        };
        // An enhancer may already have set one; only a response that appears
        // across the handler call is the handler's doing.
        let response_before_handler = context.response().is_some();
        // `AssertUnwindSafe`: handler bodies aren't required to be unwind-safe
        // and adding `RefUnwindSafe` bounds to user code would be punitive.
        // We trust the application to set its own state to a sane shape after
        // a panic — this layer only ensures the panic doesn't escape the
        // dispatcher.
        let exec_result = AssertUnwindSafe(instance.execute(req, &mut *context))
            .catch_unwind()
            .await;
        // A handler answers by returning, and what it returns is written over
        // the context below — so a response it set there is silently discarded.
        // Legal to write and never what the author meant.
        if !response_before_handler && context.response().is_some() {
            tracing::warn!(
                method = %instance.get_method().as_str(),
                path = %instance.get_path(),
                "handler set a response on the context; the value it returned is sent instead. \
                 Return the response, or short-circuit from a guard or interceptor."
            );
        }
        let exec_result = match exec_result {
            Ok(result) => result,
            Err(payload) => {
                let event =
                    PanicRecovered::from_panic_payload(PipelineSegment::HandlerBody, payload);
                // Lift the framework event into HttpError via the From blanket.
                ExecutionResult::Err(HttpError::from(event))
            }
        };
        match exec_result {
            ExecutionResult::Ok(response) => context.set_response(response),
            ExecutionResult::Err(http_err) => {
                // For chain dispatch + observers, expose the underlying domain
                // error (when wrapped) so downcasting against `MyError`
                // continues to work the way `#[catch(MyError)]` expects. For
                // non-`AppError` variants (named HttpError variants), pass the
                // HttpError itself.
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) = match &http_err
                {
                    HttpError::AppError(e) => e.as_ref(),
                    other => other,
                };
                Self::fan_out_observers(observers, observed_err, &mut *context).await;
                for handler in error_handlers.iter().rev() {
                    if let Some(claimed) =
                        Self::try_chain_handler(handler, observed_err, &mut *context, observers)
                            .await
                    {
                        context.set_response(claimed);
                        return;
                    }
                }
                let rendered =
                    Self::safe_render(|| http_err.to_response(), observers, &mut *context).await;
                context.set_response(rendered);
            }
        }
    }
}
