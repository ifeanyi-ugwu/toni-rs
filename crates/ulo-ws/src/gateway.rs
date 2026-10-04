//! Gateways and their connection hooks (transports DESIGN §4.1, §4.2).
//!
//! `#[ulo_ws::gateway(..)]` on a `#[routes]` impl writes [`GatewayConfig`]; its `#[message]`
//! handlers are `Ws` handlers routed by the envelope's event. A gateway whose protocol does not fit
//! the envelope, graphql-transport-ws for one, implements [`Gateway`] by hand: it receives each
//! data message raw and answers through its [`Connection`].

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::future::Future;

use ulo::{Bound, Construct, EnhancerSpec, Mount};
use ulo_transport::{Count, ErrorKind};

use crate::codec::Codec;
use crate::connection::Connection;
use crate::envelope::Frame;
use crate::session::SessionFactory;
use crate::transport::WsConnect;

/// A gateway's configuration, which `#[ulo_ws::gateway(..)]` writes on the impl it sits on and a
/// hand-written [`Gateway`] implements: where it listens, how its messages are framed, its
/// connect guards, its session, and the mount of its connect handler.
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
        let _ = m;
        todo!()
    }
}

/// What a gateway attribute declares. A limit left at `Count::Default` or `Bound::Default`, and a
/// `message_limit` left `None`, takes the default of `ulo_ws::Server` for a gateway on its own
/// port and of `WsModule` for one on the HTTP server's port.
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
/// `bound` (X21). A lifecycle call, not an execution.
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
    /// The client sent a Close frame.
    ClientClose { code: u16, reason: String },
    /// The gateway closed the connection.
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

impl ConnectRefused {
    /// A refusal with the close code `code` and `reason`. Refused: a reason over 123 bytes of
    /// UTF-8 (RFC 6455 §5.5.1), 1004, 1005, 1006 and 1015, which are never sent in a frame, and a
    /// code outside 1000–4999.
    pub fn code(code: u16, reason: impl Into<String>) -> Result<Self, CloseCodeError> {
        let _ = (code, reason);
        todo!()
    }

    /// A refusal of kind `kind`, closing with the code the kind maps to; under `refuse =
    /// handshake`, answered 401 for `Unauthorized` and 403 otherwise.
    pub fn kind(kind: ErrorKind, reason: impl Into<String>) -> Self {
        let _ = (kind, reason);
        todo!()
    }

    pub fn close_code(&self) -> u16 {
        self.close_code
    }

    pub fn reason(&self) -> &str {
        &self.reason
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
