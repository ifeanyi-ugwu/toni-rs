use crate::dispatch::Items;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::context::Metadata;
use crate::dispatch::ExecutionResult;
use crate::dispatch::resolve::Resolved;
use crate::dispatch::transport::Ws;
use crate::enhancer::Interceptor;
use crate::enhancer::pipeline::{GuardFailure, Leaf, through_interceptors};
use crate::ws::WsContext;

use super::{
    DisconnectReason, Gateway, WsClient, WsError, WsHandlerOutput, WsHandlerResult, WsMessage,
};
use futures::StreamExt;

/// The innermost step of the chain: the gateway asked to handle this event.
struct GatewayLeaf(Arc<dyn Gateway>);

#[async_trait]
impl Leaf<Ws> for GatewayLeaf {
    async fn call(&self, context: &WsContext) -> WsHandlerResult {
        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::HandlerBody,
            self.0.handle_event(context),
        )
        .await
        {
            Ok(ExecutionResult::Ok(output)) => Ok(output),
            Ok(ExecutionResult::Err(ws_err)) => Err(ws_err),
            Err(event) => Err(WsError::from(event)),
        }
    }
}

/// Parallel to `RoutePipeline` on the HTTP side — wraps a gateway with the full
/// guard/interceptor pipeline and tracks its own connected clients.
pub(crate) struct GatewayWrapper {
    gateway: Arc<dyn Gateway>,
    /// The gateway's enhancers, each event's merged over them at create. A connect runs the
    /// gateway's set.
    enhancers: Resolved<Ws>,
    metadata: Arc<Metadata>,
    /// Per-event metadata, already merged over `metadata` at expansion. An event absent here
    /// declared nothing of its own and reads the gateway's.
    handler_metadata: HashMap<String, Arc<Metadata>>,
    /// Active client connections (client_id => WsClient). A client carries the session scoped to
    /// its connection, so there is nothing to keep beside it.
    clients: Arc<RwLock<HashMap<String, WsClient>>>,
}

impl GatewayWrapper {
    pub(crate) fn new(
        gateway: Arc<dyn Gateway>,
        enhancers: Resolved<Ws>,
        metadata: Arc<Metadata>,
        handler_metadata: HashMap<String, Arc<Metadata>>,
    ) -> Self {
        Self {
            gateway,
            enhancers,
            metadata,
            handler_metadata,
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
    pub(crate) async fn begin_connect(&self, client: WsClient) -> Result<WsContext, WsError> {
        // The client was born with its session, so a guard below writes to the store every later
        // execution on this connection reads.
        let context = WsContext::new(
            client.clone(),
            WsMessage::text(""),
            "connect",
            Some(self.metadata.clone()),
        );

        // One guard at a time, and a guard after a refusing one is never built. A connect has no
        // chain to route a refusal through: there is no answer to shape on a refused upgrade.
        match crate::enhancer::pipeline::run_guards::<Ws>(&self.enhancers.target().guards, &context)
            .await
        {
            Ok(()) => {}
            Err(GuardFailure::Panicked { index, event }) => {
                // The refusal reaches the caller as a close frame, so the panic is narrated
                // rather than reported. Keeping the event typed is what puts an internal-error
                // close code on the wire instead of a policy one.
                tracing::debug!(client_id = %client.id, guard_index = index, panic = %event.message, "connect guard panicked");
                return Err(WsError::from(event));
            }
            Err(GuardFailure::Rejected(rejection)) => {
                tracing::debug!(client_id = %client.id, guard_index = rejection.guard_index, "guard rejected WebSocket connection");
                return Err(WsError::AuthFailed("Guard rejected connection".into()));
            }
        }

        tracing::debug!(client_id = %client.id, "WebSocket client connected");
        self.clients.write().insert(client.id.clone(), client);
        Ok(context)
    }

    /// Phase 2 of connection setup: fire the `on_connect` lifecycle hook on the context phase 1
    /// built, so the hook reads the bag the guards wrote to.
    pub(crate) async fn complete_connect(&self, context: &WsContext) -> Result<(), WsError> {
        let client = context.client();
        if !self.clients.read().contains_key(&client.id) {
            return Err(WsError::ConnectionClosed("Client not found".into()));
        }

        self.gateway.on_connect(client, context).await
    }

    /// Handle one inbound frame, from routing it to framing the answer.
    ///
    /// Answers `Err` only when there is nothing left to answer on — the client has gone. Every
    /// other outcome, failures included, leaves as an `Ok` the caller writes to the socket.
    pub(crate) async fn handle_message(
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

        // A control frame is the protocol talking, not the application. A keepalive names no
        // event because it is not asking for one, and answering it would put an error frame on
        // the wire in reply to a ping.
        if matches!(
            message,
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Close(_)
        ) {
            return Ok(Items::Empty);
        }

        // A frame naming no event fails to route with the socket still open, so the caller is
        // told, exactly as it is for an event nothing handles. Guards and interceptors do not
        // run: a guard answers whether a caller may make some call and an interceptor wraps
        // that call, and this frame named none. An error handler does run, because there is an
        // answer to shape. Its context carries an empty event, for the same reason.
        let (event, unroutable) = match self.extract_event(&message) {
            Ok(event) => (event, None),
            Err(e) => {
                tracing::debug!(client_id = %client_id, reason = %e, "WebSocket frame did not route");
                (String::new(), Some(e))
            }
        };

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

        let enhancers = self.enhancers.for_key(&event);

        let answer = match unroutable {
            // Resolving guards and interceptors is what constructs them, execution-scoped ones
            // included, so an unroutable frame must not reach it — a flood of garbage would
            // otherwise build a pipeline per frame and run none of it.
            Some(e) => Err(e),
            None => {
                // Guards first, one at a time, and no interceptor built until every one has
                // passed.
                match crate::enhancer::pipeline::run_guards::<Ws>(&enhancers.guards, &context).await
                {
                    Ok(()) => {
                        let interceptors = crate::enhancer::pipeline::interceptors_for::<Ws>(
                            &enhancers.interceptors,
                            &context,
                        )
                        .await;
                        Self::run_chain(&context, &self.gateway, &interceptors).await
                    }
                    Err(GuardFailure::Rejected(rejection)) => {
                        tracing::debug!(
                            guard_index = rejection.guard_index,
                            "guard rejected message"
                        );
                        Err(WsError::from(rejection))
                    }
                    Err(GuardFailure::Panicked { index, event }) => {
                        tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                        Err(WsError::from(event))
                    }
                }
            }
        };

        // The one place the chain runs. A guard's refusal, a panic from any segment and the
        // handler's own error all arrive as `Err`, so a `#[catch]` handler is offered every one of
        // them and an unclaimed one renders the same envelope whichever produced it — except a
        // `Refused`, which renders as the close it names.
        //
        // A message a guard refused renders rather than failing the call: the socket stays open
        // and the client learns its message went nowhere, which is what the read loop needs.
        let answer = match answer {
            Ok(output) => Ok(output),
            Err(ws_err) => {
                let observed: &(dyn std::error::Error + Send + Sync + 'static) = match &ws_err {
                    WsError::AppError(e) => e.as_ref(),
                    other => other,
                };
                match crate::enhancer::pipeline::claim::<Ws>(
                    &enhancers.error_handlers,
                    observed,
                    &context,
                )
                .await
                {
                    Some(Ok(output)) => Ok(output),
                    Some(Err(reshaped)) => {
                        Ok(Items::One(Self::safe_render(|| reshaped.to_message())))
                    }
                    None => Ok(Items::One(Self::safe_render(|| ws_err.to_message()))),
                }
            }
        };

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(Items::Many(stream)) => Ok(Items::Many(
                crate::dispatch::ScopedStream::new(stream, context).boxed(),
            )),
            other => other,
        }
    }

    /// The interceptor chain around the handler. Every way this can fail leaves as `Err`.
    async fn run_chain(
        context: &WsContext,
        gateway: &Arc<dyn Gateway>,
        interceptors: &[Arc<dyn Interceptor<WsContext, WsHandlerResult>>],
    ) -> WsHandlerResult {
        through_interceptors::<Ws>(
            context,
            interceptors,
            Arc::new(GatewayLeaf(gateway.clone())),
        )
        .await
    }

    /// Drive a renderer with the shared recovery, falling back to the canonical envelope.
    fn safe_render<F>(render: F) -> WsMessage
    where
        F: FnOnce() -> WsMessage,
    {
        crate::enhancer::pipeline::safe_render(render, Self::fallback_internal_message)
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

    pub(crate) async fn handle_disconnect(&self, client_id: String, reason: DisconnectReason) {
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
            WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Close(_) => Err(
                WsError::InvalidMessage("Control frames don't have events".into()),
            ),
        }
    }

    pub(crate) async fn call_after_init(&self) {
        self.gateway.after_init().await;
    }

    pub(crate) fn namespace(&self) -> Option<String> {
        self.gateway.namespace()
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.gateway.port()
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
        impl Gateway for TestGateway {
            fn token(&self) -> String {
                "TestGateway".to_string()
            }

            fn path(&self) -> String {
                "/test".to_string()
            }

            async fn handle_event(
                &self,
                _ctx: &WsContext,
            ) -> ExecutionResult<WsHandlerOutput, WsError> {
                ExecutionResult::Ok(Items::Empty)
            }
        }

        GatewayWrapper::new(
            Arc::new(TestGateway),
            Resolved::default(),
            Arc::new(Metadata::new()),
            HashMap::new(),
        )
    }
}
