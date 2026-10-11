//! The application every scenario runs: one controller, a global error handler, a CORS entry, the
//! echo upgrade handler where the host takes upgrades, and the probe the disconnect scenario reads.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::stream::{self, BoxStream};
use futures_util::{AsyncReadExt, AsyncWriteExt, StreamExt};
use serde::{Deserialize, Serialize};
use ulo::{
    AnyErrorHandler, BoxError, BoxFuture, CancelReason, Dep, ErrorHandler, Module, ModuleDef, ModuleIdentity, Runtime,
    StreamOutcome, Timer, injectable, routes,
};
use ulo_http::embed::EmbedLimits;
use ulo_http::{
    BodyLimit, Cors, Event, Host, Http, HttpBody, HttpCx, Json, PreDispatch, Query, Request, Response, Sse,
    StatusCode, UpgradeHandler, Upgrades,
};
use ulo_transport::{CallError, Classify, ErrorKind, IntoReply, Valid, Validate};

/// The origin the CORS entry admits.
pub const ORIGIN: &str = "https://suite.example";

/// The value a host puts in its own request store from the `x-host-value` header, read back by
/// `GET /host-value` as `Host<HostValue>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostValue(pub String);

/// The header a host copies into [`HostValue`].
pub const HOST_VALUE_HEADER: &str = "x-host-value";

/// How long `GET /endless` stays idle after its first event, before it writes every 100 ms.
pub(crate) const IDLE: Duration = Duration::from_millis(600);

pub(crate) struct SuiteModule {
    pub(crate) limits: EmbedLimits,
}

impl Module for SuiteModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Probe::default());
        m.controller::<Suite>();
        let reshape: Arc<AnyErrorHandler<Http>> = Arc::new(Reshape);
        m.enhancer::<AnyErrorHandler<Http>>().value(reshape);
        m.meta::<PreDispatch>().apply_value(Cors::new().allow_origin(ORIGIN));
        // A host declaring `upgrades: false` refuses any upgrade handler in `prepare`.
        if self.limits.upgrades {
            m.meta::<Upgrades>().register(Echo::default());
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Page {
    pub page: u32,
}

#[derive(Debug, Deserialize, Serialize, Validate)]
pub struct Item {
    #[validate(length(min = 1, max = 8))]
    pub name: String,
}

/// The error `GET /fail` and the SSE stream's second item return, classified `conflict` and
/// reshaped to `forbidden` by [`Reshape`].
#[derive(Debug, Classify)]
#[classify(conflict)]
pub struct SuiteError;

impl fmt::Display for SuiteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the suite refused")
    }
}

impl Error for SuiteError {}

/// The error `GET /recover` returns, classified `not_found`, which [`Substitute`] answers with a
/// value of its own.
#[derive(Debug, Classify)]
#[classify(not_found)]
pub struct Shortage;

impl fmt::Display for Shortage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the suite has none left")
    }
}

impl Error for Shortage {}

/// The item [`Substitute`] answers a [`Shortage`] with.
pub(crate) const SPARE: &str = "spare";

/// `GET /recover`'s error handler: a [`Shortage`] is answered with the JSON item named [`SPARE`],
/// anything else passes on.
pub struct Substitute;

impl ErrorHandler<Http> for Substitute {
    async fn handle(&self, err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        let short = err.downcast_ref::<Shortage>().is_some()
            || err.downcast_ref::<CallError>().is_some_and(|call| call.source_as::<Shortage>().is_some());
        if !short {
            return Err(err);
        }
        Ok(Json(Item { name: SPARE.to_owned() }).into_reply(cx)?)
    }
}

#[injectable]
pub struct Suite {
    timer: Dep<dyn Timer>,
}

#[routes]
impl Suite {
    #[ulo_http::get("/hit")]
    fn hit(&self) -> &'static str {
        "hit"
    }

    #[ulo_http::get("/page")]
    fn page(&self, page: Query<Page>) -> String {
        page.0.page.to_string()
    }

    #[ulo_http::post("/json")]
    fn json(&self, item: Json<Item>) -> Json<Item> {
        item
    }

    #[ulo_http::post("/small")]
    #[meta(BodyLimit(16))]
    fn small(&self, item: Json<Item>) -> Json<Item> {
        item
    }

    #[ulo_http::post("/valid")]
    fn valid(&self, item: Valid<Json<Item>>) -> Json<Item> {
        item.into_inner()
    }

    #[ulo_http::get("/fail")]
    fn fail(&self) -> Result<String, SuiteError> {
        Err(SuiteError)
    }

    #[ulo_http::get("/recover")]
    #[error_handlers(value = Substitute)]
    fn recover(&self) -> Result<String, Shortage> {
        Err(Shortage)
    }

    #[ulo_http::get("/sse")]
    fn sse(&self) -> Sse<stream::Iter<std::vec::IntoIter<Result<Event, SuiteError>>>> {
        Sse::new(stream::iter(vec![Ok(Event::default().data("one")), Err(SuiteError)]))
    }

    #[ulo_http::get("/endless")]
    fn endless(&self, cx: HttpCx, probe: Dep<Probe>) -> Sse<BoxStream<'static, Event>> {
        let timer = self.timer.clone();
        let probe = Probe::clone(&probe);
        let exec = cx.exec().clone();
        cx.exec().on_stream_end(move |outcome| probe.record(outcome, exec.cancel_reason()));
        let first = stream::once(async { Event::default().data("start") });
        let idle = stream::once({
            let timer = timer.clone();
            async move {
                timer.sleep(IDLE).await;
                Event::default().data("awake")
            }
        });
        let ticks = stream::unfold(timer, |timer| async move {
            timer.sleep(Duration::from_millis(100)).await;
            Some((Event::default().data("tick"), timer))
        });
        Sse::new(first.chain(idle).chain(ticks).boxed())
    }

    #[ulo_http::get("/host-value")]
    fn host_value(&self, value: Host<HostValue>) -> String {
        value.0.0
    }
}

/// The global error handler: a [`SuiteError`] becomes a `forbidden` error, anything else passes on.
pub struct Reshape;

impl ErrorHandler<Http> for Reshape {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let ours = err.downcast_ref::<SuiteError>().is_some()
            || err.downcast_ref::<CallError>().is_some_and(|call| call.source_as::<SuiteError>().is_some());
        if ours {
            return Err(Box::new(CallError::new(ErrorKind::Forbidden, "reshaped by the suite")));
        }
        Err(err)
    }
}

/// What `GET /endless` recorded when its stream ended: the outcome and the execution's
/// cancellation reason read then.
#[derive(Clone, Default)]
pub struct Probe {
    ends: Arc<Mutex<Vec<(StreamOutcome, Option<CancelReason>)>>>,
}

impl Probe {
    fn record(&self, outcome: StreamOutcome, reason: Option<CancelReason>) {
        self.ends.lock().unwrap_or_else(PoisonError::into_inner).push((outcome, reason));
    }

    /// Whether a stream was cut off by a disconnect.
    pub(crate) fn disconnected(&self) -> bool {
        self.ends.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|(outcome, reason)| {
            matches!(outcome, StreamOutcome::CutOff(_))
                && (*outcome == StreamOutcome::CutOff(Some(CancelReason::Disconnected))
                    || *reason == Some(CancelReason::Disconnected))
        })
    }
}

/// The upgrade handler at `/echo`: answers 101 naming `websocket`, then writes back whatever the
/// client sends on the upgraded connection, on a task of the app's runtime. No WebSocket framing
/// is spoken; the scenario checks the hand-off of the connection, not the protocol.
#[derive(Default)]
struct Echo {
    /// The app's runtime, taken when the server prepares.
    runtime: OnceLock<Arc<dyn Runtime>>,
}

impl UpgradeHandler for Echo {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<std::borrow::Cow<'static, str>> {
        vec!["/echo".into()]
    }

    fn prepare(&self, app: &ulo::AppHandle) -> Result<(), BoxError> {
        let runtime = app.runtime().ok_or("the suite's app has no runtime for the echo's connections")?;
        let _ = self.runtime.set(Arc::clone(runtime));
        Ok(())
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let runtime = self.runtime.get().cloned();
        Box::pin(async move {
            let Some(pending) = req.upgrade else {
                let mut response = Response::new(HttpBody::from("the host handed over no upgrade"));
                *response.status_mut() = StatusCode::BAD_REQUEST;
                return response;
            };
            let Some(runtime) = runtime else {
                let mut response = Response::new(HttpBody::from("the echo was not prepared"));
                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                return response;
            };
            drop(runtime.spawn(Box::pin(async move {
                let Ok(mut io) = pending.await else { return };
                let mut buffer = [0u8; 64];
                while let Ok(read) = io.read(&mut buffer).await {
                    if read == 0 || io.write_all(&buffer[..read]).await.is_err() || io.flush().await.is_err() {
                        return;
                    }
                }
            })));
            let mut response = Response::new(HttpBody::empty());
            *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
            let headers = response.headers_mut();
            headers.insert(http::header::CONNECTION, http::HeaderValue::from_static("upgrade"));
            headers.insert(http::header::UPGRADE, http::HeaderValue::from_static("websocket"));
            response
        })
    }
}
