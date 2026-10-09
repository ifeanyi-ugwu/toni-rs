//! Gateways and their connection hooks (transports DESIGN §4.1, §4.2).
//!
//! `#[ulo_ws::gateway(..)]` on a `#[routes]` impl writes [`GatewayConfig`]; its `#[message]`
//! handlers are `Ws` handlers routed by the envelope's event. A gateway whose protocol does not fit
//! the envelope, graphql-transport-ws for one, implements [`Gateway`] by hand: it receives each
//! data message raw and answers through its [`Connection`].

use std::any::Any;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use http::StatusCode;
use ulo::{
    BoxError, BoxFuture, Bound, Construct, EnhancerSpec, ExecOptions, Execution, ExecutionRef, ModuleRef, Mount,
    MountedHandler, TypeName,
};
use ulo_transport::prepare::{Failure, Failures, Names, zero_bound, zero_count};
use ulo_transport::{CallError, Count, ErrorKind};

use crate::__private::{HandlerFn, Hooks, WsHandler};
use crate::codec::Codec;
use crate::connection::Connection;
use crate::envelope::Frame;
use crate::table::GatewayDefaults;
use crate::session::SessionFactory;
use crate::transport::{ConnectCx, ConnectReply, Ws, WsConnect};

/// A gateway's configuration, which `#[ulo_ws::gateway(..)]` writes on the impl it sits on and a
/// hand-written [`Gateway`] implements: where it listens, how its messages are framed, its
/// connect guards, its session, and the mount of its connect handler.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a WebSocket gateway",
    label = "a `#[ulo_ws::message]` handler needs its impl to be a gateway",
    note = "add `#[ulo_ws::gateway(path = \"/..\")]` to the `#[routes]` impl, or implement `ulo_ws::Gateway` by hand"
)]
pub trait GatewayConfig: Construct {
    /// The gateway attribute's arguments.
    fn settings() -> GatewaySettings;

    /// The connect guards, `connect_guards(..)` on the attribute: `Guard<WsConnect>`
    /// implementations, run through `dispatch` in the connection phase.
    fn connect_guards(spec: &mut EnhancerSpec<WsConnect>) {
        let _ = spec;
    }

    /// The session every connection starts with, created before the connect guards run: none,
    /// `session = T` for `T::default()`, or `session_with = |head: Dep<UpgradeHead>| ..`.
    fn session() -> SessionFactory {
        SessionFactory::none()
    }

    /// Mounts the gateway's connect handler under `WsConnect`, once per controller type (X15).
    /// Every `#[message]` mount function calls it; the attribute writes it with the connection
    /// hooks the type implements, found by probing the concrete type. A hand-written gateway
    /// writes `ulo_ws::Gateway::mount(m)`.
    fn mount_gateway(m: &mut Mount<'_>);
}

/// A gateway written by hand, without `#[message]` handlers: each data message reaches
/// [`on_message`](Self::on_message) as it arrived, and the gateway answers through
/// `Connection::send`, opening an execution per unit of work with `Connection::open_execution`.
///
/// It is a controller: `impl Controller for G { fn mount(m: &mut Mount<'_>) { <G as Gateway>::mount(m) } }`,
/// listed in a module's `controllers`, beside an import of `WsModule`.
///
/// The gateway instance `on_message` runs on is resolved once per connection, in the connection
/// phase's execution, and kept while the connection lasts. Messages reach `on_message` one at a
/// time and in order; work that outlives a message runs in an execution the gateway opens.
pub trait Gateway: GatewayConfig {
    /// After the connect guards admit; an `Err` refuses the connection with its close code.
    fn on_connect(&self, conn: &Connection) -> impl Future<Output = Result<(), ConnectRefused>> + Send {
        let _ = conn;
        async { Ok(()) }
    }

    /// One data message, text or binary, as it arrived. Control frames never reach it.
    fn on_message(&self, conn: &Connection, frame: Frame) -> impl Future<Output = ()> + Send;

    /// After the connection ended, as a terminal execution during the drain. A connection the
    /// connect phase refused never reaches it.
    fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) -> impl Future<Output = ()> + Send {
        let _ = (conn, why);
        async {}
    }

    /// Once per gateway, after `listen()`.
    fn after_init(&self, gw: GatewayRef) -> impl Future<Output = ()> + Send {
        let _ = gw;
        async {}
    }

    /// Mounts the connect handler, which carries this gateway's message entry point, once per
    /// controller type.
    fn mount(m: &mut Mount<'_>) {
        m.once::<crate::__private::HandWritten<Self>>(crate::__private::mount_hand_written::<Self>);
    }
}

/// What a gateway attribute declares. A limit left at `Count::Default` or `Bound::Default`, and a
/// `message_limit` left `None`, takes the standalone server's default for a gateway on its own
/// port (its [`GatewayDefaults`](crate::GatewayDefaults)) and `WsModule`'s for one on the HTTP
/// server's port.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct GatewaySettings {
    pub path: Cow<'static, str>,
    pub namespace: Option<Cow<'static, str>>,
    /// The envelope's event field: `"event"` unset, `"type"` for graphql-ws.
    pub event: Cow<'static, str>,
    pub codec: Codec,
    /// What the handshake may echo in `Sec-WebSocket-Protocol`, in preference order: the first a
    /// client offered. None listed echoes none.
    pub subprotocols: Vec<Cow<'static, str>>,
    pub refuse: Refuse,
    /// Bytes per message after reassembly.
    pub message_limit: Option<u64>,
    pub max_connections: Count,
    pub max_inflight: Count,
    pub max_outbound: Count,
    pub overflow: Overflow,
    pub ping_interval: Bound,
    pub pong_timeout: Bound,
    /// Which server serves the gateway: the HTTP server's port, the default, or a standalone
    /// server such as `ulo_ws_hyper::Server`, `port = own` on the attribute.
    pub port: Port,
}

impl GatewaySettings {
    /// A gateway at `path`, every other setting unset.
    pub fn at(path: impl Into<Cow<'static, str>>) -> Self {
        GatewaySettings {
            path: path.into(),
            namespace: None,
            event: Cow::Borrowed("event"),
            codec: Codec::Json,
            subprotocols: Vec::new(),
            refuse: Refuse::Close,
            message_limit: None,
            max_connections: Count::Default,
            max_inflight: Count::Default,
            max_outbound: Count::Default,
            overflow: Overflow::Close,
            ping_interval: Bound::Default,
            pong_timeout: Bound::Default,
            port: Port::Http,
        }
    }

    pub fn namespace(mut self, namespace: impl Into<Cow<'static, str>>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    pub fn event(mut self, field: impl Into<Cow<'static, str>>) -> Self {
        self.event = field.into();
        self
    }

    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    pub fn subprotocols(mut self, names: impl IntoIterator<Item = impl Into<Cow<'static, str>>>) -> Self {
        self.subprotocols = names.into_iter().map(Into::into).collect();
        self
    }

    pub fn refuse(mut self, refuse: Refuse) -> Self {
        self.refuse = refuse;
        self
    }

    pub fn message_limit(mut self, bytes: u64) -> Self {
        self.message_limit = Some(bytes);
        self
    }

    pub fn max_connections(mut self, connections: Count) -> Self {
        self.max_connections = connections;
        self
    }

    pub fn max_inflight(mut self, messages: Count) -> Self {
        self.max_inflight = messages;
        self
    }

    pub fn max_outbound(mut self, messages: Count) -> Self {
        self.max_outbound = messages;
        self
    }

    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }

    pub fn ping_interval(mut self, interval: Bound) -> Self {
        self.ping_interval = interval;
        self
    }

    pub fn pong_timeout(mut self, timeout: Bound) -> Self {
        self.pong_timeout = timeout;
        self
    }

    pub fn port(mut self, port: Port) -> Self {
        self.port = port;
        self
    }
}

/// Where a connect refusal happens: after the 101 with a close code, the one refusal a browser
/// can read (the default), or before it with 401 or 403, `refuse = handshake`, for non-browser
/// clients and proxy logs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Refuse {
    #[default]
    Close,
    Handshake,
}

/// What an overflowing outbound queue does: close with 1008 "slow consumer" (the default), or
/// drop the oldest queued message, `overflow = drop_oldest`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Overflow {
    #[default]
    Close,
    DropOldest,
}

/// Which server serves a gateway. `Http`, the default, is the upgrade hand-off `WsModule`
/// registers on the HTTP server's port and on an embedding host declaring `upgrades`; `Own`,
/// `port = own` on the attribute, is a standalone server on a port of its own, such as
/// `ulo_ws_hyper::Server`, serving through
/// [`GatewayTable::own_port`](crate::GatewayTable::own_port). Each serves only the gateways
/// naming it, so no gateway is reachable on a port its author did not choose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Port {
    #[default]
    Http,
    Own,
}

/// Runs after the connect guards admit, and may refuse: implemented on the gateway type, which
/// the gateway attribute detects by probing the concrete type. Impl-level enhancers do not reach
/// it.
pub trait OnConnect: Send + Sync + 'static {
    fn on_connect(&self, cx: &crate::transport::ConnectCx) -> impl Future<Output = Result<(), ConnectRefused>> + Send;
}

/// Runs once a connection that connected has ended, as a terminal execution without guards; its
/// failures and panics are logged.
pub trait OnDisconnect: Send + Sync + 'static {
    fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) -> impl Future<Output = ()> + Send;
}

/// Runs once per gateway after `listen()`, on the HTTP server's port from the upgrade handler's
/// `bound` (X21). A lifecycle call, not an execution: the gateway instance is resolved in a
/// short-lived execution that ends before the hook runs.
pub trait AfterInit: Send + Sync + 'static {
    fn after_init(&self, gw: GatewayRef) -> impl Future<Output = ()> + Send;
}

/// One gateway as `AfterInit` sees it.
#[derive(Clone, Debug)]
pub struct GatewayRef {
    pub(crate) path: Cow<'static, str>,
    pub(crate) namespace: Option<Cow<'static, str>>,
}

impl GatewayRef {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }
}

/// Why a connection ended, as `on_disconnect` receives it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    /// The client sent a Close frame. A frame without a status reads as code 1005.
    ClientClose { code: u16, reason: String },
    /// The gateway closed the connection: `Connection::close`, a slow consumer's 1008, a binary
    /// frame on a text-only gateway's 1003.
    ServerClose { code: u16 },
    /// A protocol, UTF-8 or capacity error the protocol layer reported.
    ProtocolError,
    /// The drain's 1001.
    Drain,
    /// An I/O error, a connection closed without a Close frame, or a Pong that missed
    /// `pong_timeout`.
    Lost,
}

/// A connection refused in the connect phase, with the close code it ends with: by kind, 1008 for
/// `Unauthorized` and `Forbidden`, 1013 for `TooManyRequests` and `Unavailable`, 1011 for
/// `Internal`; or one set with [`code`](Self::code), as graphql-transport-ws sets 4401.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectRefused {
    pub(crate) close_code: u16,
    pub(crate) reason: String,
    pub(crate) kind: Option<ErrorKind>,
}

/// The longest close reason a Close frame carries (RFC 6455 §5.5.1: 125 bytes of payload, two of
/// them the code).
pub(crate) const REASON_LIMIT: usize = 123;

impl ConnectRefused {
    /// A refusal with the close code `code` and `reason`. Refused: a reason over 123 bytes of
    /// UTF-8 (RFC 6455 §5.5.1), 1004, 1005, 1006 and 1015, which are never sent in a frame, and a
    /// code outside 1000–4999.
    pub fn code(code: u16, reason: impl Into<String>) -> Result<Self, CloseCodeError> {
        let reason = reason.into();
        check_close(code, &reason)?;
        Ok(ConnectRefused { close_code: code, reason, kind: None })
    }

    /// A refusal of kind `kind`, closing with the code the kind maps to; under `refuse =
    /// handshake`, answered 401 for `Unauthorized` and 403 otherwise. A reason over 123 bytes is
    /// cut at the last character boundary within them.
    ///
    /// The kinds the design names map as it states; the others, the client's own mistakes
    /// (`BadRequest`, `NotFound`, `Conflict`, `Unprocessable`) close with 1008 and the server's
    /// (`Timeout`, `Unimplemented`, and any kind added later) with 1011.
    pub fn kind(kind: ErrorKind, reason: impl Into<String>) -> Self {
        ConnectRefused { close_code: close_code_for(kind), reason: truncated(reason.into()), kind: Some(kind) }
    }

    pub fn close_code(&self) -> u16 {
        self.close_code
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// The handshake's answer under `refuse = handshake`.
    pub(crate) fn status(&self) -> StatusCode {
        match self.kind {
            Some(ErrorKind::Unauthorized) => StatusCode::UNAUTHORIZED,
            _ => StatusCode::FORBIDDEN,
        }
    }

    /// An error the connect phase's error handlers left unclaimed, refused by its kind.
    pub(crate) fn of_error(err: BoxError) -> Self {
        let error = CallError::from_boxed(err);
        ConnectRefused::kind(error.kind(), error.message())
    }
}

impl fmt::Display for ConnectRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connection refused with close code {}: {}", self.close_code, self.reason)
    }
}

impl Error for ConnectRefused {}

/// Why [`ConnectRefused::code`] refused its arguments.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseCodeError {
    /// The reason is over 123 bytes of UTF-8.
    ReasonTooLong { len: usize },
    /// 1004, 1005, 1006 or 1015.
    Reserved { code: u16 },
    /// Outside 1000–4999.
    OutOfRange { code: u16 },
}

impl fmt::Display for CloseCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CloseCodeError::ReasonTooLong { len } => {
                write!(f, "a close reason is at most 123 bytes of UTF-8, and this one is {len}")
            }
            CloseCodeError::Reserved { code } => write!(f, "close code {code} is never sent in a Close frame"),
            CloseCodeError::OutOfRange { code } => write!(f, "close code {code} is outside 1000–4999"),
        }
    }
}

impl Error for CloseCodeError {}

/// The checks `ConnectRefused::code` and `Connection::close` apply to a code and reason.
pub(crate) fn check_close(code: u16, reason: &str) -> Result<(), CloseCodeError> {
    if !(1000..=4999).contains(&code) {
        return Err(CloseCodeError::OutOfRange { code });
    }
    if matches!(code, 1004 | 1005 | 1006 | 1015) {
        return Err(CloseCodeError::Reserved { code });
    }
    if reason.len() > REASON_LIMIT {
        return Err(CloseCodeError::ReasonTooLong { len: reason.len() });
    }
    Ok(())
}

fn close_code_for(kind: ErrorKind) -> u16 {
    match kind {
        ErrorKind::Unauthorized
        | ErrorKind::Forbidden
        | ErrorKind::BadRequest
        | ErrorKind::NotFound
        | ErrorKind::Conflict
        | ErrorKind::Unprocessable => 1008,
        ErrorKind::TooManyRequests | ErrorKind::Unavailable => 1013,
        _ => 1011,
    }
}

/// `reason` cut to at most 123 bytes at a character boundary.
pub(crate) fn truncated(mut reason: String) -> String {
    if reason.len() > REASON_LIMIT {
        let mut end = REASON_LIMIT;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    reason
}

/// The gateway instance a hand-written gateway's messages run on, erased.
pub(crate) type Instance = Arc<dyn Any + Send + Sync>;

/// The connect handler's call: `OnConnect` or `Gateway::on_connect`, inside `dispatch` after the
/// connect guards.
pub(crate) type AdmitFn = Arc<dyn Fn(ConnectCx) -> BoxFuture<'static, Result<ConnectReply, BoxError>> + Send + Sync>;

/// `on_disconnect`, given the terminal execution it resolves the gateway in.
pub(crate) type DisconnectFn =
    Arc<dyn Fn(ExecutionRef, Connection, DisconnectReason) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;

/// `after_init`, given the gateway's module.
pub(crate) type AfterInitFn = Arc<dyn Fn(ModuleRef, GatewayRef) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;

pub(crate) type InstanceFn = Arc<dyn Fn(ExecutionRef) -> BoxFuture<'static, Result<Instance, BoxError>> + Send + Sync>;

pub(crate) type MessageFn = Arc<dyn Fn(Instance, Connection, Frame) -> BoxFuture<'static, ()> + Send + Sync>;

/// How a gateway's data messages are read.
pub(crate) enum Messages {
    /// The envelope, routed by event to the `#[message]` handlers.
    Envelope,
    /// Raw, to a hand-written gateway's `on_message`.
    Raw { instance: InstanceFn, on_message: MessageFn },
}

/// The handler value of a gateway's connect handler, mounted under `WsConnect` as
/// `Arc<ConnectHandler>`: everything the hand-off and the standalone server read about a gateway
/// besides its message handlers.
pub(crate) struct ConnectHandler {
    pub(crate) settings: GatewaySettings,
    pub(crate) session: SessionFactory,
    pub(crate) admit: AdmitFn,
    pub(crate) messages: Messages,
    pub(crate) disconnect: Option<DisconnectFn>,
    pub(crate) after_init: Option<AfterInitFn>,
}

impl ConnectHandler {
    /// An attributed gateway's: the hooks its type implements, found by the attribute's probes.
    pub(crate) fn attributed<G: GatewayConfig>(hooks: Hooks<G>) -> Self {
        let Hooks { on_connect, on_disconnect, after_init } = hooks;
        let admit: AdmitFn = Arc::new(move |cx: ConnectCx| -> BoxFuture<'static, Result<ConnectReply, BoxError>> {
            Box::pin(async move {
                let Some(hook) = on_connect else { return Ok(ConnectReply::Admitted) };
                let gateway = cx.exec().get::<G>().await?;
                Ok(match hook(&*gateway, &cx).await {
                    Ok(()) => ConnectReply::Admitted,
                    Err(refused) => ConnectReply::Refused(refused),
                })
            })
        });
        let disconnect = on_disconnect.map(|hook| -> DisconnectFn {
            Arc::new(move |exec: ExecutionRef, conn: Connection, why: DisconnectReason| -> BoxFuture<'static, Result<(), BoxError>> {
                Box::pin(async move {
                    let gateway = exec.get::<G>().await?;
                    hook(&*gateway, &conn, why).await;
                    Ok(())
                })
            })
        });
        let after_init = after_init.map(|hook| -> AfterInitFn {
            Arc::new(move |module: ModuleRef, gw: GatewayRef| -> BoxFuture<'static, Result<(), BoxError>> {
                Box::pin(async move {
                    let gateway = resolve::<G>(&module).await?;
                    hook(&*gateway, gw).await;
                    Ok(())
                })
            })
        });
        ConnectHandler {
            settings: G::settings(),
            session: G::session(),
            admit,
            messages: Messages::Envelope,
            disconnect,
            after_init,
        }
    }

    /// A hand-written gateway's: its `Gateway` methods.
    pub(crate) fn hand_written<G: Gateway>() -> Self {
        let admit: AdmitFn = Arc::new(|cx: ConnectCx| -> BoxFuture<'static, Result<ConnectReply, BoxError>> {
            Box::pin(async move {
                let gateway = cx.exec().get::<G>().await?;
                Ok(match <G as Gateway>::on_connect(&gateway, cx.conn()).await {
                    Ok(()) => ConnectReply::Admitted,
                    Err(refused) => ConnectReply::Refused(refused),
                })
            })
        });
        let instance: InstanceFn = Arc::new(|exec: ExecutionRef| -> BoxFuture<'static, Result<Instance, BoxError>> {
            Box::pin(async move {
                let gateway: Instance = exec.get::<G>().await?.into_arc();
                Ok(gateway)
            })
        });
        let on_message: MessageFn = Arc::new(|instance: Instance, conn: Connection, frame: Frame| -> BoxFuture<'static, ()> {
            Box::pin(async move {
                if let Ok(gateway) = instance.downcast::<G>() {
                    <G as Gateway>::on_message(&gateway, &conn, frame).await;
                }
            })
        });
        let disconnect: DisconnectFn =
            Arc::new(|exec: ExecutionRef, conn: Connection, why: DisconnectReason| -> BoxFuture<'static, Result<(), BoxError>> {
                Box::pin(async move {
                    let gateway = exec.get::<G>().await?;
                    <G as Gateway>::on_disconnect(&gateway, &conn, why).await;
                    Ok(())
                })
            });
        let after_init: AfterInitFn = Arc::new(|module: ModuleRef, gw: GatewayRef| -> BoxFuture<'static, Result<(), BoxError>> {
            Box::pin(async move {
                let gateway = resolve::<G>(&module).await?;
                <G as Gateway>::after_init(&gateway, gw).await;
                Ok(())
            })
        });
        ConnectHandler {
            settings: G::settings(),
            session: G::session(),
            admit,
            messages: Messages::Raw { instance, on_message },
            disconnect: Some(disconnect),
            after_init: Some(after_init),
        }
    }
}

/// The gateway `G` for a lifecycle call: resolved in an execution of its module that ends before
/// the call runs, so a hook that runs for a long time holds no execution open across the drain.
async fn resolve<G: Send + Sync + 'static>(module: &ModuleRef) -> Result<Arc<G>, BoxError> {
    let exec = Execution::open(module, ExecOptions::new())?;
    let gateway = exec.get::<G>().await?.into_arc();
    Ok(gateway)
}

/// Mounts `G`'s connect handler, carrying `handler`, with `G`'s connect guards and the session
/// factory's reads as the handler's dependencies.
pub(crate) fn mount<G: GatewayConfig>(m: &mut Mount<'_>, handler: ConnectHandler) {
    let mut guards = EnhancerSpec::<WsConnect>::new();
    G::connect_guards(&mut guards);
    let route = handler.settings.path.clone();
    let dependencies = handler.session.dependencies();
    m.handler(
        ulo::HandlerSpec::<WsConnect, Arc<ConnectHandler>>::new("connect", Arc::new(handler))
            .controller(guards)
            .route(route)
            .dependencies(dependencies),
    );
}

/// 64 MiB, tungstenite's own message ceiling.
const DEFAULT_MESSAGE_LIMIT: u64 = 64 * 1024 * 1024;
/// tungstenite's frame ceiling, lowered to the message limit when that is smaller.
const FRAME_LIMIT: u64 = 16 * 1024 * 1024;
const DEFAULT_MAX_OUTBOUND: usize = 1024;
const DEFAULT_KEEPALIVE: Duration = Duration::from_secs(30);

/// A gateway's limits with the server's or the module's defaults applied.
#[derive(Clone, Debug)]
pub(crate) struct Limits {
    pub(crate) message_limit: usize,
    pub(crate) frame_limit: usize,
    /// `None`: unbounded.
    pub(crate) max_connections: Option<usize>,
    pub(crate) max_inflight: usize,
    /// `None`: unbounded.
    pub(crate) max_outbound: Option<usize>,
    /// `None`: no keep-alive Ping.
    pub(crate) ping_interval: Option<Duration>,
    /// `None`: a Pong may take any time, and a server-initiated close waits for the client's
    /// Close until the server closes.
    pub(crate) pong_timeout: Option<Duration>,
}

impl Limits {
    fn resolve(settings: &GatewaySettings, defaults: &GatewayDefaults) -> Self {
        let message_limit = settings.message_limit.or(defaults.message_limit).unwrap_or(DEFAULT_MESSAGE_LIMIT);
        Limits {
            message_limit: usize::try_from(message_limit).unwrap_or(usize::MAX),
            frame_limit: usize::try_from(message_limit.min(FRAME_LIMIT)).unwrap_or(usize::MAX),
            max_connections: count(settings.max_connections, defaults.max_connections, None),
            max_inflight: count(settings.max_inflight, defaults.max_inflight, Count::Default.max_inflight()).unwrap_or(usize::MAX),
            max_outbound: count(settings.max_outbound, defaults.max_outbound, Some(DEFAULT_MAX_OUTBOUND)),
            ping_interval: bound(settings.ping_interval, defaults.ping_interval),
            pong_timeout: bound(settings.pong_timeout, defaults.pong_timeout),
        }
    }
}

fn count(declared: Count, default: Count, builtin: Option<usize>) -> Option<usize> {
    let count = if declared == Count::Default { default } else { declared };
    match count {
        Count::Default => builtin,
        Count::Max(n) => Some(usize::try_from(n).unwrap_or(usize::MAX)),
        Count::Unlimited => None,
    }
}

fn bound(declared: Bound, default: Bound) -> Option<Duration> {
    let bound = if declared == Bound::Default { default } else { declared };
    match bound {
        Bound::Default => Some(DEFAULT_KEEPALIVE),
        Bound::After(after) => Some(after),
        Bound::Unbounded => None,
    }
}

/// One message handler of a gateway.
pub(crate) struct Event {
    pub(crate) mounted: MountedHandler<Ws>,
    pub(crate) call: HandlerFn,
}

/// A gateway as a server serves it: its path with the controller's prefix, its settings and
/// limits, its connect handler and its message handlers by event.
pub(crate) struct GatewayRuntime {
    pub(crate) path: Arc<str>,
    pub(crate) namespace: Option<Arc<str>>,
    pub(crate) event_field: Arc<str>,
    pub(crate) controller: TypeName,
    pub(crate) limits: Limits,
    pub(crate) handler: Arc<ConnectHandler>,
    pub(crate) connect: MountedHandler<WsConnect>,
    pub(crate) events: HashMap<String, Event>,
    /// Connections holding a slot under `max_connections`.
    pub(crate) live: AtomicUsize,
}

impl GatewayRuntime {
    pub(crate) fn settings(&self) -> &GatewaySettings {
        &self.handler.settings
    }

    pub(crate) fn reference(&self) -> GatewayRef {
        GatewayRef {
            path: Cow::Owned(self.path.to_string()),
            namespace: self.namespace.as_deref().map(|namespace| Cow::Owned(namespace.to_owned())),
        }
    }
}

/// The gateways served on `port`: each connect handler mounted under `WsConnect` whose settings
/// name `port`, paired by controller with the message handlers mounted under `Ws`. Refused, each
/// as one failure: a path not starting with `/`, two gateways on one path, the event field `id`
/// or `data`, an event spelling the reserved `cancel`, two handlers for one event, message
/// handlers on a hand-written gateway, and every zero limit.
pub(crate) fn build_table(
    connects: &[MountedHandler<WsConnect>],
    messages: &[MountedHandler<Ws>],
    port: Port,
    defaults: &GatewayDefaults,
    failures: &mut Failures,
) -> Vec<Arc<GatewayRuntime>> {
    let mut table: Vec<Arc<GatewayRuntime>> = Vec::new();
    for connect in connects {
        let Some(handler) = connect.handler::<Arc<ConnectHandler>>() else { continue };
        let handler = Arc::clone(handler);
        let settings = &handler.settings;
        if settings.port != port {
            continue;
        }
        let controller = connect.controller().key().type_name();
        let path = join_path(connect.info().prefix(), &settings.path);
        let mut fail = |text: String| failures.push(gateway_failure(controller, path.clone(), text));
        if !path.starts_with('/') {
            fail("a gateway path starts with `/`".to_owned());
        }
        if path.contains(['{', '}']) {
            fail("a gateway path is matched as written and takes no `{param}` segment".to_owned());
        }
        if matches!(&*settings.event, "id" | "data") {
            fail(format!("the event field `{}` is a key the envelope reserves", settings.event));
        }
        for text in zero_settings(settings) {
            fail(text);
        }
        let mut events: HashMap<String, Event> = HashMap::new();
        for mounted in messages.iter().filter(|mounted| mounted.controller() == connect.controller()) {
            let Some(ws) = mounted.handler::<WsHandler>() else { continue };
            if matches!(handler.messages, Messages::Raw { .. }) {
                fail(format!(
                    "it implements `ulo_ws::Gateway` by hand, which reads every message raw, so its `#[message(\"{}\")]` \
                     handler would never run",
                    ws.event
                ));
                continue;
            }
            if ws.event == "cancel" {
                fail("`cancel` is the envelope's reserved event, and a handler spells it".to_owned());
                continue;
            }
            if events.contains_key(ws.event) {
                fail(format!("two handlers answer the event `{}`", ws.event));
                continue;
            }
            events.insert(ws.event.to_owned(), Event { mounted: mounted.clone(), call: Arc::clone(&ws.call) });
        }
        if let Some(taken) = table.iter().find(|taken| same_path(&taken.path, &path)) {
            let first = taken.controller;
            let shown = path.clone();
            failures.push(Failure::naming(vec![first, controller], move |names: &Names<'_>| {
                format!("two gateways on the path `{shown}`: `{}` and `{}`", names.of(first), names.of(controller))
            }));
            continue;
        }
        table.push(Arc::new(GatewayRuntime {
            path: Arc::from(path.as_str()),
            namespace: settings.namespace.as_deref().map(Arc::from),
            event_field: Arc::from(&*settings.event),
            controller,
            limits: Limits::resolve(settings, defaults),
            handler: Arc::clone(&handler),
            connect: connect.clone(),
            events,
            live: AtomicUsize::new(0),
        }));
    }
    table
}

/// The zero refusals of a server's or a module's defaults, `owner` naming which.
pub(crate) fn check_defaults(owner: &str, defaults: &GatewayDefaults, failures: &mut Failures) {
    let settings = GatewaySettings {
        message_limit: defaults.message_limit,
        max_connections: defaults.max_connections,
        max_inflight: defaults.max_inflight,
        max_outbound: defaults.max_outbound,
        ping_interval: defaults.ping_interval,
        pong_timeout: defaults.pong_timeout,
        ..GatewaySettings::at("/")
    };
    for text in zero_settings(&settings) {
        failures.push(format!("{owner}: {text}"));
    }
}

/// Each limit in `settings` that would refuse everything it governs, as text.
fn zero_settings(settings: &GatewaySettings) -> Vec<String> {
    let none = HashSet::new();
    let names = Names::new(&none);
    let mut texts: Vec<String> = [
        zero_count("max_connections", settings.max_connections, "close every connection with 1013 as it opens"),
        zero_count("max_inflight", settings.max_inflight, "stop reading every connection before its first message"),
        zero_count("max_outbound", settings.max_outbound, "overflow every connection at its first outbound message"),
        zero_bound("ping_interval", settings.ping_interval, "ping every connection without pause"),
        zero_bound("pong_timeout", settings.pong_timeout, "end every connection at its first Ping"),
    ]
    .into_iter()
    .flatten()
    .map(|failure| failure.text(&names))
    .collect();
    if settings.message_limit == Some(0) {
        texts.push(
            "`.message_limit(0)` would close every connection with 1009 at its first message; leave it unset for the 64 MiB \
             default"
                .to_owned(),
        );
    }
    texts
}

fn gateway_failure(controller: TypeName, path: String, text: String) -> Failure {
    Failure::naming(vec![controller], move |names: &Names<'_>| format!("gateway `{}` at `{path}`: {text}", names.of(controller)))
}

/// `path` under the controller's `.at(prefix)`, joined as `ulo_http` joins a route to its prefix:
/// one `/` between them and the prefix's trailing slash dropped, so `/graphql` and `/` give
/// `/graphql`, and `/api` and `/chat` give `/api/chat`.
pub(crate) fn join_path(prefix: Option<&str>, path: &str) -> String {
    let Some(prefix) = prefix else { return path.to_owned() };
    let prefix = prefix.trim_end_matches('/');
    let route = path.trim_start_matches('/');
    let mut joined = String::with_capacity(prefix.len() + route.len() + 2);
    if !prefix.starts_with('/') {
        joined.push('/');
    }
    joined.push_str(prefix);
    if !route.is_empty() {
        if !joined.ends_with('/') {
            joined.push('/');
        }
        joined.push_str(route);
    }
    joined
}

/// Whether two paths take the same requests: one trailing slash is insignificant.
pub(crate) fn same_path(a: &str, b: &str) -> bool {
    normalized(a) == normalized(b)
}

pub(crate) fn normalized(path: &str) -> &str {
    match path.strip_suffix('/') {
        Some(stripped) if !stripped.is_empty() => stripped,
        _ => path,
    }
}
