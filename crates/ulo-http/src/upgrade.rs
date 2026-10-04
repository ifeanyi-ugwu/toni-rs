//! The WebSocket upgrade hand-off point (transports DESIGN §3.5).
//!
//! When a request carrying `Upgrade` matches a path an [`UpgradeHandler`] takes, the router hands
//! the request over before route matching's 404 or 405; the handler answers the handshake and
//! completes it on the backend's upgrade future. `ulo-ws` registers its gateways here; a
//! backend whose limits declare `upgrades: false` cannot take them, and `prepare` refuses the
//! combination naming the limit.

use std::borrow::Cow;
use std::sync::Arc;

use ulo::{AppHandle, BoxError, BoxFuture, DrainToken, Meta};

use crate::request::Request;
use crate::response::Response;

/// A transport that takes upgrade requests on the HTTP server's port.
///
/// An upgraded connection has left the backend, so the backend's drain and close never reach it;
/// the handler holds those connections and carries the server's lifecycle for them (X21).
/// `Server<B>` and `Embedded<A>` call [`prepare`](Self::prepare) inside their own `prepare`,
/// [`bound`](Self::bound) once bound, and [`drain`](Self::drain) and [`close`](Self::close)
/// concurrently with the backend's or the host's, so a gateway on the HTTP server's port and one
/// on its own port run the same steps at the same moments.
pub trait UpgradeHandler: Send + Sync + 'static {
    /// The paths it takes, as route patterns, read once when the server prepares; `app` lists the
    /// mounted handlers, a gateway's among them.
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>>;

    /// Answers one upgrade request: the 101 response, after which the handler awaits
    /// `req.upgrade` for the connection's I/O, or a refusal.
    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response>;

    /// Builds what can fail without a connection: a gateway's handlers through
    /// `AppHandle::mounted`, duplicate gateway paths, a gateway's own zero limits. Its error joins
    /// the HTTP server's `prepare` failure; a `PrepareError` keeps its names in the report.
    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        Ok(())
    }

    /// Called once the HTTP server has bound, where a gateway's `AfterInit` runs. Work that waits
    /// is the handler's to spawn: the server does not wait for it.
    fn bound(&self, app: &AppHandle) {
        let _ = app;
    }

    /// The drain: idle connections closed with 1001 at once, busy ones stop reading, finish their
    /// messages, then close with 1001. A connection's cleanup opens with
    /// `Execution::open_terminal(&token, ..)`.
    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> {
        let _ = token;
        Box::pin(async {})
    }

    /// Closes every connection left.
    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// Module metadata through which a transport registers its upgrade handler:
/// `m.meta::<ulo_http::Upgrades>().register(GatewayHandoff::new())`.
#[derive(Default)]
pub struct Upgrades {
    pub(crate) handlers: Vec<Arc<dyn UpgradeHandler>>,
}

impl Upgrades {
    pub fn register(&mut self, handler: impl UpgradeHandler) -> &mut Self {
        self.handlers.push(Arc::new(handler));
        self
    }
}

impl Meta for Upgrades {}
