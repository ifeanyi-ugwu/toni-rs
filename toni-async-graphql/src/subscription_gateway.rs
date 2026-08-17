use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::AbortHandle;
use futures::stream::Abortable;

use async_graphql::{ObjectType, Schema, SubscriptionType};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use toni::traits_helpers::{Provider, ProviderContext};
use toni::{
    context::WsContext, DisconnectReason, GatewayTrait, ProviderScope, WsClient, WsError,
    WsHandlerOutput, WsMessage,
};

use crate::subscription_context_builder::SubscriptionContextBuilder;

// ---- graphql-ws protocol message types --------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum ClientMessage {
    ConnectionInit {
        payload: Option<Value>,
    },
    Subscribe {
        id: String,
        payload: SubscribePayload,
    },
    Complete {
        id: String,
    },
    Ping {
        payload: Option<Value>,
    },
    Pong {
        payload: Option<Value>,
    },
}

#[derive(Debug, Deserialize)]
struct SubscribePayload {
    query: String,
    #[serde(rename = "operationName")]
    operation_name: Option<String>,
    variables: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum ServerMessage<'a> {
    ConnectionAck,
    Next { id: &'a str, payload: Value },
    Error { id: &'a str, payload: Value },
    Complete { id: &'a str },
    Ping,
    Pong,
}

// ---- GraphQLSubscriptionGateway ---------------------------------------

/// WebSocket gateway that implements the graphql-ws protocol for GraphQL subscriptions.
///
/// Register it by calling `.with_subscription_path("/graphql/ws")` on `GraphQLModule`.
/// The gateway handles the full graphql-ws handshake and drives `Schema::execute_stream`
/// as a `WsHandlerOutput::Stream`.
pub struct GraphQLSubscriptionGateway<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    pub(crate) schema: Arc<Schema<Q, M, S>>,
    pub(crate) context_builder: Arc<dyn SubscriptionContextBuilder>,
    pub(crate) path: String,
    // Stores the connection_init payload per client so it can be passed to the
    // context builder when a subscribe message arrives later on the same connection.
    pub(crate) init_payloads: Arc<Mutex<HashMap<String, Value>>>,
    // Tracks running subscription streams by (client_id, subscription_id) so a
    // client-sent "complete" can abort exactly that stream without disconnecting.
    pub(crate) abort_handles: Arc<Mutex<HashMap<(String, String), AbortHandle>>>,
}

impl<Q, M, S> Clone for GraphQLSubscriptionGateway<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    fn clone(&self) -> Self {
        Self {
            schema: self.schema.clone(),
            context_builder: self.context_builder.clone(),
            path: self.path.clone(),
            init_payloads: self.init_payloads.clone(),
            abort_handles: self.abort_handles.clone(),
        }
    }
}

// ---- GatewayTrait -----------------------------------------------------

#[async_trait]
impl<Q, M, S> GatewayTrait for GraphQLSubscriptionGateway<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    fn get_token(&self) -> String {
        format!("GraphQLSubscriptionGateway_{}", self.path)
    }

    fn get_path(&self) -> String {
        self.path.clone()
    }

    // graphql-ws protocol uses "type" as the routing field, not "event".
    fn event_field(&self) -> &str {
        "type"
    }

    // Reject connections that do not negotiate the graphql-transport-ws sub-protocol.
    // Clients using the graphql-ws library set this automatically; raw clients must set it explicitly.
    async fn on_connect(&self, client: &WsClient, _context: &WsContext) -> Result<(), WsError> {
        let protocol = client
            .handshake
            .headers
            .get("sec-websocket-protocol")
            .map(|s| s.as_str());
        if protocol != Some("graphql-transport-ws") {
            return Err(WsError::AuthFailed(
                "graphql-ws requires Sec-WebSocket-Protocol: graphql-transport-ws".into(),
            ));
        }
        Ok(())
    }

    async fn handle_event(
        &self,
        ctx: &toni::context::WsContext,
    ) -> toni::http_helpers::ExecutionResult<WsHandlerOutput, toni::WsError> {
        let client = ctx.client().clone();
        let message = ctx.message().clone();
        let event = ctx.event().to_string();
        self.handle_event_inner(client, message, &event)
            .await
            .into()
    }

    async fn on_disconnect(&self, client: &WsClient, _reason: DisconnectReason) {
        self.init_payloads.lock().unwrap().remove(&client.id);
        let mut handles = self.abort_handles.lock().unwrap();
        handles.retain(|(cid, _), handle| {
            if *cid == client.id {
                handle.abort();
                false
            } else {
                true
            }
        });
    }
}

impl<Q, M, S> GraphQLSubscriptionGateway<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    async fn handle_event_inner(
        &self,
        client: WsClient,
        message: WsMessage,
        event: &str,
    ) -> Result<WsHandlerOutput, WsError> {
        let text = match &message {
            WsMessage::Text(t) => t.clone(),
            _ => {
                return Err(WsError::InvalidMessage(
                    "graphql-ws expects text frames only".into(),
                ))
            }
        };

        // We already extracted the event name from the "type" field, but we need
        // the full parsed message for id / payload. Re-parse now.
        let _ = event; // already routed; re-parse the full envelope below
        let client_msg: ClientMessage = serde_json::from_str(&text)
            .map_err(|e| WsError::InvalidMessage(format!("invalid graphql-ws message: {e}")))?;

        match client_msg {
            ClientMessage::ConnectionInit { payload } => {
                if let Some(v) = payload {
                    self.init_payloads
                        .lock()
                        .unwrap()
                        .insert(client.id.clone(), v);
                }
                let ack = serde_json::to_string(&ServerMessage::ConnectionAck).unwrap();
                Ok(WsHandlerOutput::Single(WsMessage::text(ack)))
            }

            ClientMessage::Ping { .. } => {
                let pong = serde_json::to_string(&ServerMessage::Pong).unwrap();
                Ok(WsHandlerOutput::Single(WsMessage::text(pong)))
            }

            ClientMessage::Pong { .. } => Ok(WsHandlerOutput::Empty),

            ClientMessage::Complete { id } => {
                if let Some(handle) = self
                    .abort_handles
                    .lock()
                    .unwrap()
                    .remove(&(client.id.clone(), id))
                {
                    handle.abort();
                }
                Ok(WsHandlerOutput::Empty)
            }

            ClientMessage::Subscribe { id, payload } => {
                self.handle_subscribe(client, id, payload).await
            }
        }
    }

    async fn handle_subscribe(
        &self,
        client: WsClient,
        id: String,
        payload: SubscribePayload,
    ) -> Result<WsHandlerOutput, WsError> {
        let init_payload = self.init_payloads.lock().unwrap().get(&client.id).cloned();
        let context_data = self.context_builder.build(&client, init_payload).await;

        let mut request = async_graphql::Request::new(payload.query);
        if let Some(op) = payload.operation_name {
            request = request.operation_name(op);
        }
        if let Some(vars) = payload.variables {
            request = request.variables(async_graphql::Variables::from_json(vars));
        }
        request.data = context_data;

        // `execute_stream` borrows `&self` (the schema), making the returned stream's
        // lifetime tied to a local borrow. Bridge via a channel: a spawned task owns the
        // schema and drives the inner stream; the receiver is 'static and Send.
        let schema = self.schema.clone();
        let (tx, rx) = futures::channel::mpsc::channel::<async_graphql::Response>(16);

        tokio::spawn(async move {
            use futures::{SinkExt, StreamExt};
            let mut tx = tx;
            let s = schema.execute_stream(request);
            futures::pin_mut!(s);
            while let Some(item) = s.next().await {
                if tx.send(item).await.is_err() {
                    break;
                }
            }
        });

        let id_next = id.clone();
        let next_stream = futures::StreamExt::map(rx, move |response| {
            let payload = serde_json::to_value(&response).unwrap_or(Value::Null);
            let frame = serde_json::to_string(&ServerMessage::Next {
                id: &id_next,
                payload,
            })
            .unwrap();
            WsMessage::text(frame)
        });

        let id_complete = id.clone();
        let complete_frame = futures::stream::once(async move {
            let json =
                serde_json::to_string(&ServerMessage::Complete { id: &id_complete }).unwrap();
            WsMessage::text(json)
        });

        let full_stream = futures::StreamExt::chain(next_stream, complete_frame);

        let (abort_handle, abort_reg) = AbortHandle::new_pair();
        self.abort_handles
            .lock()
            .unwrap()
            .insert((client.id.clone(), id), abort_handle);

        Ok(WsHandlerOutput::Stream(Box::pin(Abortable::new(
            full_stream,
            abort_reg,
        ))))
    }
}

// ---- Provider ---------------------------------------------------------
// Required so the gateway can be the `instance` field inside `Injectable`.

#[async_trait]
impl<Q, M, S> Provider for GraphQLSubscriptionGateway<Q, M, S>
where
    Q: ObjectType + 'static,
    M: ObjectType + 'static,
    S: SubscriptionType + 'static,
{
    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(self.clone())
    }

    fn get_token(&self) -> String {
        format!("GraphQLSubscriptionGateway_{}", self.path)
    }

    fn get_token_factory(&self) -> String {
        format!("GraphQLSubscriptionGatewayFactory_{}", self.path)
    }

    fn get_scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }
}
