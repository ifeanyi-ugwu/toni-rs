//! The gateway hand-off on the HTTP server's port and on an embedding host declaring `upgrades`
//! (transports DESIGN §3.5, X21): the `ulo_http::UpgradeHandler` `WsModule` registers, which finds
//! its gateways through `AppHandle::mounted::<Ws>()` and `::<WsConnect>()` (X20).
//!
//! On the HTTP port, the HTTP server's drain and load shedding answer first. A gateway there is
//! the HTTP server's guest: an upgrade request arriving once the drain has begun, or over the
//! server's `max_inflight`, is answered with the HTTP server's own 503 before the hand-off sees
//! it, so the table's plain-text 503 for a handshake during the drain is a standalone server's.

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use http::StatusCode;
use ulo::{AppHandle, BoxError, BoxFuture, DrainToken, Spawn};
use ulo_http::{HttpBody, Request, Response, UpgradeHandler};
use ulo_transport::prepare::Failures;

use crate::connection::{Handshake, Refusal};
use crate::gateway::{GatewayRuntime, Port, build_table, check_defaults};
use crate::rooms::Hub;
use crate::table::{GatewayDefaults, GatewayTable};
use crate::transport::{Ws, WsConnect};

/// The hand-off itself, carrying `WsModule`'s defaults for the gateways it serves: those whose
/// settings name `Port::Http`, the default.
pub(crate) struct Handoff {
    pub(crate) defaults: GatewayDefaults,
    hub: Arc<Hub>,
    /// What `prepare` built: the gateways on the HTTP server's port, served through the same
    /// table a standalone server serves through.
    table: Arc<Mutex<Option<GatewayTable>>>,
}

impl Handoff {
    pub(crate) fn new(defaults: GatewayDefaults, hub: Arc<Hub>) -> Self {
        Handoff { defaults, hub, table: Arc::new(Mutex::new(None)) }
    }

    fn table(&self) -> Option<GatewayTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl UpgradeHandler for Handoff {
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>> {
        let _ = app;
        self.table().map(|table| table.paths().map(|path| Cow::Owned(path.to_owned())).collect()).unwrap_or_default()
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let table = self.table();
        Box::pin(async move {
            let Some(table) = table.filter(|table| table.gateway_for(req.path()).is_some()) else {
                return plain(StatusCode::NOT_FOUND, "no gateway at this path");
            };
            let Request { head, conn, upgrade, .. } = req;
            let Some(upgrade) = upgrade else {
                return plain(StatusCode::BAD_REQUEST, "this request cannot be upgraded to a WebSocket");
            };
            match table.handshake(head, conn.peer).await {
                Handshake::Switch(switch) => {
                    let response = switch.response().map(|()| HttpBody::empty());
                    switch.serve(upgrade);
                    response
                }
                Handshake::Refuse(refusal) => refusal.into_response().map(HttpBody::from_bytes),
            }
        })
    }

    /// The gateways on the HTTP server's port, built and checked; `WsModule`'s own defaults are
    /// checked for zero limits too, and the broadcast adapter's `prepare` runs beside them.
    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> {
        let connects = app.mounted::<WsConnect>()?;
        let messages = app.mounted::<Ws>()?;
        // `mounted` refuses an app with no runtime, so this finds one.
        let runtime = app.runtime().cloned().ok_or("the WebSocket hand-off was prepared on an app with no runtime")?;
        let mut failures = Failures::new();
        check_defaults("WsModule", &self.defaults, &mut failures);
        if let Err(error) = self.hub.prepare() {
            failures.push_error(error);
        }
        let gateways = build_table(&connects, &messages, Port::Http, &self.defaults, &mut failures);
        let table = GatewayTable::new(gateways, Arc::clone(&self.hub), app.clone(), runtime, &self.defaults);
        *self.table.lock().unwrap_or_else(PoisonError::into_inner) = Some(table);
        failures.into_result()
    }

    /// Starts broadcast delivery and each gateway's `AfterInit` on its own task.
    fn bound(&self, app: &AppHandle) {
        let _ = app;
        if let Some(table) = self.table() {
            table.start();
        }
    }

    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> {
        let table = self.table();
        Box::pin(async move {
            if let Some(table) = table {
                table.drain(token).await;
            }
        })
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        let table = self.table();
        Box::pin(async move {
            match table {
                Some(table) => table.close().await,
                None => self.hub.stop(),
            }
        })
    }
}

/// Each gateway's `AfterInit`, spawned on `runtime` and detached: the server does not wait for
/// it. A failure is logged.
pub(crate) fn after_init(gateways: &[Arc<GatewayRuntime>], runtime: &dyn Spawn) {
    for gateway in gateways {
        let Some(hook) = gateway.handler.after_init.clone() else { continue };
        let module = gateway.connect.module().clone();
        let reference = gateway.reference();
        let path = Arc::clone(&gateway.path);
        drop(runtime.spawn(Box::pin(async move {
            if let Err(error) = hook(module, reference).await {
                tracing::warn!(%error, gateway = %path, "after_init failed");
            }
        })));
    }
}

/// A refusal written as the handshake's are.
fn plain(status: StatusCode, reason: &str) -> Response {
    Refusal::new(status, reason).into_response().map(HttpBody::from_bytes)
}
