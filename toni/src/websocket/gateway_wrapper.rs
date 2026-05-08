use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use parking_lot::RwLock;

use crate::context::{HandlerContext, WsContext};
use crate::http_helpers::{RequestPart, RouteMetadata};
use crate::traits_helpers::{
    Guard, Interceptor, InterceptorNext, Pipe, WsErrorHandlerArc, WsGuardEntry,
    WsInterceptorEntry, WsPipeEntry,
};

use super::{DisconnectReason, GatewayTrait, WsClient, WsError, WsHandlerOutput, WsMessage};

struct WsChainNext {
    interceptors: Vec<Arc<dyn Interceptor<WsContext>>>,
    gateway: Arc<Box<dyn GatewayTrait>>,
    event: String,
    pipes: Vec<Arc<dyn Pipe<WsContext>>>,
    error_handlers: Vec<WsErrorHandlerArc>,
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
            if !guard.can_activate(&context).await {
                tracing::debug!(client_id = %client.id, guard_index = i, "guard rejected WebSocket connection");
                return Err(WsError::AuthFailed("Guard rejected connection".into()));
            }
            if context.should_abort() {
                return Err(WsError::AuthFailed("Connection aborted by guard".into()));
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
        for guard in &guards {
            if !guard.can_activate(&context).await {
                return Err(WsError::AuthFailed("Guard rejected message".into()));
            }
            if context.should_abort() {
                return Err(WsError::AuthFailed("Message aborted by guard".into()));
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
        stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
    ) -> Result<(), WsError> {
        Self::execute_with_interceptors_impl(
            context,
            interceptors,
            gateway,
            &event,
            pipes,
            error_handlers,
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
        stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
    ) {
        if interceptors.is_empty() {
            Self::execute_handler_with_error_handling(
                context,
                gateway,
                event,
                pipes,
                error_handlers,
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
            stream_slot,
        };

        first.intercept(context, Box::new(next)).await;
    }

    async fn execute_handler_with_error_handling(
        context: &mut WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        event: &str,
        pipes: &[Arc<dyn Pipe<WsContext>>],
        error_handlers: &[WsErrorHandlerArc],
        stream_slot: Arc<parking_lot::Mutex<Option<BoxStream<'static, WsMessage>>>>,
    ) {
        let result = Self::execute_handler(context, gateway, event, pipes).await;

        // Streams bypass context storage — deposit and return early.
        if let Ok(WsHandlerOutput::Stream(stream)) = result {
            *stream_slot.lock() = Some(stream);
            context.set_response(Ok(None));
            return;
        }

        let context_result = if !error_handlers.is_empty() {
            if let Err(ref e) = result {
                let error_msg = e.to_string();
                let error = std::io::Error::new(std::io::ErrorKind::Other, error_msg);
                let mut recovered = None;
                for handler in error_handlers.iter().rev() {
                    if let Some(msg) = handler.handle_error(&error, context).await {
                        recovered = Some(Ok(Some(msg)));
                        break;
                    }
                }
                recovered.unwrap_or_else(|| {
                    result.map(|o| match o {
                        WsHandlerOutput::Single(m) => Some(m),
                        _ => None,
                    })
                })
            } else {
                result.map(|o| match o {
                    WsHandlerOutput::Single(m) => Some(m),
                    _ => None,
                })
            }
        } else {
            result.map(|o| match o {
                WsHandlerOutput::Single(m) => Some(m),
                _ => None,
            })
        };

        context.set_response(context_result);
    }

    async fn execute_handler(
        context: &mut WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        event: &str,
        pipes: &[Arc<dyn Pipe<WsContext>>],
    ) -> Result<WsHandlerOutput, WsError> {
        for pipe in pipes {
            pipe.process(context);
            if context.should_abort() {
                return Err(WsError::Internal("Request aborted by pipe".into()));
            }
        }

        let client = context.client().clone();
        let message = context.message().clone();
        gateway.handle_event(client, message, event).await
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
            ) -> Result<WsHandlerOutput, WsError> {
                Ok(WsHandlerOutput::Empty)
            }
        }

        GatewayWrapper::new(
            Arc::new(Box::new(TestGateway)),
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
