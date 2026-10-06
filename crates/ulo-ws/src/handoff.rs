//! The gateway hand-off on the HTTP server's port and on an embedding host declaring `upgrades`
//! (transports DESIGN §3.5, X21): the `ulo_http::UpgradeHandler` `WsModule` registers, which finds
//! its gateways through `AppHandle::mounted::<Ws>()` and `::<WsConnect>()` (X20).

use std::borrow::Cow;
use std::sync::{Arc, Mutex, PoisonError};

use http::StatusCode;
use http::header::{CONTENT_TYPE, HeaderValue};
use ulo::{AppHandle, BoxError, BoxFuture, DrainToken, Timer};
use ulo_http::{HttpBody, Request, Response, UpgradeHandler};
use ulo_transport::prepare::Failures;

use crate::connection::{Accept, Answer, Tracker, answer};
use crate::gateway::{GatewayRuntime, Port, build_table, check_defaults, same_path};
use crate::module::Defaults;
use crate::rooms::Hub;
use crate::transport::{Ws, WsConnect};

/// The hand-off itself, carrying `WsModule`'s defaults for the gateways it serves: those whose
/// settings name `Port::Http`, the default.
pub(crate) struct Handoff {
    pub(crate) defaults: Defaults,
    hub: Arc<Hub>,
    state: Arc<Mutex<State>>,
    tracker: Arc<Tracker>,
}

/// What `prepare` built and the first upgrade resolved.
#[derive(Default)]
struct State {
    gateways: Vec<Arc<GatewayRuntime>>,
    app: Option<AppHandle>,
    /// The app's `Timer`, bound as `dyn Timer`, resolved at the first upgrade since `prepare` and
    /// `bound` cannot await.
    timer: Option<Arc<dyn Timer>>,
}

impl Handoff {
    pub(crate) fn new(defaults: Defaults, hub: Arc<Hub>) -> Self {
        Handoff { defaults, hub, state: Arc::new(Mutex::new(State::default())), tracker: Tracker::new() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl UpgradeHandler for Handoff {
    fn paths(&self, app: &AppHandle) -> Vec<Cow<'static, str>> {
        let _ = app;
        self.lock().gateways.iter().map(|gateway| Cow::Owned(gateway.path.to_string())).collect()
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let state = Arc::clone(&self.state);
        let hub = Arc::clone(&self.hub);
        let tracker = Arc::clone(&self.tracker);
        Box::pin(async move {
            let (gateway, app, timer) = {
                let state = state.lock().unwrap_or_else(PoisonError::into_inner);
                let gateway = state.gateways.iter().find(|gateway| same_path(&gateway.path, req.path())).cloned();
                (gateway, state.app.clone(), state.timer.clone())
            };
            let (Some(gateway), Some(app)) = (gateway, app) else {
                return plain(StatusCode::NOT_FOUND, "no gateway at this path");
            };
            let timer = match timer {
                Some(timer) => timer,
                None => match app.get::<dyn Timer>().await {
                    Ok(timer) => {
                        let timer = timer.into_arc();
                        state.lock().unwrap_or_else(PoisonError::into_inner).timer = Some(Arc::clone(&timer));
                        timer
                    }
                    Err(error) => {
                        tracing::error!(%error, "the WebSocket hand-off found no `Timer`");
                        return plain(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
                    }
                },
            };
            let Request { head, conn, upgrade, .. } = req;
            let Some(upgrade) = upgrade else {
                return plain(StatusCode::BAD_REQUEST, "this request cannot be upgraded to a WebSocket");
            };
            let accept = Accept { gateway, hub, app, timer, tracker };
            let upgraded = async move { upgrade.await };
            respond(answer(accept, head, conn.peer, upgraded).await)
        })
    }

    /// The gateways on the HTTP server's port, built and checked; `WsModule`'s own defaults are
    /// checked for zero limits too.
    fn prepare(&self, app: &AppHandle) -> Result<(), BoxError> {
        let connects = app.mounted::<WsConnect>()?;
        let messages = app.mounted::<Ws>()?;
        let mut failures = Failures::new();
        check_defaults("WsModule", &self.defaults, &mut failures);
        let gateways = build_table(&connects, &messages, Port::Http, &self.defaults, &mut failures);
        self.hub.record_gateways(&gateways);
        let mut state = self.lock();
        state.gateways = gateways;
        state.app = Some(app.clone());
        drop(state);
        failures.into_result()
    }

    /// Starts broadcast delivery and each gateway's `AfterInit` on its own task.
    fn bound(&self, app: &AppHandle) {
        let _ = app;
        self.hub.start();
        let gateways = self.lock().gateways.clone();
        after_init(&gateways);
    }

    fn drain(&self, token: DrainToken) -> BoxFuture<'_, ()> {
        Box::pin(self.tracker.drain(token))
    }

    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.tracker.close().await;
            self.hub.stop();
        })
    }
}

/// Each gateway's `AfterInit`, spawned: the server does not wait for it. A failure is logged.
pub(crate) fn after_init(gateways: &[Arc<GatewayRuntime>]) {
    for gateway in gateways {
        let Some(hook) = gateway.handler.after_init.clone() else { continue };
        let module = gateway.connect.module().clone();
        let reference = gateway.reference();
        let path = Arc::clone(&gateway.path);
        tokio::spawn(async move {
            if let Err(error) = hook(module, reference).await {
                tracing::warn!(%error, gateway = %path, "after_init failed");
            }
        });
    }
}

fn respond(answer: Answer) -> Response {
    match answer {
        Answer::Switch(headers) => {
            let mut response = Response::new(HttpBody::empty());
            *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
            response.headers_mut().extend(headers);
            response
        }
        Answer::Refuse { status, reason, headers } => {
            let mut response = plain(status, reason);
            for (name, value) in headers {
                response.headers_mut().insert(name, value);
            }
            response
        }
    }
}

fn plain(status: StatusCode, text: impl Into<String>) -> Response {
    let mut response = Response::new(HttpBody::from_bytes(text.into()));
    *response.status_mut() = status;
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}
