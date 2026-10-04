//! The gateway hand-off on the HTTP server's port and on an embedding host declaring `upgrades`
//! (transports DESIGN §3.5, X21): the `ulo_http::UpgradeHandler` `WsModule` registers, which finds
//! its gateways through `AppHandle::mounted::<Ws>()` and `::<WsConnect>()` (X20).

use std::borrow::Cow;

use ulo::{AppHandle, BoxError, BoxFuture, DrainToken};
use ulo_http::{Request, Response, UpgradeHandler};

use crate::module::Defaults;

/// The hand-off itself, carrying `WsModule`'s defaults for the gateways it serves.
pub(crate) struct Handoff {
    pub(crate) defaults: Defaults,
}

impl UpgradeHandler for Handoff {
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>> {
        let _ = (app, &self.defaults);
        todo!()
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let _ = req;
        todo!()
    }

    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        todo!()
    }

    fn bound(&self, app: &AppHandle) {
        let _ = app;
        todo!()
    }

    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> {
        let _ = token;
        todo!()
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        todo!()
    }
}
