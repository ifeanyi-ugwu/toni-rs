//! The backend SPI, written without macros (transports DESIGN §3.7). The `http` crate's types are
//! the common language; actix and rocket convert at their edge. A backend crate is conversion
//! plus the drain mapping.

use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;

use ulo::BoxError;
use ulo_net::{BoundListener, TlsAcceptor};

use crate::limits::MB;
use crate::service::AppService;

/// One HTTP server implementation. `ulo_http::Server<B>` implements the core's `Server` over it:
/// it prepares the route table, parses endpoints, loads TLS and checks [`limits`](Self::limits)
/// before handing the backend bound listeners and the [`AppService`] it calls per request.
pub trait Backend: Send + Sync + 'static {
    /// The backend's name, as a `Configure` error naming a limit prints it.
    const NAME: &'static str;

    /// What it cannot do, checked in `prepare`: asking for it is a `StartupError::Configure`
    /// naming the limit.
    fn limits() -> BackendLimits;

    /// Takes the listeners the server bound and starts accepting on them, without serving yet.
    /// All-or-nothing: an error closes whatever this call opened.
    fn bind(
        &mut self,
        listeners: Vec<BoundListener>,
        tls: Option<TlsAcceptor>,
        svc: AppService,
        cfg: &HttpConfig,
    ) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Serves until drained, calling `svc` for every request. A response body dropped before its
    /// end is how a peer's reset or a closed connection reaches the execution, as
    /// `CancelReason::Disconnected`, so the backend drops it as soon as it observes either. An
    /// unread request body dropped is not a disconnect.
    fn serve(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Stops accepting: HTTP/2 GOAWAY, idle HTTP/1.1 keep-alive connections closed, busy ones
    /// marked `Connection: close` on their next response.
    fn drain(&self) -> impl Future<Output = ()> + Send;

    /// Closes every listener and connection left.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

/// What a backend cannot do. The starting point per backend is documented beside each, and its
/// conformance tests confirm each entry.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendLimits {
    /// Hands over a per-request upgraded I/O; `false` refuses a gateway on its port.
    pub upgrades: bool,
    /// HTTP/2 without TLS.
    pub h2c: bool,
    /// Adopts an inherited socket.
    pub inherited_sockets: bool,
    /// Binds port 0 and reports the port chosen.
    pub port_zero: bool,
    pub tls: bool,
}

impl BackendLimits {
    /// A backend that can do everything.
    pub const NONE: BackendLimits = BackendLimits { upgrades: true, h2c: true, inherited_sockets: true, port_zero: true, tls: true };

    pub const fn upgrades(self, supported: bool) -> Self {
        BackendLimits { upgrades: supported, ..self }
    }

    pub const fn h2c(self, supported: bool) -> Self {
        BackendLimits { h2c: supported, ..self }
    }

    pub const fn inherited_sockets(self, supported: bool) -> Self {
        BackendLimits { inherited_sockets: supported, ..self }
    }

    pub const fn port_zero(self, supported: bool) -> Self {
        BackendLimits { port_zero: supported, ..self }
    }

    pub const fn tls(self, supported: bool) -> Self {
        BackendLimits { tls: supported, ..self }
    }
}

/// The server's settings a backend reads, from `ulo_http::Server`'s builder.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// The body limit a route without `#[meta(BodyLimit(..))]` takes: 2 MiB unset.
    pub body_limit: u64,
    /// The server's in-flight bound; over it a request is answered 503 with `Retry-After`.
    pub max_inflight: Option<usize>,
    /// HTTP/2 `SETTINGS_MAX_CONCURRENT_STREAMS`: excess streams are refused with
    /// `RST_STREAM(REFUSED_STREAM)` by the backend.
    pub max_concurrent_streams: Option<u32>,
    /// `Retry-After` on a load-shedding refusal and on a request refused during the drain: one
    /// second unset.
    pub shed_retry_after: Duration,
    /// The `WWW-Authenticate` challenge a 401 carries when its error names none: `Bearer` unset.
    pub challenge: Cow<'static, str>,
    /// Accept HTTP/2 without TLS (prior knowledge).
    pub h2c: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            body_limit: 2 * MB,
            max_inflight: None,
            max_concurrent_streams: None,
            shed_retry_after: Duration::from_secs(1),
            challenge: Cow::Borrowed("Bearer"),
            h2c: false,
        }
    }
}
