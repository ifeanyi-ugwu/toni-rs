use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::context::WsContext;
use crate::errors::{PanicRecovered, PipelineSegment};
use crate::http_helpers::{ExecutionResult, RouteMetadata};
use crate::traits_helpers::{
    ErrorObserver, Guard, Interceptor, InterceptorNext, Pipe, WsErrorHandlerArc, WsGuardEntry,
    WsInterceptorEntry, WsPipeEntry,
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
    _keep_alive: WsContext,
}

impl futures::Stream for ScopedStream {
    type Item = WsMessage;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

struct WsChainNext {
    interceptors: Vec<Arc<dyn Interceptor<WsContext, WsHandlerResult>>>,
    gateway: Arc<Box<dyn GatewayTrait>>,
    pipes: Vec<Arc<dyn Pipe<WsContext, WsHandlerResult>>>,
    error_handlers: Vec<WsErrorHandlerArc>,
    observers: Vec<Arc<dyn ErrorObserver>>,
}

#[async_trait]
impl InterceptorNext<WsContext, WsHandlerResult> for WsChainNext {
    async fn run(self: Box<Self>, context: &WsContext) -> WsHandlerResult {
        GatewayWrapper::execute_with_interceptors(
            context,
            &self.interceptors,
            &self.gateway,
            &self.pipes,
            &self.error_handlers,
            &self.observers,
        )
        .await
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
    ///
    /// The upgrade's parts are no longer threaded in: a connect guard reads the
    /// handshake through `ctx.client().handshake`, which is the same information
    /// by a route that works per-message too.
    /// Phase 1 of connection setup: run the guards that admit the connection.
    ///
    /// Returns the connect execution's context, which phase 2 finishes. The two phases exist so the
    /// adapter can register the client's sink between them, not because they are separate
    /// executions — a guard's writes have to reach the hook, so they share one context and one bag.
    pub async fn begin_connect(&self, client: WsClient) -> Result<WsContext, WsError> {
        let context = WsContext::new(
            client.clone(),
            WsMessage::text(""),
            "connect",
            Some(self.route_metadata.clone()),
        );

        let guards = Self::resolve_guards(&self.guards, &context).await;
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

        let guards = Self::resolve_guards(&all_guards, &context).await;
        for guard in guards.iter() {
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
                    // pipeline panics so observers + chain run once and
                    // the canonical envelope reaches the client. Without
                    // this, `WsError::AuthFailed` would silently drop in
                    // `ToniApplication`'s message callback and the only
                    // signal would be observer-side.
                    return Self::record_pipeline_panic(
                        &context,
                        &all_error_handlers,
                        &self.error_observers,
                        event,
                    )
                    .await;
                }
            };
            if !activated {
                let err = WsError::AuthFailed("Guard rejected message".into());
                Self::fan_out_observers(&self.error_observers, &err, &context).await;
                return Err(err);
            }
        }

        let interceptors = Self::resolve_interceptors(&all_interceptors, &context).await;
        let pipes = Self::resolve_pipes(&all_pipes, &context).await;

        let answer = Self::execute_with_interceptors(
            &context,
            &interceptors,
            &self.gateway,
            &pipes,
            &all_error_handlers,
            &self.error_observers,
        )
        .await;

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(WsHandlerOutput::Stream(stream)) => Ok(WsHandlerOutput::Stream(
                ScopedStream {
                    inner: stream,
                    _keep_alive: context,
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

    async fn resolve_pipes(
        entries: &[WsPipeEntry],
        ctx: &WsContext,
    ) -> Vec<Arc<dyn Pipe<WsContext, WsHandlerResult>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let p = match entry {
                WsPipeEntry::Ready(p) => p.clone(),
                WsPipeEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(p);
        }
        out
    }

    async fn execute_with_interceptors(
        context: &WsContext,
        interceptors: &[Arc<dyn Interceptor<WsContext, WsHandlerResult>>],
        gateway: &Arc<Box<dyn GatewayTrait>>,
        pipes: &[Arc<dyn Pipe<WsContext, WsHandlerResult>>],
        error_handlers: &[WsErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) -> WsHandlerResult {
        if interceptors.is_empty() {
            return Self::execute_handler_with_error_handling(
                context,
                gateway,
                pipes,
                error_handlers,
                observers,
            )
            .await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = WsChainNext {
            interceptors: rest.to_vec(),
            gateway: gateway.clone(),
            pipes: pipes.to_vec(),
            error_handlers: error_handlers.to_vec(),
            observers: observers.to_vec(),
        };

        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(answer) => answer,
            Err(event) => {
                Self::record_pipeline_panic(context, error_handlers, observers, event).await
            }
        }
    }

    /// Surface a panicking pre-handler segment (interceptor; pipes flow
    /// through `execute_handler`'s `ExecutionResult::Err` instead) through
    /// the existing observer + chain pipeline so it cannot tear down the
    /// connection. Fan to observers, give error handlers first claim,
    /// and fall back to a wire-`Err` frame.
    async fn record_pipeline_panic(
        context: &WsContext,
        error_handlers: &[WsErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        event: PanicRecovered,
    ) -> WsHandlerResult {
        Self::fan_out_observers(observers, &event, context).await;
        for handler in error_handlers.iter().rev() {
            if let Some(claimed) =
                Self::try_chain_handler(handler, &event, context, observers).await
            {
                return Ok(WsHandlerOutput::Single(claimed));
            }
        }
        let ws_err = WsError::from(event);
        let msg = Self::safe_render(|| ws_err.to_message(), observers, context).await;
        Ok(WsHandlerOutput::Single(msg))
    }

    /// Run pipes + handler, then route the outcome.
    ///
    /// `Ok` is the answer, streams included. On `Err`, observers fan out on the
    /// underlying error, the chain's most-specific handler gets first claim,
    /// and `WsError::to_message` is the fallback frame when none claims.
    async fn execute_handler_with_error_handling(
        context: &WsContext,
        gateway: &Arc<Box<dyn GatewayTrait>>,
        pipes: &[Arc<dyn Pipe<WsContext, WsHandlerResult>>],
        error_handlers: &[WsErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) -> WsHandlerResult {
        for pipe in pipes {
            // `pipe.process` is sync — `catch_sync` wraps it the same way
            // `catch_async` wraps async segments. A panic routes through the
            // observer + chain pipeline below; remaining pipes and the handler
            // are skipped.
            match crate::panic_recovery::catch_sync(PipelineSegment::Pipe, || pipe.process(context))
            {
                Ok(Some(answer)) => return answer,
                Ok(None) => {}
                Err(event) => {
                    return Self::record_pipeline_panic(context, error_handlers, observers, event)
                        .await;
                }
            }
        }

        match Self::execute_handler(context, gateway).await {
            ExecutionResult::Ok(output) => Ok(output),
            ExecutionResult::Err(ws_err) => {
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) = match &ws_err {
                    WsError::AppError(e) => e.as_ref(),
                    other => other,
                };
                Self::fan_out_observers(observers, observed_err, context).await;
                for handler in error_handlers.iter().rev() {
                    if let Some(msg) =
                        Self::try_chain_handler(handler, observed_err, context, observers).await
                    {
                        return Ok(WsHandlerOutput::Single(msg));
                    }
                }
                let msg = Self::safe_render(|| ws_err.to_message(), observers, context).await;
                Ok(WsHandlerOutput::Single(msg))
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
