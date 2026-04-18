use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::http_helpers::RouteMetadata;
use crate::injector::Context;
use crate::traits_helpers::{
    ErrorHandler, Guard, GuardEntry, Interceptor, InterceptorEntry, InterceptorNext, Pipe,
    PipeEntry,
};

use super::{RpcContext, RpcControllerTrait, RpcData, RpcError};

struct RpcChainNext {
    interceptors: Vec<Arc<dyn Interceptor>>,
    controller: Arc<Box<dyn RpcControllerTrait>>,
    pipes: Vec<Arc<dyn Pipe>>,
    error_handlers: Vec<Arc<dyn ErrorHandler>>,
}

#[async_trait]
impl InterceptorNext for RpcChainNext {
    async fn run(self: Box<Self>, context: &mut Context) {
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
///
/// Parallel to [`GatewayWrapper`] for WebSocket — the framework constructs one
/// wrapper per discovered RPC controller and routes all incoming messages through it.
///
/// [`GatewayWrapper`]: crate::websocket::GatewayWrapper
pub struct RpcControllerWrapper {
    controller: Arc<Box<dyn RpcControllerTrait>>,
    guards: Vec<GuardEntry>,
    interceptors: Vec<InterceptorEntry>,
    pipes: Vec<PipeEntry>,
    error_handlers: Vec<Arc<dyn ErrorHandler>>,
    route_metadata: Arc<RouteMetadata>,
    /// Per-handler enhancers keyed by pattern, pre-resolved at startup.
    /// Appended after controller-level enhancers when dispatching a message.
    handler_guards: HashMap<String, Vec<GuardEntry>>,
    handler_interceptors: HashMap<String, Vec<InterceptorEntry>>,
    handler_pipes: HashMap<String, Vec<PipeEntry>>,
    handler_error_handlers: HashMap<String, Vec<Arc<dyn ErrorHandler>>>,
}

impl RpcControllerWrapper {
    pub fn new(
        controller: Arc<Box<dyn RpcControllerTrait>>,
        guards: Vec<GuardEntry>,
        interceptors: Vec<InterceptorEntry>,
        pipes: Vec<PipeEntry>,
        error_handlers: Vec<Arc<dyn ErrorHandler>>,
        route_metadata: Arc<RouteMetadata>,
        handler_guards: HashMap<String, Vec<GuardEntry>>,
        handler_interceptors: HashMap<String, Vec<InterceptorEntry>>,
        handler_pipes: HashMap<String, Vec<PipeEntry>>,
        handler_error_handlers: HashMap<String, Vec<Arc<dyn ErrorHandler>>>,
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
        context: RpcContext,
    ) -> Result<Option<RpcData>, RpcError> {
        let pattern = context.pattern.clone();
        let mut ctx = Context::from_rpc(data, context, Some(self.route_metadata.clone()));

        // Merge controller-level + handler-level entries (handler appended after controller).
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
    async fn resolve_guards(entries: &[GuardEntry]) -> Vec<Arc<dyn Guard>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                GuardEntry::Ready(g) => g.clone(),
                GuardEntry::Factory(f) => f.create(None).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(entries: &[InterceptorEntry]) -> Vec<Arc<dyn Interceptor>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                InterceptorEntry::Ready(i) => i.clone(),
                InterceptorEntry::Factory(f) => f.create(None).await,
            };
            out.push(i);
        }
        out
    }

    async fn resolve_pipes(entries: &[PipeEntry]) -> Vec<Arc<dyn Pipe>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let p = match entry {
                PipeEntry::Ready(p) => p.clone(),
                PipeEntry::Factory(f) => f.create(None).await,
            };
            out.push(p);
        }
        out
    }

    async fn execute_with_interceptors(
        context: &mut Context,
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        interceptors: &[Arc<dyn Interceptor>],
        pipes: &[Arc<dyn Pipe>],
        error_handlers: &[Arc<dyn ErrorHandler>],
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
            if let Some(response) = context
                .switch_to_rpc()
                .and_then(|rpc| rpc.response().cloned())
            {
                return response.clone();
            }
            return Err(RpcError::Internal(
                "Request aborted by interceptor without response".into(),
            ));
        }

        if let Some(response) = context
            .switch_to_rpc()
            .and_then(|rpc| rpc.response().cloned())
        {
            response.clone()
        } else {
            Err(RpcError::Internal("Handler did not set response".into()))
        }
    }

    /// Stores the result in context rather than returning it directly.
    async fn execute_with_interceptors_impl(
        context: &mut Context,
        interceptors: &[Arc<dyn Interceptor>],
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        pipes: &[Arc<dyn Pipe>],
        error_handlers: &[Arc<dyn ErrorHandler>],
    ) {
        if interceptors.is_empty() {
            Self::execute_handler(context, controller, pipes).await;
            if !error_handlers.is_empty() {
                if let Some(Err(e)) = context
                    .switch_to_rpc()
                    .and_then(|rpc| rpc.response().cloned())
                {
                    let error_msg = e.to_string();
                    for handler in error_handlers.iter().rev() {
                        let error: Box<dyn std::error::Error + Send> = Box::new(
                            std::io::Error::new(std::io::ErrorKind::Other, error_msg.clone()),
                        );
                        if let Some(crate::traits_helpers::ErrorResponse::Rpc(data)) =
                            handler.handle_error(error, context).await
                        {
                            context
                                .switch_to_rpc_mut()
                                .expect("Expected RPC context")
                                .set_response(Ok(Some(data)));
                            return;
                        }
                    }
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
        context: &mut Context,
        controller: &Arc<Box<dyn RpcControllerTrait>>,
        pipes: &[Arc<dyn Pipe>],
    ) {
        for pipe in pipes {
            pipe.process(context);
            if context.should_abort() {
                context
                    .switch_to_rpc_mut()
                    .expect("Expected RPC context")
                    .set_response(Err(RpcError::Internal("Request aborted by pipe".into())));
                return;
            }
        }

        let Some(rpc) = context.switch_to_rpc() else {
            context
                .switch_to_rpc_mut()
                .expect("Expected RPC context")
                .set_response(Err(RpcError::Internal("Expected RPC context".into())));
            return;
        };
        let (data, call_context) = (rpc.data().clone(), rpc.call_context().clone());

        let result = controller.handle_message(data, call_context).await;

        context
            .switch_to_rpc_mut()
            .expect("Expected RPC context")
            .set_response(result);
    }
}
