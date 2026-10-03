use std::error::Error;
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE, HeaderName, HeaderValue, UPGRADE};
use http::{HeaderMap, Method, StatusCode};
use http_body::{Body as _, Frame, SizeHint};
use tracing::{Instrument, Span, field};
use ulo::{AppHandle, BoxError, BoxFuture, CancelReason, ExecOptions, Execution, ExecutionRef, MountedHandler, Timer, Transport};
use ulo_transport::{Admission, CallError, ErrorKind, Permit, span};

use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::cx::{CxInner, HttpCx, MatchedRoute, PathParams};
use crate::pre_dispatch::{self, Rest, Stage, StageCx};
use crate::render;
use crate::request::{ConnInfo, OnUpgrade, Request};
use crate::response::Response;
use crate::router::pattern::Pattern;
use crate::router::{RouteTarget, Routed, Router};
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
///    `ulo_transport::span::call` span entered.
/// 4. The unscoped pre-dispatch entries, in order, inside `AppHandle::catch_panic`.
/// 5. An `Upgrade` request on an upgrade path goes to its `UpgradeHandler`. Otherwise routing: a
///    miss renders 404 or 405 with `Allow` after the global error handlers through
///    `ulo::recover(None, ..)`; `OPTIONS` with no handler answers 204 with `Allow`.
/// 6. `Execution::route_to` the controller's module, the route's timeout armed on the app's
///    `Timer` (`CancelReason::Deadline` when it passes), the scoped entries run.
/// 7. The `HttpCx` built from the request as the stage leaves it, and `ulo::dispatch` with the
///    route's call; an error no handler claims rendered as problem details, `Timeout` (504) when
///    the execution's cancel reason is `Deadline`.
/// 8. The response's headers merged with `HttpCx::response_headers`, a `HEAD` answered by a `GET`
///    handler stripped of its body, and the body wrapped so that a drop before its end fires
///    `CancelReason::Disconnected`. The request counts against the in-flight bound, and its
///    execution stays open, until the backend drops that body.
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
}

impl AppService {
    pub(crate) fn new(inner: ServiceInner) -> Self {
        AppService { inner: Arc::new(inner) }
    }

    /// Answers one request; never fails, an error being rendered as a response.
    pub fn call(&self, req: Request) -> impl Future<Output = Response> + Send + 'static {
        let inner = Arc::clone(&self.inner);
        async move { inner.respond(req).await }
    }
}

/// A routing miss, as routing decided it.
enum Miss {
    NotFound,
    MethodNotAllowed(HeaderValue),
    Options(HeaderValue),
}

impl ServiceInner {
    async fn respond(self: Arc<Self>, req: Request) -> Response {
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
        let method = req.head.method.clone();
        let call_span = span::call(<Http as Transport>::KEY, method.as_str(), None);
        call_span.record(span::HTTP_REQUEST_METHOD, method.as_str());
        call_span.record(span::URL_PATH, req.head.uri.path());
        call_span.record(span::URL_SCHEME, if req.conn.tls.is_some() { "https" } else { "http" });
        let response = self.unscoped(Arc::new(exec), req, call_span.clone()).instrument(call_span.clone()).await;
        let status = response.status();
        call_span.record(span::HTTP_RESPONSE_STATUS_CODE, status.as_u16());
        // A body the backend never writes is dropped unread, which is not the peer leaving.
        let unwritten = method == Method::HEAD
            || status.is_informational()
            || status == StatusCode::NO_CONTENT
            || status == StatusCode::NOT_MODIFIED;
        response.map(|body| HttpBody::new(ExecBody::new(body, handle, permit, unwritten)))
    }

    /// The unscoped sub-step, routing after it.
    fn unscoped(self: &Arc<Self>, exec: Arc<Execution>, req: Request, call_span: Span) -> BoxFuture<'static, Response> {
        let steps = self.stage.runnable();
        let at = (!steps.is_empty()).then(|| StageCx {
            service: Arc::clone(self),
            exec: exec.handle(),
            head: Arc::new(RequestHead { parts: req.head.clone() }),
            conn: req.conn.clone(),
            route: None,
        });
        let this = Arc::clone(self);
        let end: Rest = Box::new(move |req| this.route(exec, req, call_span));
        match at {
            Some(at) => pre_dispatch::run(Arc::new(at), steps, 0, req, end),
            None => end(req),
        }
    }

    fn route(self: Arc<Self>, exec: Arc<Execution>, req: Request, call_span: Span) -> BoxFuture<'static, Response> {
        Box::pin(async move {
            if req.headers().contains_key(UPGRADE) {
                let taker = self.upgrades.iter().find(|(path, _)| path.matches(req.path()).is_some()).map(|(_, taker)| Arc::clone(taker));
                if let Some(taker) = taker {
                    return taker.upgrade(req).await;
                }
            }
            let routed = match self.router.route(req.method(), req.path()) {
                Routed::Found { target, params, head_from_get } => Ok((Arc::clone(target), params, head_from_get)),
                Routed::NotFound => Err(Miss::NotFound),
                Routed::MethodNotAllowed { allow } => Err(Miss::MethodNotAllowed(allow)),
                Routed::Options { allow } => Err(Miss::Options(allow)),
            };
            match routed {
                Ok((target, params, head_from_get)) => self.found(&exec, req, target, params, head_from_get, &call_span).await,
                Err(miss) => self.miss(&exec.handle(), req, miss).await,
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
        call_span.record(span::HTTP_ROUTE, &*target.pattern);
        call_span.record(span::OTEL_NAME, field::display(RouteName { method: req.method(), pattern: &target.pattern }));
        call_span.record(span::HANDLER, field::display(HandlerName(&target.handler)));
        let timeout = target.timeout;
        let pipeline = self.scoped(handle.clone(), req, target, params);
        let response = match timeout {
            None => pipeline.await,
            Some(after) => match race(pipeline, self.timer.sleep(after)).await {
                Raced::Done(response, deadline) => {
                    response.map(|body| HttpBody::new(TimedBody { inner: body, deadline, exec: handle.clone() }))
                }
                Raced::Expired => {
                    handle.cancel_with(CancelReason::Deadline);
                    render::problem(&render::timed_out(), &self.config)
                }
            },
        };
        if head_from_get { without_body(response) } else { response }
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

    async fn miss(&self, exec: &ExecutionRef, req: Request, miss: Miss) -> Response {
        let err: BoxError = match miss {
            Miss::Options(allow) => return options(allow),
            Miss::NotFound => Box::new(CallError::new(ErrorKind::NotFound, "no route matches this path")),
            Miss::MethodNotAllowed(allow) => Box::new(MethodNotAllowed { method: req.head.method.clone(), allow }),
        };
        let Request { head, body, conn, upgrade } = req;
        let cx = self.context(exec, Arc::new(RequestHead { parts: head }), conn, None, Some(body), upgrade);
        let response = match ulo::recover::<Http>(None, exec, &cx, err).await {
            Ok(response) => response,
            Err(err) => match err.downcast::<MethodNotAllowed>() {
                Ok(refused) => method_not_allowed(&refused),
                Err(err) => render::render_error(err, exec, &self.config),
            },
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

/// 405 as problem details, with the `Allow` RFC 9110 requires. No `ErrorKind` is 405, so this is
/// rendered here rather than through `CallError`.
fn method_not_allowed(refused: &MethodNotAllowed) -> Response {
    let document = serde_json::json!({
        "type": "about:blank",
        "title": "Method Not Allowed",
        "status": 405,
        "detail": refused.to_string(),
    });
    let mut response = Response::new(HttpBody::from(document.to_string()));
    *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
    response.headers_mut().insert(ALLOW, refused.allow.clone());
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
/// in the poll the sleep fires in counts as answered; when the sleep wins, the pipeline is dropped
/// at its current await.
async fn race(mut pipeline: BoxFuture<'static, Response>, sleep: BoxFuture<'static, ()>) -> Raced {
    let mut sleep = Some(sleep);
    poll_fn(move |cx| {
        if let Poll::Ready(response) = pipeline.as_mut().poll(cx) {
            return Poll::Ready(Raced::Done(response, sleep.take()));
        }
        match sleep.as_mut().map(|sleep| sleep.as_mut().poll(cx)) {
            Some(Poll::Ready(())) => Poll::Ready(Raced::Expired),
            _ => Poll::Pending,
        }
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
    _permit: Permit,
    ended: bool,
}

impl ExecBody {
    fn new(inner: HttpBody, exec: ExecutionRef, permit: Permit, unwritten: bool) -> Self {
        let ended = unwritten || inner.is_end_stream();
        ExecBody { inner, exec, _permit: permit, ended }
    }
}

impl http_body::Body for ExecBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        let frame = Pin::new(&mut this.inner).poll_frame(cx);
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

/// A path that matches with no handler for the method, as the global error handlers receive it.
#[derive(Debug)]
struct MethodNotAllowed {
    method: Method,
    allow: HeaderValue,
}

impl fmt::Display for MethodNotAllowed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "this path does not answer `{}`", self.method)?;
        if let Ok(allow) = self.allow.to_str() {
            write!(f, "; it answers {allow}")?;
        }
        Ok(())
    }
}

impl Error for MethodNotAllowed {}
