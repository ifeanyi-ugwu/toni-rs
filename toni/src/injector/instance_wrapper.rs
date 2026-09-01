use std::sync::Arc;

use crate::{
    async_trait,
    context::Metadata,
    context::{HandlerContext, HttpContext},
    errors::{
        Error, GuardRejection, HttpError, MiddlewareFailure, PanicRecovered, PipelineSegment,
    },
    http_helpers::{ExecutionResult, HttpMethod, HttpRequest, HttpResponse},
    middleware::{Middleware, MiddlewareChain},
    structs_helpers::EnhancerMetadata,
    traits_helpers::{
        Guard, HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry, Interceptor,
        InterceptorNext, Route,
    },
};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// The next step in the interceptor chain after factory entries are resolved.
struct ChainNext {
    interceptors: Vec<Arc<dyn Interceptor<HttpContext, HttpResponse>>>,
    instance: Arc<dyn Route>,
    error_handlers: Vec<HttpErrorHandlerArc>,
    metadata: Arc<Metadata>,
}

#[async_trait]
impl InterceptorNext<HttpContext, HttpResponse> for ChainNext {
    async fn run(self: Box<Self>, context: &HttpContext) -> HttpResponse {
        InstanceWrapper::execute_with_interceptors(
            context,
            &self.interceptors,
            &self.instance,
            &self.error_handlers,
            &self.metadata,
        )
        .await
    }
}

pub struct InstanceWrapper {
    instance: Arc<dyn Route>,
    guards: Vec<HttpGuardEntry>,
    interceptors: Vec<HttpInterceptorEntry>,
    middleware_chain: MiddlewareChain,
    error_handlers: Vec<HttpErrorHandlerArc>,
    metadata: Arc<Metadata>,
}

impl InstanceWrapper {
    pub fn new(
        instance: Arc<dyn Route>,
        enhancer_metadata: EnhancerMetadata,
        global_enhancers: EnhancerMetadata,
    ) -> Self {
        // Execution order: global → controller → method
        let mut guards = global_enhancers.guards;
        guards.extend(enhancer_metadata.guards);

        let mut interceptors = global_enhancers.interceptors;
        interceptors.extend(enhancer_metadata.interceptors);

        let mut error_handlers = global_enhancers.error_handlers;
        error_handlers.extend(enhancer_metadata.error_handlers);

        let metadata = instance.metadata();

        Self {
            instance,
            guards,
            interceptors,
            middleware_chain: MiddlewareChain::new(),
            error_handlers,
            metadata,
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
        let error_handlers = self.error_handlers.clone();
        let metadata = self.metadata.clone();

        let middleware_result = self
            .middleware_chain
            .execute(req, move |req| {
                Box::pin(async move {
                    Self::execute_controller_logic(
                        req,
                        instance,
                        guards,
                        interceptors,
                        error_handlers,
                        metadata,
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
                    return Self::safe_render(|| http_err.to_response());
                }

                // Middleware failed before the request body could be split; we have no
                // parts to thread through to the handler context. Construct a stub
                // from a minimal request so error handlers still get a typed context.
                let stub = http::Request::builder().body(()).unwrap();
                let error_ctx = HttpContext::from_parts(stub.into_parts().0);

                // A caught middleware panic keeps its own type through the
                // chain, so `#[catch(PanicRecovered)]` matches it the way it
                // matches one from a guard or an interceptor.
                let e = match e.downcast::<PanicRecovered>() {
                    Ok(panic) => {
                        return Self::handle_framework_event(
                            *panic,
                            &self.error_handlers,
                            &error_ctx,
                        )
                        .await;
                    }
                    Err(e) => e,
                };

                let event = MiddlewareFailure::new(e.to_string());
                for (position, handler) in self.error_handlers.iter().rev().enumerate() {
                    if let Some(response) =
                        Self::try_chain_handler(handler, &event, &error_ctx, position).await
                    {
                        return response;
                    }
                }

                Self::safe_render(|| crate::errors::http_error::render_error(&event))
            }
        }
    }

    async fn resolve_guards(
        entries: &[HttpGuardEntry],
        ctx: &HttpContext,
    ) -> Vec<Arc<dyn Guard<HttpContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                HttpGuardEntry::Ready(g) => g.clone(),
                HttpGuardEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(
        entries: &[HttpInterceptorEntry],
        ctx: &HttpContext,
    ) -> Vec<Arc<dyn Interceptor<HttpContext, HttpResponse>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                HttpInterceptorEntry::Ready(i) => i.clone(),
                HttpInterceptorEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(i);
        }
        out
    }

    async fn execute_controller_logic(
        req: HttpRequest,
        instance: Arc<dyn Route>,
        guards: Vec<HttpGuardEntry>,
        interceptors: Vec<HttpInterceptorEntry>,
        error_handlers: Vec<HttpErrorHandlerArc>,
        metadata: Arc<Metadata>,
    ) -> HttpResponse {
        // The context comes first now: it owns the execution's cache, so a
        // request-scoped provider injected into a guard and into the controller
        // is constructed once only if both resolve against the same one.
        let context = HttpContext::new(req, metadata.clone());

        let guards = Self::resolve_guards(&guards, &context).await;
        let interceptors = Self::resolve_interceptors(&interceptors, &context).await;

        let response = Self::run_chain(
            &context,
            instance,
            guards,
            interceptors,
            error_handlers,
            metadata,
        )
        .await;

        // The execution ends when the answer does. A streaming body has produced
        // nothing yet at this point, so the context rides it to the last frame
        // rather than dying here with the handler.
        match response.body {
            Some(body) => {
                // Dropped owing frames, the client is gone. Work feeding the body escaped the
                // handler's future and would otherwise learn only at its next send.
                let cancellation = context.cancellation().clone();
                HttpResponse {
                    body: Some(
                        body.keep_alive(context)
                            .on_abandoned(move || cancellation.cancel()),
                    ),
                    ..response
                }
            }
            None => response,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_chain(
        context: &HttpContext,
        instance: Arc<dyn Route>,
        guards: Vec<Arc<dyn Guard<HttpContext>>>,
        interceptors: Vec<Arc<dyn Interceptor<HttpContext, HttpResponse>>>,
        error_handlers: Vec<HttpErrorHandlerArc>,
        metadata: Arc<Metadata>,
    ) -> HttpResponse {
        for (i, guard) in guards.iter().enumerate() {
            // `can_activate` is user code — catch panics so the request
            // doesn't tear down. A panicking guard is treated as a hard
            // rejection: the chain gets `PanicRecovered { during: Guard }`
            // and renders the normal forbidden envelope.
            let activated = match crate::panic_recovery::catch_async(
                PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    tracing::debug!(guard_index = i, "guard panicked");
                    return Self::handle_framework_event(event, &error_handlers, context).await;
                }
            };
            if !activated {
                tracing::debug!(guard_index = i, "guard rejected request");
                let event = GuardRejection::new(i);
                return Self::handle_framework_event(event, &error_handlers, context).await;
            }
        }

        if !interceptors.is_empty() {
            tracing::trace!(count = interceptors.len(), "entering interceptor chain");
        }
        Self::execute_with_interceptors(
            context,
            &interceptors,
            &instance,
            &error_handlers,
            &metadata,
        )
        .await
    }

    /// Run the chain on a typed framework event. If nothing claims it, the
    /// event renders itself through the active transport rendering. Reshape a
    /// rejection with `#[catch(GuardRejection)]`.
    async fn handle_framework_event<E>(
        event: E,
        error_handlers: &[HttpErrorHandlerArc],
        ctx: &HttpContext,
    ) -> HttpResponse
    where
        E: Error,
    {
        for (position, handler) in error_handlers.iter().rev().enumerate() {
            if let Some(handled) = Self::try_chain_handler(handler, &event, ctx, position).await {
                return handled;
            }
        }

        Self::safe_render(|| crate::errors::http_error::render_error(&event))
    }

    /// Drive the transport's error renderer with panic recovery. A panic
    /// inside `HttpError::to_response` (or the free-function
    /// `render_error`) would otherwise tear the dispatcher down — the
    /// renderer is the last thing standing between the framework and the
    /// wire, so there's nothing left to remap if it fails. Policy: log the
    /// panic and substitute a minimal hardcoded 500 envelope so the client
    /// still gets a structured reply.
    fn safe_render<F>(render: F) -> HttpResponse
    where
        F: FnOnce() -> HttpResponse,
    {
        match crate::panic_recovery::catch_sync(PipelineSegment::ResponseRendering, render) {
            Ok(resp) => resp,
            Err(panic_event) => {
                tracing::error!(panic = %panic_event.message, "error renderer panicked; falling back to a bare 500");
                Self::fallback_500_response()
            }
        }
    }

    /// Minimal hardcoded 500 used when the regular renderer panics.
    ///
    /// The envelope is a string literal rather than a rendered value: the
    /// renderer that just panicked was calling the error's own `kind()` and
    /// `message()`, and this calls neither, so a recursive panic here is
    /// structurally impossible. It keeps the canonical shape because a client
    /// decoding `{statusCode, message, error}` should not meet a different
    /// body on the one path it cannot anticipate.
    fn fallback_500_response() -> HttpResponse {
        HttpResponse {
            body: Some(
                crate::http_helpers::Body::text(
                    r#"{"statusCode":500,"message":"Internal Server Error","error":"Internal Server Error"}"#,
                )
                .with_content_type("application/json"),
            ),
            status: 500,
            headers: vec![],
        }
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` is logged and answers `None`, so the caller continues
    /// to the next handler. Without this, a single bad chain handler would
    /// kill the whole error-recovery path and the original error would
    /// never reach the fallback rendering.
    ///
    /// `position` counts from the most specific handler — the chain runs
    /// method, then controller, then global — and is logged so a panic names
    /// which registration it came from.
    async fn try_chain_handler(
        handler: &HttpErrorHandlerArc,
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &HttpContext,
        position: usize,
    ) -> Option<HttpResponse> {
        match crate::panic_recovery::catch_async(
            PipelineSegment::ErrorHandler,
            handler.handle_error(error, ctx),
        )
        .await
        {
            Ok(opt) => opt,
            Err(panic_event) => {
                tracing::error!(chain_position = position, error = %error, panic = %panic_event.message, "error handler panicked; trying the next one");
                None
            }
        }
    }

    /// Onion/Russian doll dispatch through the interceptor chain.
    async fn execute_with_interceptors(
        context: &HttpContext,
        interceptors: &[Arc<dyn Interceptor<HttpContext, HttpResponse>>],
        instance: &Arc<dyn Route>,
        error_handlers: &[HttpErrorHandlerArc],
        metadata: &Arc<Metadata>,
    ) -> HttpResponse {
        if interceptors.is_empty() {
            return Self::execute_handler(context, instance, error_handlers).await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = ChainNext {
            interceptors: rest.to_vec(),
            instance: instance.clone(),
            error_handlers: error_handlers.to_vec(),
            metadata: metadata.clone(),
        };

        match crate::panic_recovery::catch_async(
            PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(response) => response,
            Err(event) => Self::record_pipeline_panic(context, error_handlers, event).await,
        }
    }

    /// Surface a panicking pre-handler segment (an interceptor) through the
    /// chain: lift `PanicRecovered` into an `HttpError`, give error handlers
    /// first claim, and fall back to the default rendering. The panic reaches
    /// the client either way — remapped by a chain handler, or as a 500.
    async fn record_pipeline_panic(
        context: &HttpContext,
        error_handlers: &[HttpErrorHandlerArc],
        event: PanicRecovered,
    ) -> HttpResponse {
        for (position, handler) in error_handlers.iter().rev().enumerate() {
            if let Some(claimed) = Self::try_chain_handler(handler, &event, context, position).await
            {
                return claimed;
            }
        }
        Self::safe_render(|| HttpError::from(event).to_response())
    }

    /// Run the handler, then route the result.
    ///
    /// `Ok` is the answer. On `Err`, the chain's most-specific handler gets
    /// first claim on the underlying error, and `HttpError::to_response` is
    /// the fallback envelope when none claims.
    async fn execute_handler(
        context: &HttpContext,
        instance: &Arc<dyn Route>,
        error_handlers: &[HttpErrorHandlerArc],
    ) -> HttpResponse {
        tracing::trace!("executing controller handler");
        // `AssertUnwindSafe`: handler bodies aren't required to be unwind-safe
        // and adding `RefUnwindSafe` bounds to user code would be punitive.
        // We trust the application to set its own state to a sane shape after
        // a panic — this layer only ensures the panic doesn't escape the
        // dispatcher.
        let exec_result = AssertUnwindSafe(instance.execute(context))
            .catch_unwind()
            .await;
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
            ExecutionResult::Ok(response) => response,
            ExecutionResult::Err(http_err) => {
                // For chain dispatch, expose the underlying domain error
                // (when wrapped) so downcasting against `MyError`
                // continues to work the way `#[catch(MyError)]` expects. For
                // non-`AppError` variants (named HttpError variants), pass the
                // HttpError itself.
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) = match &http_err
                {
                    HttpError::AppError(e) => e.as_ref(),
                    other => other,
                };
                for (position, handler) in error_handlers.iter().rev().enumerate() {
                    if let Some(claimed) =
                        Self::try_chain_handler(handler, observed_err, context, position).await
                    {
                        return claimed;
                    }
                }
                Self::safe_render(|| http_err.to_response())
            }
        }
    }
}
