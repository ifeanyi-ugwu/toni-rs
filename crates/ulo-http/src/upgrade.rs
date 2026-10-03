//! The WebSocket upgrade hand-off point (transports DESIGN §3.5).
//!
//! When a request carrying `Upgrade` matches a path an [`UpgradeHandler`] takes, the router hands
//! the request over before route matching's 404 or 405; the handler answers the handshake and
//! completes it on the backend's upgrade future. `ulo-ws` registers its gateways here; a
//! backend whose limits declare `upgrades: false` cannot take them, and `prepare` refuses the
//! combination naming the limit.

use std::borrow::Cow;
use std::sync::Arc;

use ulo::{AppHandle, BoxFuture, Meta};

use crate::request::Request;
use crate::response::Response;

/// A transport that takes upgrade requests on the HTTP server's port.
pub trait UpgradeHandler: Send + Sync + 'static {
    /// The paths it takes, as route patterns, read once when the server prepares; `app` lists the
    /// mounted handlers, a gateway's among them.
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>>;

    /// Answers one upgrade request: the 101 response, after which the handler awaits
    /// `req.upgrade` for the connection's I/O, or a refusal.
    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response>;
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
