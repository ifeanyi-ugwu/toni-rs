use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::context::{HandlerContext, RpcContext};
use crate::errors::{PanicRecovered, PipelineSegment};
use crate::http_helpers::{ExecutionResult, RouteMetadata};
use crate::traits_helpers::{
    ErrorObserver, Guard, Interceptor, InterceptorNext, Pipe, RpcErrorHandlerArc, RpcGuardEntry,
    RpcInterceptorEntry, RpcPipeEntry,
};

use super::{RpcControllerTrait, RpcData, RpcError};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

struct RpcChainNext {
    interceptors: Vec<Arc<dyn Interceptor<RpcContext>>>,
    controller: Arc<Box<dyn RpcControllerTrait>>,
    pipes: Vec<Arc<dyn Pipe<RpcContext>>>,
    error_handlers: Vec<RpcErrorHandlerArc>,
    observers: Vec<Arc<dyn ErrorObserver>>,
}

#[async_trait]
impl InterceptorNext<RpcContext> for RpcChainNext {
    async fn run(self: Box<Self>, context: &mut RpcContext) {
        RpcControllerWrapper::execute_with_interceptors_impl(
            context,
            &self.interceptors,
            &self.controller,
            &self.pipes,
            &self.error_handlers,
            &self.observers,
        )
        .await;
    }
}

/// Wraps an [`RpcControllerTrait`] with the full guard/interceptor/pipe pipeline.
pub struct RpcControllerWrapper {
    controller: Arc<Box<dyn RpcControllerTrait>>,
    guards: Vec<RpcGuardEntry>,
    interceptors: Vec<RpcInterceptorEntry>,
    pipes: Vec<RpcPipeEntry>,
    error_handlers: Vec<RpcErrorHandlerArc>,
    error_observers: Vec<Arc<dyn ErrorObserver>>,
    route_metadata: Arc<RouteMetadata>,
    handler_guards: HashMap<String, Vec<RpcGuardEntry>>,
    handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>>,
    handler_pipes: HashMap<String, Vec<RpcPipeEntry>>,
    handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>>,
}

impl RpcControllerWrapper {
    pub fn new(
        controller: Arc<Box<dyn RpcControllerTrait>>,
        guards: Vec<RpcGuardEntry>,
        interceptors: Vec<RpcInterceptorEntry>,
        pipes: Vec<RpcPipeEntry>,
        error_handlers: Vec<RpcErrorHandlerArc>,
        error_observers: Vec<Arc<dyn ErrorObserver>>,
        route_metadata: Arc<RouteMetadata>,
        handler_guards: HashMap<String, Vec<RpcGuardEntry>>,
        handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>>,
        handler_pipes: HashMap<String, Vec<RpcPipeEntry>>,
        handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>>,
    ) -> Self {
        Self {
            controller,
            guards,
            interceptors,
            pipes,
            error_handlers,
            error_observers,
            route_metadata,
            handler_guards,
            handler_interceptors,
            handler_pipes,
            handler_error_handlers,
        }
    }

    pub fn get_patterns(&self) -> Vec<String> {
        self.controller.get_patterns()
    }

    pub async fn handle_message(
        &self,
        data: RpcData,
        call_metadata: HashMap<String, String>,
        pattern: String,
    ) -> Result<Option<RpcData>, RpcError> {
        let mut ctx = RpcContext::new(pattern.clone(), data, Some(self.route_metadata.clone()));
        *ctx.metadata_mut() = call_metadata;

        let mut all_guards = self.guards.clone();
        if let Some(h) = self.handler_guards.get(&pattern) {
            all_guards.extend_from_slice(h);
        }
        let mut all_interceptors = self.interceptors.clone();
        if let Some(h) = self.handler_interceptors.get(&pattern) {
            all_interceptors.extend_from_slice(h);
        }
        let mut all_pipes = self.pipes.clone();
        if let Some(h) = self.handler_pipes.get(&pattern) {
            all_pipes.extend_from_slice(h);
        }
        let mut all_error_handlers = self.error_handlers.clone();
        if let Some(h) = self.handler_error_handlers.get(&pattern) {
            all_error_handlers.extend_from_slice(h);
        }
        let observers = self.error_observers.clone();

        let guards = Self::resolve_guards(&all_guards).await;
        for (index, guard) in guards.iter().enumerate() {
            // Treat a panic in `can_activate` as a hard rejection: observers
            // see `PanicRecovered { during: Guard }` and the caller gets
            // `RpcError::Forbidden` instead of a torn-down dispatcher.
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&ctx),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    Self::fan_out_observers(&observers, &event, &ctx).await;
                    return Err(RpcError::Forbidden(format!(
                        "guard {} panicked: {}",
                        index, event.message
                    )));
                }
            };
            if !activated {
                let err = RpcError::Forbidden("Guard rejected message".into());
                Self::fan_out_observers(&observers, &err, &ctx).await;
                return Err(err);
            }
            if ctx.should_abort() {
                let err = RpcError::Forbidden("Message aborted by guard".into());
                Self::fan_out_observers(&observers, &err, &ctx).await;
                return Err(err);
            }
        }

        let interceptors = Self::resolve_interceptors(&all_interceptors).await;
        let pipes = Self::resolve_pipes(&all_pipes).await;
        Self::execute_with_interceptors(
            &mut ctx,
            &self.controller,
            &interceptors,
            &pipes,
            &all_error_handlers,
            &observers,
        )
        .await
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` fans `PanicRecovered { during: ErrorHandler }` to
    /// observers and returns `None` so the caller continues to the next
    /// handler. Without this, a single bad chain handler would kill the
    /// whole error-recovery path and the original error would never
    /// reach the fallback `to_data` rendering.
    /// Drive `RpcError::to_data` with panic recovery — a panic in the
    /// renderer is the last thing the framework can do for the caller,
    /// so we substitute a hardcoded `Internal` envelope and fan
    /// `PanicRecovered { during: ResponseRendering }` to observers.
    async fn safe_render<F>(
        render: F,
        observers: &[Arc<dyn ErrorObserver>],
        ctx: &RpcContext,
    ) -> RpcData
    where
        F: FnOnce() -> RpcData,
    {
        match crate::panic_recovery::catch_sync(
            crate::errors::PipelineSegment::ResponseRendering,
            render,
        ) {
            Ok(data) => data,
            Err(panic_event) => {
                Self::fan_out_observers(observers, &panic_event, ctx).await;
                Self::fallback_internal_data()
            }
        }
    }

    /// Hardcoded fallback envelope when the regular renderer panics.
    /// Uses [`RpcData::text`] so no user-supplied serialiser runs.
    fn fallback_internal_data() -> RpcData {
        RpcData::text("Internal Server Error")
    }

    async fn try_chain_handler(
        handler: &RpcErrorHandlerArc,
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &RpcContext,
        observers: &[Arc<dyn ErrorObserver>],
    ) -> Option<RpcData> {
        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::ErrorHandler,
            handler.handle_error(error, ctx),
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
        ctx: &RpcContext,
    ) {
        for observer in observers {
            let observe = AssertUnwindSafe(observer.observe(error, ctx));
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

    /// RPC has no HTTP request; factory entries are called with `None`.
    /// Factory guards with `requires_http_parts() == true` should have been
    /// rejected at startup by the resolver.
    async fn resolve_guards(entries: &[RpcGuardEntry]) -> Vec<Arc<dyn Guard<RpcContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                RpcGuardEntry::Ready(g) => g.clone(),
                RpcGuardEntry::Factory(f) => f.create(None).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(
        entries: &[RpcInterceptorEntry],
    ) -> Vec<Arc<dyn Interceptor<RpcContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                RpcInterceptorEntry::Ready(i) => i.clone(),
                RpcInterceptorEntry::Factory(f) => f.create(None).await,
            };
            out.push(i);
        }
        out
    }

    async fn resolve_pipes(entries: &[RpcPipeEntry]) -> Vec<Arc<dyn Pipe<RpcContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let p = match entry {
                RpcPipeEntry::Ready(p) => p.clone(),
                RpcPipeEntry::Factory(f) => f.create(None).await,
            };
            out.push(p);
        }
        out
    }

    async fn execute_with_interceptors(
        context: &mut RpcContext,
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        interceptors: &[Arc<dyn Interceptor<RpcContext>>],
        pipes: &[Arc<dyn Pipe<RpcContext>>],
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) -> Result<Option<RpcData>, RpcError> {
        Self::execute_with_interceptors_impl(
            context,
            interceptors,
            controller,
            pipes,
            error_handlers,
            observers,
        )
        .await;

        if context.should_abort() {
            if let Some(response) = context.take_response() {
                return response.map(|opt| opt);
            }
            return Err(RpcError::Internal(
                "Request aborted by interceptor without response".into(),
            ));
        }

        if let Some(response) = context.take_response() {
            response.map(|opt| opt)
        } else {
            Err(RpcError::Internal("Handler did not set response".into()))
        }
    }

    async fn execute_with_interceptors_impl(
        context: &mut RpcContext,
        interceptors: &[Arc<dyn Interceptor<RpcContext>>],
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        pipes: &[Arc<dyn Pipe<RpcContext>>],
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) {
        if interceptors.is_empty() {
            Self::execute_handler(context, controller, pipes, error_handlers, observers).await;
            return;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = RpcChainNext {
            interceptors: rest.to_vec(),
            controller: controller.clone(),
            pipes: pipes.to_vec(),
            error_handlers: error_handlers.to_vec(),
            observers: observers.to_vec(),
        };

        if let Err(event) = crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Self::record_pipeline_panic(context, error_handlers, observers, event).await;
        }
    }

    /// Surface a panicking pre-handler segment (interceptor or pipe)
    /// through the existing observer + chain pipeline: fan to observers,
    /// give error handlers first claim, and fall back to a wire-`Err`
    /// Internal envelope.
    async fn record_pipeline_panic(
        context: &mut RpcContext,
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        event: PanicRecovered,
    ) {
        Self::fan_out_observers(observers, &event, context).await;
        for handler in error_handlers.iter().rev() {
            if let Some(claimed) =
                Self::try_chain_handler(handler, &event, context, observers).await
            {
                context.set_response(Ok(Some(claimed)));
                return;
            }
        }
        let rpc_err = RpcError::from(event);
        let data = Self::safe_render(|| rpc_err.to_data(), observers, context).await;
        context.set_response(Ok(Some(data)));
    }

    /// Run pipes + handler, then route the result.
    ///
    /// `Ok` goes straight to the context. On `Err`, observers fan out on the
    /// underlying error, the chain's most-specific handler gets first claim,
    /// and `RpcError::to_data` is the fallback envelope when none claims.
    async fn execute_handler(
        context: &mut RpcContext,
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        pipes: &[Arc<dyn Pipe<RpcContext>>],
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) {
        for pipe in pipes {
            // `pipe.process` is sync — `catch_sync` wraps it the same way
            // `catch_async` wraps async segments. A panic routes through
            // the observer + chain pipeline; remaining pipes and the
            // handler are skipped.
            if let Err(event) =
                crate::panic_recovery::catch_sync(crate::errors::PipelineSegment::Pipe, || {
                    pipe.process(context)
                })
            {
                Self::record_pipeline_panic(context, error_handlers, observers, event).await;
                return;
            }
            if context.should_abort() {
                // Pipe abort blocks the handler from running — surface as a
                // wire-level Err so adapters can frame it as an "the
                // framework couldn't process this" outcome, parallel to
                // guard rejection. User-error responses use the Ok+envelope
                // path; this isn't a user error.
                context.set_response(Err(RpcError::Internal("Request aborted by pipe".into())));
                return;
            }
        }

        let exec_result = AssertUnwindSafe(controller.handle_message(context))
            .catch_unwind()
            .await;
        let exec_result = match exec_result {
            Ok(result) => result,
            Err(payload) => {
                let event =
                    PanicRecovered::from_panic_payload(PipelineSegment::HandlerBody, payload);
                ExecutionResult::Err(RpcError::from(event))
            }
        };
        match exec_result {
            ExecutionResult::Ok(data) => context.set_response(Ok(data)),
            ExecutionResult::Err(rpc_err) => {
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) = match &rpc_err
                {
                    RpcError::AppError(e) => e.as_ref(),
                    other => other,
                };
                Self::fan_out_observers(observers, observed_err, context).await;
                for handler in error_handlers.iter().rev() {
                    if let Some(claimed) =
                        Self::try_chain_handler(handler, observed_err, context, observers).await
                    {
                        context.set_response(Ok(Some(claimed)));
                        return;
                    }
                }
                let data = Self::safe_render(|| rpc_err.to_data(), observers, context).await;
                context.set_response(Ok(Some(data)));
            }
        }
    }
}
