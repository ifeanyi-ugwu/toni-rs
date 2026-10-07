use std::any::Any;
use std::fmt;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{ALLOW, CONTENT_LENGTH, HeaderName, HeaderValue, UPGRADE};
use http::{HeaderMap, Method, StatusCode};
use http_body::{Body as _, Frame, SizeHint};
use tracing::{Instrument, Span, field};
use ulo::{
    AppHandle, BoxError, BoxFuture, CancelReason, DispatchStage, ExecOptions, Execution, ExecutionRef, MountedHandler,
    StreamOutcome, Timer, Transport,
};
use ulo_transport::{Admission, Permit, span};

use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::cx::{CxInner, HttpCx, MatchedRoute, PathParams};
use crate::embed::{Forwardable, OriginalPath};
use crate::miss::{MethodNotAllowed, NoRoute};
use crate::pre_dispatch::{self, Rest, Stage, StageCx};
use crate::render;
use crate::request::{ConnInfo, OnUpgrade, Request};
use crate::response::Response;
use crate::router::pattern::Pattern;
use crate::router::{RouteTarget, Routed, Router};
use crate::routing::Routing;
use crate::transport::{ClientAddr, Http, RequestHead};
use crate::upgrade::UpgradeHandler;

/// What a backend calls per request: load shedding, the pre-dispatch stage, routing, `dispatch`,
/// rendering (transports DESIGN §3.7). `Clone`, an `Arc` inside, so each connection task holds its
/// own.
///
/// Per request, in order:
/// 1. Over the server's in-flight bound: 503 with `Retry-After`.
/// 2. `Execution::open` at the root module. Refused during the drain: 503 with `Retry-After` and
///    `Connection: close`, answered directly, no pre-dispatch stage.
/// 3. The inputs `RequestHead` and `ClientAddr` seeded, as the client sent them; the
///    `ulo_transport::span::call` span entered, its `url.path` the request's
///    [`OriginalPath`](crate::embed::OriginalPath) when an embedding's adapter supplied one, and
///    the path received otherwise.
/// 4. The unscoped pre-dispatch entries, in order, inside `AppHandle::catch_panic`.
/// 5. An `Upgrade` request on an upgrade path goes to its `UpgradeHandler`. Otherwise routing: a
///    miss is offered to the global error handlers through `ulo::recover(None, ..)` as a
///    `CallError` whose source is [`NoRoute`] or [`MethodNotAllowed`], rendered 404, or 405 with
///    `Allow`, when none claims it; `OPTIONS` with no handler answers 204 with `Allow`.
/// 6. `Execution::route_to` the controller's module, the route's timeout armed on the app's
///    `Timer`, the scoped entries run. When the timeout passes before an answer, the pipeline is
///    dropped, the execution cancelled with `CancelReason::Deadline`, and the error handlers, the
///    matched handler's tiers then the global ones, receive `Timeout` under
///    `HttpConfig::timeout_grace`: unclaimed, not answered within the grace, or answered with a
///    body of unknown length, it renders 504.
/// 7. The `HttpCx` built from the request as the stage leaves it, and `ulo::dispatch` with the
///    route's call; an error no handler claims rendered as problem details, `Timeout` (504) when
///    the execution's cancel reason is `Deadline`.
/// 8. The response's headers merged with `HttpCx::response_headers`, a `HEAD` answered by a `GET`
///    handler stripped of its body, [`Routing`] in its extensions (`Routing::Unrouted` when the
///    request was answered before routing decided), and the body wrapped so that a drop before
///    its end fires `CancelReason::Disconnected`. The request counts against the in-flight bound,
///    and its execution stays open, until the backend drops that body.
///
/// A panic while the backend polls the response body ends the body with an error frame, reports
/// the stream `CutOff` and is logged as `PanicRecovered` with stage `Handler`, so it never
/// reaches the server's connection task.
#[derive(Clone)]
pub struct AppService {
    pub(crate) inner: Arc<ServiceInner>,
}

/// Everything a request reads, built once in `prepare`.
pub(crate) struct ServiceInner {
    pub(crate) app: AppHandle,
    pub(crate) router: Router,
    pub(crate) stage: Stage,
    pub(crate) upgrades: Vec<(Pattern, Arc<dyn UpgradeHandler>)>,
    pub(crate) admission: Admission,
    pub(crate) config: Arc<HttpConfig>,
    pub(crate) timer: Arc<dyn Timer>,
    /// The embedding's normalized `.nested_at` prefix; empty for a backend.
    pub(crate) mount: Arc<str>,
    /// The embedding's `Miss::Forward`: a miss on a request nothing has changed is answered
    /// forwardable rather than through the error handlers.
    pub(crate) forward: bool,
}

impl AppService {
    pub(crate) fn new(inner: ServiceInner) -> Self {
        AppService { inner: Arc::new(inner) }
    }

    /// Answers one request; never fails, an error being rendered as a response.
    pub fn call(&self, req: Request) -> impl Future<Output = Response> + Send + use<> {
        let inner = Arc::clone(&self.inner);
        async move { inner.respond(req).await }
    }
}

/// What one request carries from its entry to routing and back.
struct Tracking {
    /// Set once routing has decided; the response gains it on the way out, `Routing::Unrouted`
    /// when it is unset.
    routing: OnceLock<Routing>,
    /// Kept under `Miss::Forward` only.
    forward: Option<Untouched>,
}

/// What a request must still be for its miss to be forwarded: its body never polled and its path
/// as the client sent it.
struct Untouched {
    polled: Arc<AtomicBool>,
    path: String,
}

impl Tracking {
    fn route(&self, routing: Routing) {
        let _ = self.routing.set(routing);
    }

    /// Whether `req` still has its original body unread and its original path.
    fn forwardable(&self, req: &Request) -> bool {
        self.forward.as_ref().is_some_and(|untouched| !untouched.polled.load(Ordering::Acquire) && req.path() == untouched.path)
    }
}

/// A routing miss, as routing decided it.
enum Miss {
    NotFound,
    MethodNotAllowed(HeaderValue),
    Options(HeaderValue, Arc<str>),
}

impl ServiceInner {
    async fn respond(self: Arc<Self>, mut req: Request) -> Response {
        let Some(permit) = self.admission.try_admit() else {
            return render::shed(&self.config);
        };
        let Ok(exec) = Execution::open(&self.app.root(), ExecOptions::new()) else {
            return render::draining(&self.config);
        };
        exec.seed(RequestHead { parts: req.head.clone() });
        if let Some(peer) = req.conn.peer {
            exec.seed(ClientAddr(peer));
        }
        let handle = exec.handle();
        let forward = self.forward.then(|| {
            let polled = Arc::new(AtomicBool::new(false));
            let body = std::mem::take(&mut req.body);
            req.body = HttpBody::new(Watched { inner: body, polled: Arc::clone(&polled) });
            Untouched { polled, path: req.path().to_owned() }
        });
        let tracking = Arc::new(Tracking { routing: OnceLock::new(), forward });
        let method = req.head.method.clone();
        let call_span = span::call(<Http as Transport>::KEY, method.as_str(), None);
        call_span.record(span::HTTP_REQUEST_METHOD, method.as_str());
        let path = req.head.extensions.get::<OriginalPath>().map_or_else(|| req.head.uri.path(), OriginalPath::as_str);
        call_span.record(span::URL_PATH, path);
        call_span.record(span::URL_SCHEME, if req.conn.tls.is_some() { "https" } else { "http" });
        let mut response = self.unscoped(Arc::new(exec), req, call_span.clone(), Arc::clone(&tracking)).instrument(call_span.clone()).await;
        // Set here rather than where routing decides, so a pre-dispatch entry that rebuilt the
        // response does not drop it.
        if response.extensions().get::<Routing>().is_none() {
            response.extensions_mut().insert(tracking.routing.get().cloned().unwrap_or(Routing::Unrouted));
        }
        let status = response.status();
        call_span.record(span::HTTP_RESPONSE_STATUS_CODE, status.as_u16());
        // A body the backend never writes is dropped unread, which is not the peer leaving.
        let unwritten = method == Method::HEAD
            || status.is_informational()
            || status == StatusCode::NO_CONTENT
            || status == StatusCode::NOT_MODIFIED;
        let app = self.app.clone();
        response.map(|body| HttpBody::new(ExecBody::new(body, handle, app, permit, unwritten)))
    }

    /// The unscoped sub-step, routing after it.
    fn unscoped(self: &Arc<Self>, exec: Arc<Execution>, req: Request, call_span: Span, tracking: Arc<Tracking>) -> BoxFuture<'static, Response> {
        let steps = self.stage.runnable();
        let at = (!steps.is_empty()).then(|| StageCx {
            service: Arc::clone(self),
            exec: exec.handle(),
            head: Arc::new(RequestHead { parts: req.head.clone() }),
            conn: req.conn.clone(),
            route: None,
        });
        let this = Arc::clone(self);
        let end: Rest = Box::new(move |req| this.route(exec, req, call_span, tracking));
        match at {
            Some(at) => pre_dispatch::run(Arc::new(at), steps, 0, req, end),
            None => end(req),
        }
    }

    fn route(self: Arc<Self>, exec: Arc<Execution>, req: Request, call_span: Span, tracking: Arc<Tracking>) -> BoxFuture<'static, Response> {
        Box::pin(async move {
            if req.headers().contains_key(UPGRADE) {
                let taker = self.upgrades.iter().find(|(path, _)| path.matches(req.path()).is_some()).map(|(_, taker)| Arc::clone(taker));
                if let Some(taker) = taker {
                    return taker.upgrade(req).await;
                }
            }
            let outside = req.head.extensions.get::<crate::embed::OutsidePrefix>().is_some();
            let routed = match self.router.route(req.method(), req.path()) {
                _ if outside => Err(Miss::NotFound),
                Routed::Found { target, params, head_from_get } => Ok((Arc::clone(target), params, head_from_get)),
                Routed::NotFound => Err(Miss::NotFound),
                Routed::MethodNotAllowed { allow } => Err(Miss::MethodNotAllowed(allow)),
                Routed::Options { allow, route } => Err(Miss::Options(allow, route)),
            };
            match routed {
                Ok((target, params, head_from_get)) => {
                    tracking.route(Routing::Matched { route: Arc::clone(&target.route), handler: Arc::clone(target.handler.info()) });
                    self.found(&exec, req, target, params, head_from_get, &call_span).await
                }
                Err(Miss::NotFound) => {
                    tracking.route(Routing::NotFound);
                    if self.forward {
                        if tracking.forwardable(&req) {
                            return self.forwardable();
                        }
                        tracing::warn!(
                            path = req.path(),
                            "a miss under `Miss::Forward` is answered 404 by the app: a pre-dispatch entry read the body or rewrote the path, \
                             so the host cannot route the request again"
                        );
                    }
                    self.miss(&exec.handle(), req, Miss::NotFound).await
                }
                Err(miss) => {
                    tracking.route(match &miss {
                        Miss::Options(_, route) => Routing::Options { route: Arc::clone(route) },
                        _ => Routing::MethodNotAllowed,
                    });
                    self.miss(&exec.handle(), req, miss).await
                }
            }
        })
    }

    async fn found(
        self: &Arc<Self>,
        exec: &Execution,
        req: Request,
        target: Arc<RouteTarget>,
        params: PathParams,
        head_from_get: bool,
        call_span: &Span,
    ) -> Response {
        exec.route_to(target.handler.module());
        let handle = exec.handle();
        call_span.record(span::HTTP_ROUTE, &*target.route);
        call_span.record(span::OTEL_NAME, field::display(RouteName { method: req.method(), pattern: &target.route }));
        call_span.record(span::HANDLER, field::display(HandlerName(&target.handler)));
        let timeout = target.timeout;
        let response = match timeout {
            None => self.scoped(handle, req, target, params).await,
            Some(after) => self.timed(handle, req, target, params, after).await,
        };
        if head_from_get { without_body(response) } else { response }
    }

    /// The scoped sub-step and `dispatch`, raced against the route's timeout.
    async fn timed(
        self: &Arc<Self>,
        exec: ExecutionRef,
        req: Request,
        target: Arc<RouteTarget>,
        params: PathParams,
        after: Duration,
    ) -> Response {
        // The pipeline owns the request, and a timeout drops it with the request inside.
        let head = Arc::new(RequestHead { parts: req.head.clone() });
        let conn = req.conn.clone();
        let pipeline = self.scoped(exec.clone(), req, Arc::clone(&target), params.clone());
        match race(pipeline, self.timer.sleep(after), &exec).await {
            Raced::Done(response, deadline) => response.map(|body| HttpBody::new(TimedBody { inner: body, deadline, exec })),
            Raced::Expired => self.expired(&exec, head, conn, &target, params).await,
        }
    }

    /// A route timeout that passed before an answer: the error handlers receive `Timeout` with a
    /// context built from the head as the scoped sub-step received it, no body and no upgrade.
    /// What they answer, or the error they return rendered as it stands, is the response; with
    /// none by the end of the grace, the canonical 504 is, and the headers they wrote are dropped.
    /// A response whose body has no known length, an `Sse` among them, is a stream: it is dropped
    /// unread for the same 504, logged at `warn`. The grace bounds the error handlers, not a body
    /// they return.
    async fn expired(&self, exec: &ExecutionRef, head: Arc<RequestHead>, conn: ConnInfo, target: &RouteTarget, params: PathParams) -> Response {
        let route = MatchedRoute {
            handler: target.handler.clone(),
            pattern: Arc::clone(&target.pattern),
            params,
            body_limit: target.body_limit,
        };
        let cx = self.context(exec, head, conn, Some(route), None, None);
        let recovering = ulo::recover(Some(&target.handler), exec, &cx, Box::new(render::timed_out()));
        let outcome = match self.config.grace() {
            Some(grace) => match within(recovering, self.timer.sleep(grace)).await {
                Some(outcome) => outcome,
                None => return render::problem(&render::timed_out(), &self.config),
            },
            None => recovering.await,
        };
        let response = match outcome {
            Ok(response) => response,
            Err(err) => render::render_as_is(err, &self.config),
        };
        if response.body().size_hint().exact().is_none() {
            tracing::warn!(
                route = &*target.route,
                "an error handler answered a timed-out call with a stream; the stream was ended at the deadline"
            );
            return render::problem(&render::timed_out(), &self.config);
        }
        merge_headers(&cx, response)
    }

    /// The scoped sub-step, `dispatch` after it.
    fn scoped(self: &Arc<Self>, exec: ExecutionRef, req: Request, target: Arc<RouteTarget>, params: PathParams) -> BoxFuture<'static, Response> {
        let steps = target.stage.runnable();
        if steps.is_empty() {
            return self.dispatch(exec, req, target, params);
        }
        let at = StageCx {
            service: Arc::clone(self),
            exec: exec.clone(),
            head: Arc::new(RequestHead { parts: req.head.clone() }),
            conn: req.conn.clone(),
            route: Some((Arc::clone(&target), params.clone())),
        };
        let this = Arc::clone(self);
        let end: Rest = Box::new(move |req| this.dispatch(exec, req, target, params));
        pre_dispatch::run(Arc::new(at), steps, 0, req, end)
    }

    fn dispatch(self: &Arc<Self>, exec: ExecutionRef, req: Request, target: Arc<RouteTarget>, params: PathParams) -> BoxFuture<'static, Response> {
        let this = Arc::clone(self);
        Box::pin(async move {
            let Request { head, body, conn, upgrade } = req;
            let route = MatchedRoute {
                handler: target.handler.clone(),
                pattern: Arc::clone(&target.pattern),
                params,
                body_limit: target.body_limit,
            };
            let cx = this.context(&exec, Arc::new(RequestHead { parts: head }), conn, Some(route), Some(body), upgrade);
            let call = Arc::clone(&target.call);
            let response = match ulo::dispatch(&target.handler, &exec, &cx, move |cx| (*call)(cx)).await {
                Ok(response) => response,
                Err(err) => render::render_error(err, &exec, &this.config),
            };
            merge_headers(&cx, response)
        })
    }

    /// The 404 a forwardable miss answers, with [`Forwardable`] in its extensions for the
    /// adapter. It skips the error handlers: the host routes the request again, and a host that
    /// does not still sends a 404.
    fn forwardable(&self) -> Response {
        let mut response = render::problem(&NoRoute::error(), &self.config);
        response.extensions_mut().insert(Forwardable { _private: () });
        response
    }

    async fn miss(&self, exec: &ExecutionRef, req: Request, miss: Miss) -> Response {
        let err: BoxError = match miss {
            Miss::Options(allow, _) => return options(allow),
            Miss::NotFound => Box::new(NoRoute::error()),
            Miss::MethodNotAllowed(allow) => Box::new(MethodNotAllowed::new(req.head.method.clone(), allow).into_error()),
        };
        let Request { head, body, conn, upgrade } = req;
        let cx = self.context(exec, Arc::new(RequestHead { parts: head }), conn, None, Some(body), upgrade);
        let response = match ulo::recover::<Http>(None, exec, &cx, err).await {
            Ok(response) => response,
            Err(err) => render::render_error(err, exec, &self.config),
        };
        merge_headers(&cx, response)
    }

    /// A request's context, as `dispatch` and the error handlers read it.
    pub(crate) fn context(
        &self,
        exec: &ExecutionRef,
        head: Arc<RequestHead>,
        conn: ConnInfo,
        route: Option<MatchedRoute>,
        body: Option<HttpBody>,
        upgrade: Option<OnUpgrade>,
    ) -> HttpCx {
        HttpCx {
            inner: Arc::new(CxInner {
                exec: exec.clone(),
                app: self.app.clone(),
                head,
                conn,
                route,
                body: Mutex::new(body),
                upgrade: Mutex::new(upgrade),
                response_headers: Mutex::new(HeaderMap::new()),
                config: Arc::clone(&self.config),
                mount: Arc::clone(&self.mount),
                timer: Arc::clone(&self.timer),
            }),
        }
    }
}

/// `cx`'s response headers added to `response`, a name the response already carries keeping the
/// response's values.
pub(crate) fn merge_headers(cx: &HttpCx, mut response: Response) -> Response {
    let written = std::mem::take(&mut *cx.response_headers());
    let headers = response.headers_mut();
    let mut name: Option<HeaderName> = None;
    let mut keep = false;
    // A `HeaderMap` yields a name with its first value and `None` with each further one.
    for (next, value) in written {
        if let Some(next) = next {
            keep = !headers.contains_key(&next);
            name = Some(next);
        }
        if let (true, Some(name)) = (keep, &name) {
            headers.append(name.clone(), value);
        }
    }
    response
}

/// 204 with `Allow`: `OPTIONS` on a path whose routes have no `OPTIONS` handler.
fn options(allow: HeaderValue) -> Response {
    let mut response = Response::new(HttpBody::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    response.headers_mut().insert(ALLOW, allow);
    response
}

/// `response` without its body: RFC 9110 has a `HEAD` answer carry the `GET` answer's headers and
/// no content, a known length kept as `Content-Length`. A 1xx or 204 answer must not carry
/// `Content-Length` (RFC 9110 §8.6), and a 304 may carry only the length a 200 would have had,
/// which the dropped body does not establish, so none of those gains one.
fn without_body(response: Response) -> Response {
    let (mut parts, body) = response.into_parts();
    let carries_length =
        !(parts.status.is_informational() || parts.status == StatusCode::NO_CONTENT || parts.status == StatusCode::NOT_MODIFIED);
    if carries_length && !parts.headers.contains_key(CONTENT_LENGTH) {
        if let Some(length) = body.size_hint().exact() {
            parts.headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
        }
    }
    Response::from_parts(parts, HttpBody::empty())
}

enum Raced {
    /// The pipeline answered first; the sleep, still pending, goes on with the body.
    Done(Response, Option<BoxFuture<'static, ()>>),
    Expired,
}

/// `pipeline` raced against the route's timeout. The pipeline is polled first, so an answer ready
/// in the poll the sleep fires in counts as answered; when the sleep wins, `exec` is cancelled
/// with `CancelReason::Deadline` and the pipeline then dropped at its current await, so a handler's
/// future dropped there reads the reason.
async fn race(mut pipeline: BoxFuture<'static, Response>, sleep: BoxFuture<'static, ()>, exec: &ExecutionRef) -> Raced {
    let mut sleep = Some(sleep);
    poll_fn(move |cx| {
        if let Poll::Ready(response) = pipeline.as_mut().poll(cx) {
            return Poll::Ready(Raced::Done(response, sleep.take()));
        }
        match sleep.as_mut().map(|sleep| sleep.as_mut().poll(cx)) {
            Some(Poll::Ready(())) => {
                exec.cancel_with(CancelReason::Deadline);
                Poll::Ready(Raced::Expired)
            }
            _ => Poll::Pending,
        }
    })
    .await
}

/// `fut` raced against `sleep`, polled first: `None` when the sleep wins, `fut` dropped at its
/// current await.
async fn within<F: Future>(fut: F, mut sleep: BoxFuture<'static, ()>) -> Option<F::Output> {
    let mut fut = pin!(fut);
    poll_fn(move |cx| {
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        sleep.as_mut().poll(cx).map(|()| None)
    })
    .await
}

/// A route's timeout passing after its answer began: the execution is cancelled with `Deadline`,
/// which a streaming body observes through `cancelled()`, and the body is otherwise left to end as
/// it ends.
struct TimedBody {
    inner: HttpBody,
    deadline: Option<BoxFuture<'static, ()>>,
    exec: ExecutionRef,
}

impl http_body::Body for TimedBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        if let Some(deadline) = this.deadline.as_mut() {
            if deadline.as_mut().poll(cx).is_ready() {
                this.deadline = None;
                this.exec.cancel_with(CancelReason::Deadline);
            }
        }
        let frame = Pin::new(&mut this.inner).poll_frame(cx);
        if matches!(frame, Poll::Ready(None)) || this.inner.is_end_stream() {
            this.deadline = None;
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// The outermost response body: it keeps the execution and the in-flight permit until the
/// backend drops it, and a drop before the body's end is the peer leaving, so it fires
/// `CancelReason::Disconnected`. A body ending in an error frame was cut off by the server, not
/// the peer.
struct ExecBody {
    inner: HttpBody,
    exec: ExecutionRef,
    /// The app, whose redaction a panic's message goes through.
    app: AppHandle,
    _permit: Permit,
    ended: bool,
}

impl ExecBody {
    fn new(inner: HttpBody, exec: ExecutionRef, app: AppHandle, permit: Permit, unwritten: bool) -> Self {
        let ended = unwritten || inner.is_end_stream();
        ExecBody { inner, exec, app, _permit: permit, ended }
    }

    /// A panic in the body's `poll_frame`: the body is ended, its stream reported `CutOff`, the
    /// panicked body dropped without another poll, and the panic logged as `PanicRecovered` with
    /// stage `Handler`. Answers the error frame that ends the body, which the server observes as
    /// a body cut off by the server.
    fn recover(&mut self, payload: Box<dyn Any + Send>, cx: &mut Context<'_>) -> BoxError {
        self.ended = true;
        self.exec.report_stream_end(StreamOutcome::CutOff(self.exec.cancel_reason()));
        let panicked = std::mem::take(&mut self.inner);
        // A stream that panicked may panic again in its drop.
        let _ = catch_unwind(AssertUnwindSafe(move || drop(panicked)));
        let error = panic_recovered(&self.app, payload, cx);
        tracing::error!(error = %error, "a response body panicked while it was written; it ends with an error frame");
        error
    }
}

/// `payload` as `PanicRecovered` with stage `Handler`, its message redacted: resumed inside
/// `AppHandle::catch_panic`, which builds it. Resuming runs no panic hook, so the panic is
/// reported once, where it happened.
fn panic_recovered(app: &AppHandle, payload: Box<dyn Any + Send>, cx: &mut Context<'_>) -> BoxError {
    let mut caught = pin!(app.catch_panic(DispatchStage::Handler, async move { rethrow(payload) }));
    // One poll completes this: `catch_panic` polls the inner future under `catch_unwind` with no
    // await before it, and the inner future resumes the panic on its first poll. `poll_frame` is
    // synchronous and cannot wait for a second poll, so an await added in the block above, or in
    // `catch_panic` ahead of the inner poll, returns `Pending` here, and the panic is then
    // reported by the fallback below with its stage and redaction lost.
    match caught.as_mut().poll(cx) {
        Poll::Ready(Err(error)) => error,
        _ => BoxError::from("a response body panicked"),
    }
}

fn rethrow(payload: Box<dyn Any + Send>) -> Result<(), BoxError> {
    resume_unwind(payload)
}

impl http_body::Body for ExecBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        // The body is never polled again after a panic: `recover` replaces it.
        let frame = match catch_unwind(AssertUnwindSafe(|| Pin::new(&mut this.inner).poll_frame(cx))) {
            Ok(frame) => frame,
            Err(payload) => return Poll::Ready(Some(Err(this.recover(payload, cx)))),
        };
        match &frame {
            Poll::Ready(None | Some(Err(_))) => this.ended = true,
            Poll::Ready(Some(Ok(_))) if this.inner.is_end_stream() => this.ended = true,
            _ => {}
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for ExecBody {
    // Runs before `inner` drops, so a `Tracked` stream inside reports the reason.
    fn drop(&mut self) {
        if !self.ended {
            self.exec.cancel_with(CancelReason::Disconnected);
        }
    }
}

/// The span's display name, `GET /users/{id}`.
struct RouteName<'a> {
    method: &'a Method,
    pattern: &'a str,
}

impl fmt::Display for RouteName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.method, self.pattern)
    }
}

/// The span's `ulo.handler`: `UsersController::get`.
struct HandlerName<'a>(&'a MountedHandler<Http>);

impl fmt::Display for HandlerName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.0.controller(), self.0.name())
    }
}

/// A request body under `Miss::Forward`, recording whether anything polled it: a body read in
/// part cannot be handed back to the host.
struct Watched {
    inner: HttpBody,
    polled: Arc<AtomicBool>,
}

impl http_body::Body for Watched {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        self.polled.store(true, Ordering::Release);
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
