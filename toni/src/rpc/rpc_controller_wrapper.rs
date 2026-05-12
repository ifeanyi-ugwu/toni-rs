use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::Error;
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
        for guard in &guards {
            if !guard.can_activate(&ctx).await {
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

        first.intercept(context, Box::new(next)).await;
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
            pipe.process(context);
            if context.should_abort() {
                // Pipe abort blocks the handler from running — surface as a
                // wire-level Err so adapters can frame it as an "the
                // framework couldn't process this" outcome, parallel to
                // guard rejection. User-error responses use the Ok+envelope
                // path; this isn't a user error.
                context.set_response(Err(RpcError::Internal(
                    "Request aborted by pipe".into(),
                )));
                return;
            }
        }

        let exec_result = AssertUnwindSafe(controller.handle_message(context))
            .catch_unwind()
            .await;
        let exec_result = match exec_result {
            Ok(result) => result,
            Err(payload) => {
                let event = PanicRecovered::from_panic_payload(
                    PipelineSegment::HandlerBody,
                    payload,
                );
                ExecutionResult::Err(RpcError::from(event))
            }
        };
        match exec_result {
            ExecutionResult::Ok(data) => context.set_response(Ok(data)),
            ExecutionResult::Err(rpc_err) => {
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) =
                    match &rpc_err {
                        RpcError::AppError(e) => e.as_ref(),
                        other => other,
                    };
                Self::fan_out_observers(observers, observed_err, context).await;
                for handler in error_handlers.iter().rev() {
                    if let Some(claimed) =
                        handler.handle_error(observed_err, context).await
                    {
                        context.set_response(Ok(Some(claimed)));
                        return;
                    }
                }
                context.set_response(Ok(Some(rpc_err.to_data())));
            }
        }
    }
}
