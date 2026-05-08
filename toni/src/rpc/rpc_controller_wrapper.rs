use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::context::{HandlerContext, RpcContext};
use crate::http_helpers::RouteMetadata;
use crate::traits_helpers::{
    Guard, Interceptor, InterceptorNext, Pipe, RpcErrorHandlerArc, RpcGuardEntry,
    RpcInterceptorEntry, RpcPipeEntry,
};

use super::{RpcControllerTrait, RpcData, RpcError};

struct RpcChainNext {
    interceptors: Vec<Arc<dyn Interceptor<RpcContext>>>,
    controller: Arc<Box<dyn RpcControllerTrait>>,
    pipes: Vec<Arc<dyn Pipe<RpcContext>>>,
    error_handlers: Vec<RpcErrorHandlerArc>,
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

        let guards = Self::resolve_guards(&all_guards).await;
        for guard in &guards {
            if !guard.can_activate(&ctx).await {
                return Err(RpcError::Forbidden("Guard rejected message".into()));
            }
            if ctx.should_abort() {
                return Err(RpcError::Forbidden("Message aborted by guard".into()));
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
        )
        .await
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
    ) -> Result<Option<RpcData>, RpcError> {
        Self::execute_with_interceptors_impl(
            context,
            interceptors,
            controller,
            pipes,
            error_handlers,
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
    ) {
        if interceptors.is_empty() {
            Self::execute_handler(context, controller, pipes).await;
            if !error_handlers.is_empty() {
                let needs_recovery = matches!(context.response(), Some(Err(_)));
                if needs_recovery {
                    let Some(Err(e)) = context.take_response() else {
                        return;
                    };
                    let error_msg = e.to_string();
                    let error =
                        std::io::Error::new(std::io::ErrorKind::Other, error_msg.clone());
                    for handler in error_handlers.iter().rev() {
                        if let Some(data) = handler.handle_error(&error, context).await {
                            context.set_response(Ok(Some(data)));
                            return;
                        }
                    }
                    // No handler claimed it — restore the original error.
                    context.set_response(Err(RpcError::Internal(error_msg)));
                }
            }
            return;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = RpcChainNext {
            interceptors: rest.to_vec(),
            controller: controller.clone(),
            pipes: pipes.to_vec(),
            error_handlers: error_handlers.to_vec(),
        };

        first.intercept(context, Box::new(next)).await;
    }

    async fn execute_handler(
        context: &mut RpcContext,
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        pipes: &[Arc<dyn Pipe<RpcContext>>],
    ) {
        for pipe in pipes {
            pipe.process(context);
            if context.should_abort() {
                context.set_response(Err(RpcError::Internal("Request aborted by pipe".into())));
                return;
            }
        }

        let result = controller.handle_message(context).await;
        context.set_response(result);
    }
}
