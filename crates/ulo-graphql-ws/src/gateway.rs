//! The graphql-transport-ws gateway.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::{self, Either};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ulo::{
    Bound, CancelReason, Construct, ConstructError, Controller, Dep, Dependencies, Execution, ExecutionRef,
    FromContainer, Mount, Resolver, scope::Auto,
};
use ulo_graphql::{BoxStream, Engine, GqlError, GqlRequest, GqlResponse, Outcome};
use ulo_transport::{ErrorKind, Tracked};
use ulo_ws::{
    ConnectRefused, Connection, DisconnectReason, Frame, Gateway, GatewayConfig, GatewaySettings, SessionFactory,
};

/// The subprotocol the handshake must have echoed.
const PROTOCOL: &str = "graphql-transport-ws";

/// `connection_init_timeout` at `Bound::Default`: the reference server's
/// `connectionInitWaitTimeout`.
const DEFAULT_INIT_TIMEOUT: Duration = Duration::from_secs(3);

/// A close reason is at most 123 bytes of UTF-8 (RFC 6455 §5.5.1).
const MAX_REASON: usize = 123;

/// The gateway, serving the engine bound as `dyn Engine` (or `dyn Engine @ Q`): a controller listed
/// in a module's `controllers` beside an import of `WsModule`, mounted at the subscription path
/// with `ModuleDef::controller::<GraphqlWs>().at(path)`. Its settings: `event = "type"`,
/// `subprotocols = ["graphql-transport-ws"]`.
///
/// `GraphqlModule` mounts it at `GraphqlConfig::subscriptions(path)`, which is the usual way to
/// serve it. The engine binding has to be visible from the module that mounts it: under
/// `GraphqlModule`, bound in a global module that exports `dyn Engine`.
///
/// A connection whose handshake echoed no `graphql-transport-ws` is refused with 4406, and one
/// whose `connection_init` does not arrive within `connection_init_timeout` (3 seconds unset)
/// closes with 4408. Each `subscribe` runs in its own execution, opened on the connection with its
/// inputs seeded, in which the engine builds the schema's context; a query or mutation sent as a
/// `subscribe` answers one `next`.
pub struct GraphqlWs<Q = ()> {
    pub(crate) _engine: PhantomData<fn() -> Q>,
    engine: Dep<dyn Engine, Q>,
    /// `None` under `Bound::Unbounded`.
    init_timeout: Option<Duration>,
}

/// The `connection_init` payload, kept in the connection's session for the context an engine
/// builds per execution: `Session<ConnectionInit>`.
///
/// It also holds the connection's protocol state, which the gateway reads; a clone shares it.
#[derive(Clone, Debug, Default)]
pub struct ConnectionInit {
    pub(crate) payload: OnceLock<Map<String, Value>>,
    pub(crate) state: Arc<ConnState>,
}

impl ConnectionInit {
    /// The payload the client sent, `None` before `connection_init` arrives or when it sent none.
    pub fn payload(&self) -> Option<&Map<String, Value>> {
        self.payload.get()
    }
}

/// `GraphqlConfig::connection_init_timeout`, as `GraphqlModule` binds it for the gateway it mounts.
/// Read optionally: a gateway mounted without it takes the 3-second default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitTimeout(pub Bound);

/// One connection's protocol state. The lock is never held across an await.
#[derive(Default)]
pub(crate) struct ConnState {
    inner: Mutex<Operations>,
}

#[derive(Default)]
struct Operations {
    acknowledged: bool,
    running: HashMap<String, Running>,
    /// Tells a finished operation from a later one the client started under the same `id`.
    next: u64,
}

struct Running {
    seq: u64,
    exec: ExecutionRef,
}

impl ConnState {
    fn lock(&self) -> MutexGuard<'_, Operations> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_acknowledged(&self) -> bool {
        self.lock().acknowledged
    }

    /// Forgets the operation `id` once its stream has ended, unless a `complete`, a disconnect or a
    /// new `subscribe` under the same `id` replaced it first.
    fn finish(&self, id: &str, seq: u64) {
        let mut ops = self.lock();
        if ops.running.get(id).is_some_and(|running| running.seq == seq) {
            ops.running.remove(id);
        }
    }
}

impl fmt::Debug for ConnState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnState").finish_non_exhaustive()
    }
}

/// What a client sends (graphql-ws PROTOCOL.md). Anything else, an unknown `type` included, does
/// not deserialize and closes the connection with 4400.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    ConnectionInit {
        #[serde(default)]
        payload: Option<Map<String, Value>>,
    },
    Ping {
        #[serde(default)]
        payload: Option<Map<String, Value>>,
    },
    /// Accepted and ignored, its payload unread.
    Pong {},
    Subscribe {
        id: String,
        payload: GqlRequest,
    },
    Complete {
        id: String,
    },
}

/// What the gateway sends.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage<'a> {
    ConnectionAck,
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<&'a Map<String, Value>>,
    },
    Next {
        id: &'a str,
        payload: &'a GqlResponse,
    },
    Error {
        id: &'a str,
        payload: &'a [GqlError],
    },
    Complete {
        id: &'a str,
    },
}

impl<Q: Send + Sync + 'static> Construct for GraphqlWs<Q> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<dyn Engine, Q>>("engine").field::<Option<Dep<InitTimeout>>>("init_timeout");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        let engine = r.dep_qualified::<dyn Engine, Q>().await?;
        let configured = <Option<Dep<InitTimeout>> as FromContainer>::from_container(r).await?;
        let init_timeout = match configured.map_or(Bound::Default, |timeout| timeout.0) {
            Bound::Default => Some(DEFAULT_INIT_TIMEOUT),
            Bound::After(after) => Some(after),
            Bound::Unbounded => None,
        };
        Ok(GraphqlWs { _engine: PhantomData, engine, init_timeout })
    }
}

impl<Q: Send + Sync + 'static> GatewayConfig for GraphqlWs<Q> {
    /// At `/`, which the controller's `.at(path)` prefix puts at the subscription path.
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/").event("type").subprotocols([PROTOCOL])
    }

    fn session() -> SessionFactory {
        SessionFactory::default_of::<ConnectionInit>()
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

impl<Q: Send + Sync + 'static> Gateway for GraphqlWs<Q> {
    async fn on_connect(&self, conn: &Connection) -> Result<(), ConnectRefused> {
        if conn.head().subprotocol() != Some(PROTOCOL) {
            return Err(refusal(4406, "Subprotocol not acceptable"));
        }
        if let Some(after) = self.init_timeout {
            let Some(session) = conn.session().get::<ConnectionInit>() else {
                return Err(ConnectRefused::kind(ErrorKind::Internal, "the connection has no graphql-transport-ws session"));
            };
            let state = Arc::clone(&session.state);
            let timer = Arc::clone(conn.timer());
            let conn = conn.clone();
            tokio::spawn(async move {
                timer.sleep(after).await;
                if !state.is_acknowledged() {
                    conn.close(4408, "Connection initialisation timeout").await;
                }
            });
        }
        Ok(())
    }

    async fn on_message(&self, conn: &Connection, frame: Frame) {
        let Some(session) = conn.session().get::<ConnectionInit>() else {
            conn.close(1011, "the connection has no graphql-transport-ws session").await;
            return;
        };
        let message = match &frame {
            Frame::Text(text) => serde_json::from_str::<ClientMessage>(text).ok(),
            Frame::Binary(_) => None,
        };
        let Some(message) = message else {
            conn.close(4400, "Invalid message received").await;
            return;
        };
        match message {
            ClientMessage::ConnectionInit { payload } => {
                let first = {
                    let mut ops = session.state.lock();
                    let first = !ops.acknowledged;
                    if first {
                        // Set under the lock, so a `subscribe` admitted after the ack always finds
                        // the payload its context may read.
                        if let Some(payload) = payload {
                            let _ = session.payload.set(payload);
                        }
                        ops.acknowledged = true;
                    }
                    first
                };
                if first {
                    send(conn, &ServerMessage::ConnectionAck).await;
                } else {
                    conn.close(4429, "Too many initialisation requests").await;
                }
            }
            ClientMessage::Ping { payload } => {
                send(conn, &ServerMessage::Pong { payload: payload.as_ref() }).await;
            }
            ClientMessage::Pong {} => {}
            ClientMessage::Subscribe { id, payload } => self.subscribe(conn, &session, id, payload).await,
            ClientMessage::Complete { id } => {
                let running = session.state.lock().running.remove(&id);
                if let Some(running) = running {
                    running.exec.cancel_with(CancelReason::ClientCancelled);
                }
            }
        }
    }

    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        let _ = why;
        let Some(session) = conn.session().get::<ConnectionInit>() else { return };
        let running: Vec<Running> = session.state.lock().running.drain().map(|(_, running)| running).collect();
        for operation in running {
            operation.exec.cancel_with(CancelReason::Disconnected);
        }
    }
}

impl<Q: Send + Sync + 'static> Controller for GraphqlWs<Q> {
    fn mount(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

/// Whether a `subscribe` may start.
enum Admission {
    Unauthorized,
    Duplicate,
    Draining,
    Open(Execution, u64),
}

impl<Q: Send + Sync + 'static> GraphqlWs<Q> {
    async fn subscribe(&self, conn: &Connection, session: &ConnectionInit, id: String, request: GqlRequest) {
        let admission = {
            let mut ops = session.state.lock();
            if !ops.acknowledged {
                Admission::Unauthorized
            } else if ops.running.contains_key(&id) {
                Admission::Duplicate
            } else {
                match conn.open_execution() {
                    Ok(exec) => {
                        let seq = ops.next;
                        ops.next += 1;
                        ops.running.insert(id.clone(), Running { seq, exec: exec.handle() });
                        Admission::Open(exec, seq)
                    }
                    Err(_) => Admission::Draining,
                }
            }
        };
        match admission {
            Admission::Unauthorized => conn.close(4401, "Unauthorized").await,
            Admission::Duplicate => {
                let reason = format!("Subscriber for {id} already exists");
                let reason = if reason.len() <= MAX_REASON { reason } else { "Subscriber already exists".to_owned() };
                conn.close(4409, &reason).await;
            }
            Admission::Draining => {
                let errors = [GqlError::new("the server is shutting down")];
                send(conn, &ServerMessage::Error { id: &id, payload: &errors }).await;
            }
            Admission::Open(exec, seq) => {
                let stream = Tracked::new(self.engine.subscribe(request, exec.handle()), exec.handle());
                tokio::spawn(run(conn.clone(), Arc::clone(&session.state), id, seq, exec, stream));
            }
        }
    }
}

/// One operation: each response written as `next` until the stream ends with `complete`; a
/// request error written as `error`, which ends the operation without `complete`. A `complete`
/// from the client or a disconnect cancels the execution and ends it silently; the drain ends it
/// with `complete`, so the client learns the server stopped it.
async fn run(
    conn: Connection,
    state: Arc<ConnState>,
    id: String,
    seq: u64,
    exec: Execution,
    mut stream: Tracked<BoxStream<'static, GqlResponse>>,
) {
    let mut stop = future::select(exec.cancelled(), exec.draining());
    let complete = loop {
        match future::select(stream.next(), &mut stop).await {
            Either::Left((Some(response), _)) => {
                let written = match response.outcome() {
                    Outcome::Executed => send(&conn, &ServerMessage::Next { id: &id, payload: &response }).await,
                    Outcome::RequestError => {
                        send(&conn, &ServerMessage::Error { id: &id, payload: response.errors() }).await;
                        break false;
                    }
                };
                if !written {
                    break false;
                }
            }
            Either::Left((None, _)) => break true,
            Either::Right(_) => break !exec.is_cancelled(),
        }
    };
    if complete {
        send(&conn, &ServerMessage::Complete { id: &id }).await;
    }
    // Dropped after the end reached the wire, so `on_stream_end` callbacks read how it ended.
    drop(stream);
    state.finish(&id, seq);
}

/// Writes `message` as one text frame; `false` once the connection has closed.
async fn send(conn: &Connection, message: &ServerMessage<'_>) -> bool {
    match serde_json::to_string(message) {
        Ok(text) => conn.send(Frame::text(text)).await.is_ok(),
        Err(error) => {
            tracing::error!(%error, "a graphql-transport-ws message could not be serialized");
            false
        }
    }
}

/// A refusal with a protocol close code. `code` and `reason` are constants `ConnectRefused::code`
/// accepts; the fallback keeps the refusal should that ever change.
fn refusal(code: u16, reason: &'static str) -> ConnectRefused {
    ConnectRefused::code(code, reason).unwrap_or_else(|_| ConnectRefused::kind(ErrorKind::Internal, reason))
}
