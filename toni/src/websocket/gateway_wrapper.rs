use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use parking_lot::RwLock;

use crate::Error;
use crate::context::{HandlerContext, WsContext};
use crate::errors::{PanicRecovered, PipelineSegment};
use crate::http_helpers::{ExecutionResult, RequestPart, RouteMetadata};
use crate::traits_helpers::{
    ErrorObserver, Guard, Interceptor, InterceptorNext, Pipe, WsErrorHandlerArc, WsGuardEntry,
    WsInterceptorEntry, WsPipeEntry,
};

use super::{DisconnectReason, GatewayTrait, WsClient, WsError, WsHandlerOutput, WsMessage};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

struct WsChainNext {
    interceptors: Vec<Arc<dyn Interceptor<WsContext>>>,
    gateway: Arc<Box<dyn GatewayTrait>>,
    event: String,
    pipes: Vec<Arc<dyn Pipe<WsContext>>>,
    error_handlers: Vec<WsErrorHandlerArc>,
    observers: Vec<Arc<dyn ErrorObserver>>,
    stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
}

#[async_trait]
impl InterceptorNext<WsContext> for WsChainNext {
    async fn run(self: Box<Self>, context: &mut WsContext) {
        GatewayWrapper::execute_with_interceptors_impl(
            context,
            &self.interceptors,
            &self.gateway,
            &self.event,
            &self.pipes,
            &self.error_handlers,
            &self.observers,
            self.stream_slot,
        )
        .await;
    }
}

/// Parallel to `InstanceWrapper` on the HTTP side — wraps a gateway with the full
/// guard/interceptor/pipe pipeline and tracks its own connected clients.
pub struct GatewayWrapper {
    gateway: Arc<Box<dyn GatewayTrait>>,
    guards: Vec<WsGuardEntry>,
    interceptors: Vec<WsInterceptorEntry>,
    pipes: Vec<WsPipeEntry>,
    error_handlers: Vec<WsErrorHandlerArc>,
    error_observers: Vec<Arc<dyn ErrorObserver>>,
    route_metadata: Arc<RouteMetadata>,
    handler_guards: HashMap<String, Vec<WsGuardEntry>>,
    handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>>,
    handler_pipes: HashMap<String, Vec<WsPipeEntry>>,
    handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>>,
    /// Active client connections (client_id => WsClient)
    clients: Arc<RwLock<HashMap<String, WsClient>>>,
}

impl GatewayWrapper {
    pub fn new(
        gateway: Arc<Box<dyn GatewayTrait>>,
        guards: Vec<WsGuardEntry>,
        interceptors: Vec<WsInterceptorEntry>,
        pipes: Vec<WsPipeEntry>,
        error_handlers: Vec<WsErrorHandlerArc>,
        error_observers: Vec<Arc<dyn ErrorObserver>>,
        route_metadata: Arc<RouteMetadata>,
        handler_guards: HashMap<String, Vec<WsGuardEntry>>,
        handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>>,
        handler_pipes: HashMap<String, Vec<WsPipeEntry>>,
        handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>>,
    ) -> Self {
        Self {
            gateway,
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
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Phase 1 of connection setup: run guards and store client.
    pub async fn begin_connect(
        &self,
        client: WsClient,
        parts: &RequestPart,
    ) -> Result<(), WsError> {
        let context = WsContext::new(
            client.clone(),
            WsMessage::text(""),
            "connect",
            Some(self.route_metadata.clone()),
        );

        let guards = Self::resolve_guards(&self.guards, Some(parts)).await;
        for (i, guard) in guards.iter().enumerate() {
            // A panic in `can_activate` is treated as a hard rejection so the
            // dispatcher doesn't tear down. Observers see
            // `PanicRecovered { during: Guard }`; the connection is refused.
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    tracing::debug!(client_id = %client.id, guard_index = i, "guard panicked during connect");
                    Self::fan_out_observers(&self.error_observers, &event, &context).await;
                    return Err(WsError::AuthFailed(format!(
                        "guard {} panicked: {}",
                        i, event.message
                    )));
                }
            };
            if !activated {
                tracing::debug!(client_id = %client.id, guard_index = i, "guard rejected WebSocket connection");
                let err = WsError::AuthFailed("Guard rejected connection".into());
                Self::fan_out_observers(&self.error_observers, &err, &context).await;
                return Err(err);
            }
            if context.should_abort() {
                let err = WsError::AuthFailed("Connection aborted by guard".into());
                Self::fan_out_observers(&self.error_observers, &err, &context).await;
                return Err(err);
            }
        }

        tracing::debug!(client_id = %client.id, "WebSocket client connected");
        self.clients.write().insert(client.id.clone(), client);
        Ok(())
    }

    /// Phase 2 of connection setup: fire the `on_connect` lifecycle hook.
    pub async fn complete_connect(&self, client_id: &str) -> Result<(), WsError> {
        let client = self
            .clients
            .read()
            .get(client_id)
            .cloned()
            .ok_or_else(|| WsError::ConnectionClosed("Client not found".into()))?;

        let context = WsContext::new(
            client.clone(),
            WsMessage::text(""),
            "connect",
            Some(self.route_metadata.clone()),
        );

        self.gateway.on_connect(&client, &context).await
    }

    /// Handle new WebSocket connection (simple path — no ConnectionManager).
    pub async fn handle_connect(
        &self,
        client: WsClient,
        parts: &RequestPart,
    ) -> Result<(), WsError> {
        let client_id = client.id.clone();
        self.begin_connect(client, parts).await?;
        self.complete_connect(&client_id).await
    }

    pub async fn handle_message(
        &self,
        client_id: String,
        message: WsMessage,
    ) -> Result<WsHandlerOutput, WsError> {
        let client = self
            .clients
            .read()
            .get(&client_id)
            .cloned()
            .ok_or_else(|| WsError::ConnectionClosed("Client not found".into()))?;

        let event = self.extract_event(&message)?;

        tracing::trace!(client_id = %client_id, event = %event, "WebSocket message received");

        let mut context = WsContext::new(
            client.clone(),
            message.clone(),
            event.clone(),
            Some(self.route_metadata.clone()),
        );

        let mut all_guards = self.guards.clone();
        if let Some(h) = self.handler_guards.get(&event) {
            all_guards.extend_from_slice(h);
        }
        let mut all_interceptors = self.interceptors.clone();
        if let Some(h) = self.handler_interceptors.get(&event) {
            all_interceptors.extend_from_slice(h);
        }
        let mut all_pipes = self.pipes.clone();
        if let Some(h) = self.handler_pipes.get(&event) {
            all_pipes.extend_from_slice(h);
        }
        let mut all_error_handlers = self.error_handlers.clone();
        if let Some(h) = self.handler_error_handlers.get(&event) {
            all_error_handlers.extend_from_slice(h);
        }

        let guards = Self::resolve_guards(&all_guards, None).await;
        for (index, guard) in guards.iter().enumerate() {
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    Self::fan_out_observers(&self.error_observers, &event, &context).await;
                    return Err(WsError::AuthFailed(format!(
                        "guard {} panicked: {}",
                        index, event.message
                    )));
                }
            };
            if !activated {
                let err = WsError::AuthFailed("Guard rejected message".into());
                Self::fan_out_observers(&self.error_observers, &err, &context).await;
                return Err(err);
            }
            if context.should_abort() {
                let err = WsError::AuthFailed("Message aborted by guard".into());
                Self::fan_out_observers(&self.error_observers, &err, &context).await;
                return Err(err);
            }
        }

        let interceptors = Self::resolve_interceptors(&all_interceptors, None).await;
        let pipes = Self::resolve_pipes(&all_pipes, None).await;

        // Streams bypass the context (which stores only Option<WsMessage>).
        // The handler deposits a stream here; handle_message lifts it out.
        let stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>> =
            Arc::new(parking_lot::Mutex::new(None));

        Self::execute_with_interceptors(
            &mut context,
            event,
            &self.gateway,
            &interceptors,
            &pipes,
            &all_error_handlers,
            &self.error_observers,
            stream_slot.clone(),
        )
        .await?;

        if let Some(stream) = stream_slot.lock().take() {
            return Ok(WsHandlerOutput::Stream(stream));
        }

        match context.take_response() {
            Some(Ok(Some(msg))) => Ok(WsHandlerOutput::Single(msg)),
            Some(Ok(None)) => Ok(WsHandlerOutput::Empty),
            Some(Err(e)) => Err(e),
            None => Err(WsError::Internal("Handler did not set response".into())),
        }
    }

    async fn resolve_guards(
        entries: &[WsGuardEntry],
        parts: Option<&RequestPart>,
    ) -> Vec<Arc<dyn Guard<WsContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                WsGuardEntry::Ready(g) => g.clone(),
                WsGuardEntry::Factory(f) => f.create(parts).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(
        entries: &[WsInterceptorEntry],
        parts: Option<&RequestPart>,
    ) -> Vec<Arc<dyn Interceptor<WsContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                WsInterceptorEntry::Ready(i) => i.clone(),
                WsInterceptorEntry::Factory(f) => f.create(parts).await,
            };
            out.push(i);
        }
        out
    }

    async fn resolve_pipes(
        entries: &[WsPipeEntry],
        parts: Option<&RequestPart>,
    ) -> Vec<Arc<dyn Pipe<WsContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let p = match entry {
                WsPipeEntry::Ready(p) => p.clone(),
                WsPipeEntry::Factory(f) => f.create(parts).await,
            };
            out.push(p);
        }
        out
    }

    async fn execute_with_interceptors(
        context: &mut WsContext,
        event: String,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        interceptors: &[Arc<dyn Interceptor<WsContext>>],
        pipes: &[Arc<dyn Pipe<WsContext>>],
        error_handlers: &[WsErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
    ) -> Result<(), WsError> {
        Self::execute_with_interceptors_impl(
            context,
            interceptors,
            gateway,
            &event,
            pipes,
            error_handlers,
            observers,
            stream_slot,
        )
        .await;

        if context.should_abort() {
            return context
                .take_response()
                .unwrap_or_else(|| {
                    Err(WsError::Internal(
                        "Request aborted by interceptor without response".into(),
                    ))
                })
                .map(|_| ());
        }

        Ok(())
    }

    async fn execute_with_interceptors_impl(
        context: &mut WsContext,
        interceptors: &[Arc<dyn Interceptor<WsContext>>],
        gateway: &Arc<Box<dyn GatewayTrait>>,
        event: &str,
        pipes: &[Arc<dyn Pipe<WsContext>>],
        error_handlers: &[WsErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
    ) {
        if interceptors.is_empty() {
            Self::execute_handler_with_error_handling(
                context,
                gateway,
                event,
                pipes,
                error_handlers,
                observers,
                stream_slot,
            )
            .await;
            return;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = WsChainNext {
            interceptors: rest.to_vec(),
            gateway: gateway.clone(),
            event: event.to_string(),
            pipes: pipes.to_vec(),
            error_handlers: error_handlers.to_vec(),
            observers: observers.to_vec(),
            stream_slot,
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

    /// Surface a panicking pre-handler segment (interceptor; pipes flow
    /// through `execute_handler`'s `ExecutionResult::Err` instead) through
    /// the existing observer + chain pipeline so it cannot tear down the
    /// connection. Fan to observers, give error handlers first claim,
    /// and fall back to a wire-`Err` frame.
    async fn record_pipeline_panic(
        context: &mut WsContext,
        error_handlers: &[WsErrorHandlerArc],
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
        let ws_err = WsError::from(event);
        let msg = Self::safe_render(|| ws_err.to_message(), observers, context).await;
        context.set_response(Ok(Some(msg)));
    }

    /// Run pipes + handler, then route the outcome.
    ///
    /// `Ok` goes straight to the context. On `Err`, observers fan out on the
    /// underlying error, the chain's most-specific handler gets first claim,
    /// and `WsError::to_message` is the fallback frame when none claims.
    async fn execute_handler_with_error_handling(
        context: &mut WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        event: &str,
        pipes: &[Arc<dyn Pipe<WsContext>>],
        error_handlers: &[WsErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
    ) {
        let result = Self::execute_handler(context, gateway, event, pipes).await;

        match result {
            ExecutionResult::Ok(WsHandlerOutput::Stream(stream)) => {
                // Streams bypass context storage — deposit and return early.
                *stream_slot.lock() = Some(stream);
                context.set_response(Ok(None));
            }
            ExecutionResult::Ok(WsHandlerOutput::Single(msg)) => {
                context.set_response(Ok(Some(msg)));
            }
            ExecutionResult::Ok(WsHandlerOutput::Empty) => {
                context.set_response(Ok(None));
            }
            ExecutionResult::Err(ws_err) => {
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) =
                    match &ws_err {
                        WsError::AppError(e) => e.as_ref(),
                        other => other,
                    };
                Self::fan_out_observers(observers, observed_err, context).await;
                for handler in error_handlers.iter().rev() {
                    if let Some(msg) =
                        Self::try_chain_handler(handler, observed_err, context, observers).await
                    {
                        context.set_response(Ok(Some(msg)));
                        return;
                    }
                }
                let msg = Self::safe_render(|| ws_err.to_message(), observers, context).await;
                context.set_response(Ok(Some(msg)));
            }
        }
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` fans `PanicRecovered { during: ErrorHandler }` to
    /// observers and returns `None` so the caller continues to the next
    /// handler. Without this, a single bad chain handler would kill the
    /// whole error-recovery path and the original error would never
    /// reach the fallback `to_message` rendering.
    /// Drive `WsError::to_message` with panic recovery — a panic in the
    /// renderer would close the connection without ever framing an
    /// outbound error message. Policy: fan
    /// `PanicRecovered { during: ResponseRendering }` to observers,
    /// substitute a hardcoded text frame.
    async fn safe_render<F>(
        render: F,
        observers: &[Arc<dyn ErrorObserver>],
        ctx: &WsContext,
    ) -> WsMessage
    where
        F: FnOnce() -> WsMessage,
    {
        match crate::panic_recovery::catch_sync(
            crate::errors::PipelineSegment::ResponseRendering,
            render,
        ) {
            Ok(msg) => msg,
            Err(panic_event) => {
                Self::fan_out_observers(observers, &panic_event, ctx).await;
                Self::fallback_internal_message()
            }
        }
    }

    /// Hardcoded fallback frame when the regular renderer panics.
    fn fallback_internal_message() -> WsMessage {
        WsMessage::text("Internal Server Error")
    }

    async fn try_chain_handler(
        handler: &WsErrorHandlerArc,
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &WsContext,
        observers: &[Arc<dyn ErrorObserver>],
    ) -> Option<WsMessage> {
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
        ctx: &WsContext,
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

    async fn execute_handler(
        context: &mut WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        event: &str,
        pipes: &[Arc<dyn Pipe<WsContext>>],
    ) -> ExecutionResult<WsHandlerOutput, WsError> {
        for pipe in pipes {
            // `pipe.process` is sync — `catch_sync` wraps it the same way
            // `catch_async` wraps async segments. A panic returns as
            // `ExecutionResult::Err(WsError::from(panic_event))`; the
            // caller's chain fans observers, gives error handlers first
            // claim, and falls back to the default frame.
            if let Err(event) = crate::panic_recovery::catch_sync(
                PipelineSegment::Pipe,
                || pipe.process(context),
            ) {
                return ExecutionResult::Err(WsError::from(event));
            }
            if context.should_abort() {
                return ExecutionResult::Err(WsError::Internal(
                    "Request aborted by pipe".into(),
                ));
            }
        }

        let client = context.client().clone();
        let message = context.message().clone();
        let result = AssertUnwindSafe(gateway.handle_event(client, message, event))
            .catch_unwind()
            .await;
        match result {
            Ok(exec) => exec,
            Err(payload) => ExecutionResult::Err(WsError::from(
                PanicRecovered::from_panic_payload(PipelineSegment::HandlerBody, payload),
            )),
        }
    }

    pub async fn handle_disconnect(&self, client_id: String, reason: DisconnectReason) {
        let maybe_client = self.clients.write().remove(&client_id);
        if let Some(client) = maybe_client {
            tracing::debug!(client_id = %client_id, "WebSocket client disconnected");
            self.gateway.on_disconnect(&client, reason).await;
        }
    }

    /// Parses the event name from a message using the gateway's `event_field()` key.
    fn extract_event(&self, message: &WsMessage) -> Result<String, WsError> {
        match message {
            WsMessage::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
                    let field = self.gateway.event_field();
                    if let Some(event) = parsed.get(field).and_then(|v| v.as_str()) {
                        return Ok(event.to_string());
                    }
                }

                Err(WsError::InvalidMessage(format!(
                    "Missing '{}' field in JSON message",
                    self.gateway.event_field()
                )))
            }
            WsMessage::Binary(_) => Err(WsError::InvalidMessage(
                "Binary messages not yet supported for event extraction".into(),
            )),
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Close => Err(
                WsError::InvalidMessage("Control frames don't have events".into()),
            ),
        }
    }

    pub async fn get_clients(&self) -> Vec<WsClient> {
        self.clients.read().values().cloned().collect()
    }

    pub async fn get_client(&self, client_id: &str) -> Option<WsClient> {
        self.clients.read().get(client_id).cloned()
    }

    pub async fn call_after_init(&self) {
        self.gateway.after_init().await;
    }

    pub fn get_path(&self) -> String {
        self.gateway.get_path()
    }

    pub fn get_namespace(&self) -> Option<String> {
        self.gateway.get_namespace()
    }

    pub fn get_port(&self) -> Option<u16> {
        self.gateway.get_port()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_event_from_json() {
        let wrapper = create_test_wrapper();

        let msg = WsMessage::text(r#"{"event": "message", "data": "hello"}"#);
        let event = wrapper.extract_event(&msg).unwrap();
        assert_eq!(event, "message");
    }

    #[test]
    fn test_extract_event_missing_field() {
        let wrapper = create_test_wrapper();

        let msg = WsMessage::text(r#"{"data": "hello"}"#);
        let result = wrapper.extract_event(&msg);
        assert!(result.is_err());
    }

    fn create_test_wrapper() -> GatewayWrapper {
        struct TestGateway;

        #[async_trait::async_trait]
        impl GatewayTrait for TestGateway {
            fn get_token(&self) -> String {
                "TestGateway".to_string()
            }

            fn get_path(&self) -> String {
                "/test".to_string()
            }

            async fn handle_event(
                &self,
                _client: WsClient,
                _message: WsMessage,
                _event: &str,
            ) -> ExecutionResult<WsHandlerOutput, WsError> {
                ExecutionResult::Ok(WsHandlerOutput::Empty)
            }
        }

        GatewayWrapper::new(
            Arc::new(Box::new(TestGateway)),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            Arc::new(RouteMetadata::new()),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        )
    }
}
