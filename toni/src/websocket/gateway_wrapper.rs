use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::context::Metadata;
use crate::context::WsContext;
use crate::errors::{PanicRecovered, PipelineSegment};
use crate::http_helpers::ExecutionResult;
use crate::traits_helpers::{
    Guard, Interceptor, InterceptorNext, WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry,
};

use super::{
    DisconnectReason, GatewayTrait, WsClient, WsError, WsHandlerOutput, WsHandlerResult, WsMessage,
};
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};
use std::panic::AssertUnwindSafe;

/// Delegates to an inner stream while holding something alive alongside it.
///
/// `BoxStream` is a `Pin<Box<_>>` and therefore `Unpin`, so the projection needs
/// no pin machinery. The HTTP side does the same for response bodies.
struct ScopedStream {
    inner: BoxStream<'static, WsMessage>,
    context: WsContext,
    /// Set once the inner stream answers `None`, which is the end of it.
    drained: bool,
}

impl futures::Stream for ScopedStream {
    type Item = WsMessage;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_next(cx);
        if matches!(polled, std::task::Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// A stream dropped with messages still to come is the connection having gone. The handler returned
/// when it had a stream, so whatever feeds that stream is not inside a future anything drops.
impl Drop for ScopedStream {
    fn drop(&mut self) {
        if !self.drained {
            use crate::context::HandlerContext as _;
            self.context.cancellation().cancel();
        }
    }
}

struct WsChainNext {
    interceptors: Vec<Arc<dyn Interceptor<WsContext, WsHandlerResult>>>,
    gateway: Arc<Box<dyn GatewayTrait>>,
    error_handlers: Vec<WsErrorHandlerArc>,
}

#[async_trait]
impl InterceptorNext<WsContext, WsHandlerResult> for WsChainNext {
    async fn run(self: Box<Self>, context: &WsContext) -> WsHandlerResult {
        GatewayWrapper::execute_with_interceptors(
            context,
            &self.interceptors,
            &self.gateway,
            &self.error_handlers,
        )
        .await
    }
}

/// Parallel to `InstanceWrapper` on the HTTP side — wraps a gateway with the full
/// guard/interceptor pipeline and tracks its own connected clients.
pub struct GatewayWrapper {
    gateway: Arc<Box<dyn GatewayTrait>>,
    guards: Vec<WsGuardEntry>,
    interceptors: Vec<WsInterceptorEntry>,
    error_handlers: Vec<WsErrorHandlerArc>,
    metadata: Arc<Metadata>,
    /// Per-event metadata, already merged over `metadata` at expansion. An event absent here
    /// declared nothing of its own and reads the gateway's.
    handler_metadata: HashMap<String, Arc<Metadata>>,
    handler_guards: HashMap<String, Vec<WsGuardEntry>>,
    handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>>,
    handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>>,
    /// Active client connections (client_id => WsClient). A client carries the session scoped to
    /// its connection, so there is nothing to keep beside it.
    clients: Arc<RwLock<HashMap<String, WsClient>>>,
}

impl GatewayWrapper {
    pub fn new(
        gateway: Arc<Box<dyn GatewayTrait>>,
        guards: Vec<WsGuardEntry>,
        interceptors: Vec<WsInterceptorEntry>,
        error_handlers: Vec<WsErrorHandlerArc>,
        metadata: Arc<Metadata>,
        handler_metadata: HashMap<String, Arc<Metadata>>,
        handler_guards: HashMap<String, Vec<WsGuardEntry>>,
        handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>>,
        handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>>,
    ) -> Self {
        Self {
            gateway,
            guards,
            interceptors,
            error_handlers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Phase 1 of connection setup: run guards and store client.
    ///
    /// The upgrade's parts are no longer threaded in: a connect guard reads the
    /// handshake through `ctx.client().handshake`, which is the same information
    /// by a route that works per-message too.
    /// Phase 1 of connection setup: run the guards that admit the connection.
    ///
    /// Returns the connect execution's context, which phase 2 finishes. The two phases exist so the
    /// adapter can register the client's sink between them, not because they are separate
    /// executions — a guard's writes have to reach the hook, so they share one context and one bag.
    ///
    /// Guards run here and interceptors do not, because a connect is an admission decision rather
    /// than a call. A guard answers admission, which is its whole contract. An interceptor wraps a
    /// call and its answer, and an error handler shapes an answer; a connection has none to wrap or
    /// shape, and refusing one is answered by not opening it. The same rule decides the message
    /// path: a refused message has an open socket to answer on, so it goes through the chain and
    /// the caller is told.
    pub async fn begin_connect(&self, client: WsClient) -> Result<WsContext, WsError> {
        // The client was born with its session, so a guard below writes to the store every later
        // execution on this connection reads.
        let context = WsContext::new(
            client.clone(),
            WsMessage::text(""),
            "connect",
            Some(self.metadata.clone()),
        );

        let guards = Self::resolve_guards(&self.guards, &context).await;
        for (i, guard) in guards.iter().enumerate() {
            // A panic in `can_activate` is treated as a hard rejection so the
            // dispatcher doesn't tear down: the panic is logged and the
            // connection is refused. A connect has no chain to route it
            // through — there is no answer to shape on a refused upgrade.
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    tracing::error!(client_id = %client.id, guard_index = i, panic = %event.message, "connect guard panicked; refusing the connection");
                    return Err(WsError::AuthFailed(format!(
                        "guard {} panicked: {}",
                        i, event.message
                    )));
                }
            };
            if !activated {
                tracing::debug!(client_id = %client.id, guard_index = i, "guard rejected WebSocket connection");
                return Err(WsError::AuthFailed("Guard rejected connection".into()));
            }
        }

        tracing::debug!(client_id = %client.id, "WebSocket client connected");
        self.clients.write().insert(client.id.clone(), client);
        Ok(context)
    }

    /// Phase 2 of connection setup: fire the `on_connect` lifecycle hook on the context phase 1
    /// built, so the hook reads the bag the guards wrote to.
    pub async fn complete_connect(&self, context: &WsContext) -> Result<(), WsError> {
        let client = context.client();
        if !self.clients.read().contains_key(&client.id) {
            return Err(WsError::ConnectionClosed("Client not found".into()));
        }

        self.gateway.on_connect(client, context).await
    }

    /// Handle new WebSocket connection (simple path — no ConnectionManager).
    pub async fn handle_connect(&self, client: WsClient) -> Result<(), WsError> {
        let context = self.begin_connect(client).await?;
        self.complete_connect(&context).await
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

        let context = WsContext::new(
            client.clone(),
            message.clone(),
            event.clone(),
            Some(
                self.handler_metadata
                    .get(&event)
                    .unwrap_or(&self.metadata)
                    .clone(),
            ),
        );

        let mut all_guards = self.guards.clone();
        if let Some(h) = self.handler_guards.get(&event) {
            all_guards.extend_from_slice(h);
        }
        let mut all_interceptors = self.interceptors.clone();
        if let Some(h) = self.handler_interceptors.get(&event) {
            all_interceptors.extend_from_slice(h);
        }
        let mut all_error_handlers = self.error_handlers.clone();
        if let Some(h) = self.handler_error_handlers.get(&event) {
            all_error_handlers.extend_from_slice(h);
        }

        let guards = Self::resolve_guards(&all_guards, &context).await;
        for (guard_index, guard) in guards.iter().enumerate() {
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    // Guard panic is a developer error, not a rejection
                    // verdict: route through the same path as other
                    // pipeline panics so the chain runs once and the
                    // canonical envelope reaches the client. Without this,
                    // `WsError::AuthFailed` would drop unsent in
                    // `ToniApplication`'s message callback.
                    tracing::debug!(guard_index = guard_index, panic = %event.message, "guard panicked");
                    return Self::record_pipeline_panic(&context, &all_error_handlers, event).await;
                }
            };
            if !activated {
                return Self::record_guard_rejection(
                    &context,
                    &all_error_handlers,
                    crate::errors::GuardRejection::new(guard_index),
                )
                .await;
            }
        }

        let interceptors = Self::resolve_interceptors(&all_interceptors, &context).await;

        let answer = Self::execute_with_interceptors(
            &context,
            &interceptors,
            &self.gateway,
            &all_error_handlers,
        )
        .await;

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(WsHandlerOutput::Stream(stream)) => Ok(WsHandlerOutput::Stream(
                ScopedStream {
                    inner: stream,
                    context,
                    drained: false,
                }
                .boxed(),
            )),
            other => other,
        }
    }

    async fn resolve_guards(
        entries: &[WsGuardEntry],
        ctx: &WsContext,
    ) -> Vec<Arc<dyn Guard<WsContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                WsGuardEntry::Ready(g) => g.clone(),
                WsGuardEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(
        entries: &[WsInterceptorEntry],
        ctx: &WsContext,
    ) -> Vec<Arc<dyn Interceptor<WsContext, WsHandlerResult>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                WsInterceptorEntry::Ready(i) => i.clone(),
                WsInterceptorEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(i);
        }
        out
    }

    async fn execute_with_interceptors(
        context: &WsContext,
        interceptors: &[Arc<dyn Interceptor<WsContext, WsHandlerResult>>],
        gateway: &Arc<Box<dyn GatewayTrait>>,
        error_handlers: &[WsErrorHandlerArc],
    ) -> WsHandlerResult {
        if interceptors.is_empty() {
            return Self::execute_handler_with_error_handling(context, gateway, error_handlers)
                .await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = WsChainNext {
            interceptors: rest.to_vec(),
            gateway: gateway.clone(),
            error_handlers: error_handlers.to_vec(),
        };

        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(answer) => answer,
            Err(event) => Self::record_pipeline_panic(context, error_handlers, event).await,
        }
    }

    /// Surface a panicking pre-handler segment (an interceptor; it flows
    /// through `execute_handler`'s `ExecutionResult::Err` instead) through
    /// the chain so it cannot tear down the connection. Error handlers get
    /// first claim, and the fallback is a wire-`Err` frame.
    async fn record_pipeline_panic(
        context: &WsContext,
        error_handlers: &[WsErrorHandlerArc],
        event: PanicRecovered,
    ) -> WsHandlerResult {
        for (position, handler) in error_handlers.iter().rev().enumerate() {
            if let Some(claimed) = Self::try_chain_handler(handler, &event, context, position).await
            {
                return Ok(WsHandlerOutput::Single(claimed));
            }
        }
        let ws_err = WsError::from(event);
        Ok(WsHandlerOutput::Single(Self::safe_render(|| {
            ws_err.to_message()
        })))
    }

    /// Route a guard's refusal through the chain, as HTTP does.
    ///
    /// A refused message has an open socket to answer on, so the caller is told
    /// which is what every other transport does, and a `#[catch(GuardRejection)]`
    /// handler gets first claim on the shape. Returning the rendered message
    /// rather than `Err` is what keeps the connection usable: the read loop
    /// carries on, and the client learns its message went nowhere.
    async fn record_guard_rejection(
        context: &WsContext,
        error_handlers: &[WsErrorHandlerArc],
        rejection: crate::errors::GuardRejection,
    ) -> WsHandlerResult {
        for (position, handler) in error_handlers.iter().rev().enumerate() {
            if let Some(claimed) =
                Self::try_chain_handler(handler, &rejection, context, position).await
            {
                return Ok(WsHandlerOutput::Single(claimed));
            }
        }
        Ok(WsHandlerOutput::Single(Self::safe_render(|| {
            super::ws_error::render_error(&rejection)
        })))
    }

    /// Run the handler, then route the outcome.
    ///
    /// `Ok` is the answer, streams included. On `Err`, the chain's
    /// most-specific handler gets first claim on the underlying error, and
    /// `WsError::to_message` is the fallback frame when none claims.
    async fn execute_handler_with_error_handling(
        context: &WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        error_handlers: &[WsErrorHandlerArc],
    ) -> WsHandlerResult {
        match Self::execute_handler(context, gateway).await {
            ExecutionResult::Ok(output) => Ok(output),
            ExecutionResult::Err(ws_err) => {
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) = match &ws_err {
                    WsError::AppError(e) => e.as_ref(),
                    other => other,
                };
                for (position, handler) in error_handlers.iter().rev().enumerate() {
                    if let Some(msg) =
                        Self::try_chain_handler(handler, observed_err, context, position).await
                    {
                        return Ok(WsHandlerOutput::Single(msg));
                    }
                }
                Ok(WsHandlerOutput::Single(Self::safe_render(|| {
                    ws_err.to_message()
                })))
            }
        }
    }

    /// Drive `WsError::to_message` with panic recovery — a panic in the
    /// renderer would close the connection without ever framing an outbound
    /// error message. Policy: log the panic and substitute a hardcoded text
    /// frame.
    fn safe_render<F>(render: F) -> WsMessage
    where
        F: FnOnce() -> WsMessage,
    {
        match crate::panic_recovery::catch_sync(
            crate::errors::PipelineSegment::ResponseRendering,
            render,
        ) {
            Ok(msg) => msg,
            Err(panic_event) => {
                tracing::error!(panic = %panic_event.message, "error renderer panicked; falling back to a bare text frame");
                Self::fallback_internal_message()
            }
        }
    }

    /// Hardcoded fallback frame when the regular renderer panics.
    ///
    /// A string literal, so none of the user code the renderer was calling
    /// runs again. It keeps the canonical envelope: a frame carries no content
    /// type, so a bare string would reach the client's decoder as text where
    /// every other error frame is an object.
    fn fallback_internal_message() -> WsMessage {
        WsMessage::text(r#"{"status":"error","kind":"Internal","message":"Internal Server Error"}"#)
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` is logged and answers `None`, so the caller continues
    /// to the next handler. Without this, a single bad chain handler would
    /// kill the whole error-recovery path and the original error would
    /// never reach the fallback `to_message` rendering.
    ///
    /// `position` counts from the most specific handler — the chain runs
    /// event, then gateway, then global — and is logged so a panic names which
    /// registration it came from.
    async fn try_chain_handler(
        handler: &WsErrorHandlerArc,
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &WsContext,
        position: usize,
    ) -> Option<WsMessage> {
        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::ErrorHandler,
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

    async fn execute_handler(
        context: &WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
    ) -> ExecutionResult<WsHandlerOutput, WsError> {
        let result = AssertUnwindSafe(gateway.handle_event(context))
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
        let maybe = self.clients.write().remove(&client_id);
        if let Some(client) = maybe {
            tracing::debug!(client_id = %client_id, "WebSocket client disconnected");
            // An execution of its own, so teardown reads the session the way every other
            // participant does. No enhancers run: a disconnect cannot be rejected.
            let context = WsContext::new(
                client.clone(),
                WsMessage::text(""),
                "disconnect",
                Some(self.metadata.clone()),
            );
            self.gateway
                .on_disconnect(context.client(), reason, &context)
                .await;
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
                _ctx: &WsContext,
            ) -> ExecutionResult<WsHandlerOutput, WsError> {
                ExecutionResult::Ok(WsHandlerOutput::Empty)
            }
        }

        GatewayWrapper::new(
            Arc::new(Box::new(TestGateway)),
            vec![],
            vec![],
            vec![],
            Arc::new(Metadata::new()),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        )
    }
}
