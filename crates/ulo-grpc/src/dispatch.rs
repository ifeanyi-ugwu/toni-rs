//! The `:path` dispatcher (transports DESIGN §6.1): one service routing each call to its marker's
//! handler by path. A path off the table answers UNIMPLEMENTED, offered to no error handler.
//! `grpc-timeout` is parsed here into the execution's deadline.
//!
//! Per call, in order:
//! 1. Over the server's or the connection's in-flight bound: UNAVAILABLE.
//! 2. `Execution::open` at the root module, its deadline the caller's `grpc-timeout`. Refused
//!    during the drain: UNAVAILABLE, answered directly, no pre-dispatch stage.
//! 3. The inputs `GrpcMetadata` and `PeerAddr` seeded as the client sent them, and the
//!    `ulo_transport::span::call` span entered, named `package.Service/Method`.
//! 4. The unscoped pre-dispatch entries of `PreDispatch<Grpc>`, in order.
//! 5. The path routed: a handler's path routes the execution to the controller's module and runs
//!    the scoped entries, then `ulo::dispatch`; the health and reflection services answer their
//!    own paths; any other path answers UNIMPLEMENTED.
//! 6. An error no handler claims rendered as a status: a `tonic::Status` as it stands,
//!    DEADLINE_EXCEEDED when the deadline cancelled the call, any other error by its kind.
//! 7. A response a pre-dispatch entry answered with an HTTP status other than 200 translated by
//!    the gRPC specification's HTTP-to-status table.
//!
//! The whole of it is raced against the deadline: when the deadline passes first, the pipeline is
//! dropped at its await, the execution cancelled with `CancelReason::Deadline`, and the error
//! handlers receive `Timeout` under `Server::timeout_grace`, the matched handler's tiers then the
//! global ones, or the global ones alone when the deadline passed before routing. What they
//! answer, or the error they return rendered as it stands, is the reply; unclaimed, or with no
//! answer by the end of the grace, the call answers DEADLINE_EXCEEDED. A reply they answer is
//! delivered only as a single reply: its body is read under the same grace, and one carrying a
//! second message, or not ended when the grace runs out, is dropped for DEADLINE_EXCEEDED, logged
//! at `warn` as a stream. When the reply comes first,
//! the deadline moves into its body, which ends with DEADLINE_EXCEEDED trailers, offered to no
//! error handler, if the deadline passes while it streams.
//!
//! The dispatcher is built on tonic's codec layer, the pieces `tonic::server::Grpc` composes:
//! `tonic::Streaming::new_request` decodes a request in the extractors, after the guards have
//! admitted, and `EncodeBody::new_server` encodes a reply into the response every interceptor
//! sees. `Grpc::unary` and its siblings decode, call and encode in one call whose service answers
//! an unencoded message, which would leave the interceptors no encoded reply to read and decode a
//! message before the guards run.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue};
use http::{HeaderMap, StatusCode};
use http_body::{Body as _, Frame, SizeHint};
use hyper::body::Incoming;
use tonic::codec::{BufferSettings, EncodeBody, SingleMessageCompressionOverride};
use tonic::metadata::MetadataMap;
use tonic::server::NamedService;
use tonic::{Code, Status};
use tonic_prost::ProstEncoder;
use tower::ServiceExt;
use tracing::{Instrument, Span, field};
use ulo::{
    AppHandle, BoxError, BoxFuture, CancelReason, ExecOptions, Execution, ExecutionRef, LateOutcome, MountedHandler,
    StreamOutcome, Timer, Transport,
};
use ulo_http::stage::{Rest, ScopedStage, Stage};
use ulo_http::{ConnInfo, HttpBody, Request, Response};
use ulo_transport::{Admission, CallError, ConnectionAdmission, Permit, Tracked, span};

use crate::__private::HandlerFn;
use crate::pre_dispatch::StageCx;
use crate::status::{self, code_for_http};
use crate::transport::{CxInner, Grpc, GrpcCx, GrpcMetadata, IntoGrpcItem, PeerAddr, Reply};

const GRPC_STATUS: HeaderName = HeaderName::from_static("grpc-status");
const GRPC_ENCODING: HeaderName = HeaderName::from_static("grpc-encoding");
const GRPC_ACCEPT_ENCODING: HeaderName = HeaderName::from_static("grpc-accept-encoding");
const GRPC_TIMEOUT: HeaderName = HeaderName::from_static("grpc-timeout");

/// The headers a reply's metadata never sets: the protocol's own, which the reply and its
/// trailers write.
const RESERVED: [&str; 8] = [
    "te",
    "content-type",
    "content-length",
    "grpc-status",
    "grpc-message",
    "grpc-message-type",
    "grpc-status-details-bin",
    "grpc-encoding",
];

/// A service the server answers beside the handlers, health and reflection, called with the
/// request as the unscoped pre-dispatch entries left it.
pub(crate) type Builtin = Arc<dyn Fn(http::Request<HttpBody>) -> BoxFuture<'static, Reply> + Send + Sync>;

/// Everything a call reads, built once in `prepare`.
pub(crate) struct Dispatcher {
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    pub(crate) routes: HashMap<&'static str, Arc<Route>>,
    /// By service name, `grpc.health.v1.Health`.
    pub(crate) builtins: Vec<(&'static str, Builtin)>,
    pub(crate) stage: Stage<Grpc>,
    pub(crate) admission: Admission,
    /// The per-connection in-flight bound, `None` for none.
    pub(crate) per_connection: Option<usize>,
    /// `Server::timeout_grace` as a duration; `None` waits for the error handlers.
    pub(crate) grace: Option<Duration>,
}

/// One method path's handler, as `prepare` built it.
pub(crate) struct Route {
    pub(crate) handler: MountedHandler<Grpc>,
    pub(crate) call: HandlerFn,
    /// The pre-dispatch entries whose scope covers the path, composed once.
    pub(crate) stage: Arc<ScopedStage<Grpc>>,
}

impl Dispatcher {
    /// One connection's in-flight bound, nested in the server's.
    pub(crate) fn connection(&self) -> ConnectionAdmission {
        self.admission.connection(self.per_connection)
    }

    /// Answers one call; never fails, an error being answered as a status.
    pub(crate) async fn call(self: Arc<Self>, req: http::Request<Incoming>, conn: ConnInfo, admission: ConnectionAdmission) -> Response {
        let Some(permit) = admission.try_admit() else {
            return status_response(Status::unavailable("the server is at its in-flight limit"));
        };
        let timeout = grpc_timeout(req.headers());
        let mut opts = ExecOptions::new();
        if let Some(deadline) = timeout.and_then(|after| self.timer.now().checked_add(after)) {
            opts = opts.deadline(deadline);
        }
        let Ok(exec) = Execution::open(&self.app.root(), opts) else {
            return status_response(Status::unavailable("the server is shutting down"));
        };
        let (head, body) = req.into_parts();
        let metadata = GrpcMetadata::from_headers(&head.headers);
        exec.seed(metadata.clone());
        if let Some(peer) = conn.peer {
            exec.seed(PeerAddr(peer));
        }
        let path: Arc<str> = Arc::from(head.uri.path());
        let peer = conn.peer;
        let call_span = call_span(head.uri.path());
        let handle = exec.handle();
        let mut conn = conn;
        conn.version = head.version;
        let request = Request { head, body: HttpBody::new(body), conn, upgrade: None };
        let matched = Arc::new(OnceLock::new());
        let pipeline: BoxFuture<'static, Response> = Box::pin(
            Arc::clone(&self).unscoped(Arc::new(exec), request, call_span.clone(), Arc::clone(&matched)).instrument(call_span.clone()),
        );
        let mut abandoned = CancelOnDrop(Some(handle.clone()));
        let (response, deadline) = match timeout {
            None => (pipeline.await, None),
            Some(after) => match race(pipeline, self.timer.sleep(after), &handle).await {
                Raced::Done(response, deadline) => (response, deadline),
                Raced::Expired => {
                    let handler = matched.get().cloned();
                    let expired = self.expired(&handle, path, metadata, peer, handler).instrument(call_span.clone()).await;
                    (expired, None)
                }
            },
        };
        abandoned.0 = None;
        let response = as_grpc(response);
        if let Some(code) = response.headers().get(GRPC_STATUS) {
            record_status(&call_span, Code::from_bytes(code.as_bytes()));
        }
        response.map(|body| HttpBody::new(CallBody::new(body, handle, deadline, call_span, permit)))
    }

    /// A deadline that passed before the call answered: the error handlers receive `Timeout`
    /// with a context built from the call's path, metadata and peer, no request. What they answer,
    /// if it is a single reply ([`single`]), or the error they return rendered as it stands, is the
    /// reply; with none by the end of the grace, DEADLINE_EXCEEDED is. `handler` is the handler
    /// routing matched, `None` when the deadline passed before routing, which leaves the global
    /// error handlers alone.
    async fn expired(
        &self,
        exec: &ExecutionRef,
        path: Arc<str>,
        metadata: GrpcMetadata,
        peer: Option<SocketAddr>,
        handler: Option<MountedHandler<Grpc>>,
    ) -> Response {
        let cx = self.context(exec, Arc::clone(&path), metadata, peer, handler.clone(), None);
        let recovering = ulo::recover(handler.as_ref(), exec, &cx, BoxError::from(status::timed_out()));
        let mut grace = self.grace.map(|grace| self.timer.sleep(grace));
        let Some(outcome) = within(recovering, grace.as_mut()).await else {
            return status_response(status::deadline_exceeded());
        };
        match outcome {
            Ok(reply) => match single(reply, grace.as_mut()).await {
                Ok(reply) => reply,
                Err(()) => {
                    tracing::warn!(
                        path = &*path,
                        "an error handler answered a timed-out call with a stream; the stream was ended at the deadline"
                    );
                    status_response(status::deadline_exceeded())
                }
            },
            Err(err) => status_response(status::render_as_is(err)),
        }
    }

    /// The unscoped sub-step, routing after it. Routing records the handler it matched in
    /// `matched`, for the error handlers of a deadline that passes after it.
    fn unscoped(
        self: Arc<Self>,
        exec: Arc<Execution>,
        req: Request,
        call_span: Span,
        matched: Arc<OnceLock<MountedHandler<Grpc>>>,
    ) -> BoxFuture<'static, Response> {
        let host = (!self.stage.is_empty()).then(|| StageCx::new(&self, exec.handle(), &req, None));
        let this = Arc::clone(&self);
        let end: Rest = Box::new(move |req| this.route(exec, req, call_span, matched));
        match host {
            Some(host) => self.stage.run(Arc::new(host), req, end),
            None => end(req),
        }
    }

    fn route(
        self: Arc<Self>,
        exec: Arc<Execution>,
        req: Request,
        call_span: Span,
        matched: Arc<OnceLock<MountedHandler<Grpc>>>,
    ) -> BoxFuture<'static, Response> {
        Box::pin(async move {
            if let Some(route) = self.routes.get(req.path()).map(Arc::clone) {
                let _ = matched.set(route.handler.clone());
                exec.route_to(route.handler.module());
                call_span.record(span::HANDLER, field::display(HandlerName(&route.handler)));
                return self.scoped(exec.handle(), req, route).await;
            }
            let builtin = service_of(req.path())
                .and_then(|service| self.builtins.iter().find(|(name, _)| *name == service))
                .map(|(_, builtin)| Arc::clone(builtin));
            if let Some(builtin) = builtin {
                let Request { head, body, .. } = req;
                return (*builtin)(http::Request::from_parts(head, body)).await.map(HttpBody::new);
            }
            status_response(Status::unimplemented(format!("this server has no method `{}`", req.path())))
        })
    }

    /// The scoped sub-step, `dispatch` after it.
    fn scoped(self: &Arc<Self>, exec: ExecutionRef, req: Request, route: Arc<Route>) -> BoxFuture<'static, Response> {
        if route.stage.is_empty() {
            return self.dispatch(exec, req, route);
        }
        let host = StageCx::new(self, exec.clone(), &req, Some(route.handler.clone()));
        let stage = Arc::clone(&route.stage);
        let this = Arc::clone(self);
        let end: Rest = Box::new(move |req| this.dispatch(exec, req, route));
        stage.run(Arc::new(host), req, end)
    }

    fn dispatch(self: &Arc<Self>, exec: ExecutionRef, req: Request, route: Arc<Route>) -> BoxFuture<'static, Response> {
        let this = Arc::clone(self);
        Box::pin(async move {
            if let Some(refused) = unsupported_encoding(req.headers()) {
                return refused;
            }
            let Request { head, body, conn, .. } = req;
            let cx = this.context(
                &exec,
                Arc::from(head.uri.path()),
                GrpcMetadata::from_headers(&head.headers),
                conn.peer,
                Some(route.handler.clone()),
                Some(body),
            );
            let call = Arc::clone(&route.call);
            let reply = match ulo::dispatch(&route.handler, &exec, &cx, move |cx| (*call)(cx)).await {
                Ok(reply) => reply,
                Err(err) => status_reply(status::render(err, &exec)),
            };
            reply.map(HttpBody::new)
        })
    }

    /// A call's context, as `dispatch` and the error handlers read it.
    pub(crate) fn context(
        &self,
        exec: &ExecutionRef,
        path: Arc<str>,
        metadata: GrpcMetadata,
        peer: Option<SocketAddr>,
        handler: Option<MountedHandler<Grpc>>,
        body: Option<HttpBody>,
    ) -> GrpcCx {
        GrpcCx::new(CxInner {
            exec: exec.clone(),
            app: self.app.clone(),
            timer: Arc::clone(&self.timer),
            path,
            metadata,
            peer,
            handler,
            body: Mutex::new(body),
        })
    }
}

/// The health or reflection service `service`, by the name its paths start with.
pub(crate) fn builtin<S>(service: S) -> (&'static str, Builtin)
where
    S: tower::Service<http::Request<HttpBody>, Response = Reply, Error = Infallible> + NamedService + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    // A tonic server is `Send` and not always `Sync`; each call clones it out of the lock.
    let service = Mutex::new(service);
    let call: Builtin = Arc::new(move |request| {
        let service = service.lock().unwrap_or_else(PoisonError::into_inner).clone();
        Box::pin(async move {
            match service.oneshot(request).await {
                Ok(reply) => reply,
                Err(never) => match never {},
            }
        })
    });
    (S::NAME, call)
}

/// One message as a reply: status 200, `application/grpc`, `metadata` in the headers, the message
/// in one data frame, and `grpc-status: 0` in the trailers.
pub(crate) fn encode_one<T: prost::Message + Default + Send + 'static>(message: T, metadata: MetadataMap) -> Reply {
    let messages = futures_util::stream::once(futures_util::future::ready(Ok::<T, Status>(message)));
    let body = EncodeBody::new_server(
        ProstEncoder::<T>::new(BufferSettings::default()),
        messages,
        None,
        SingleMessageCompressionOverride::default(),
        None,
    );
    with_metadata(tonic::body::Body::new(body), metadata)
}

/// A stream as a reply, wrapped in `Tracked`. An `Err` item runs the matched handler's error
/// handlers through `ulo::dispatch_late` and, unless they end the stream with `EndStream`, is
/// written as the trailers `grpc-status`, `grpc-message` and `grpc-status-details-bin`, which end
/// the stream: once data frames are sent, the status travels in the trailers.
pub(crate) fn encode_stream<S>(stream: S, metadata: MetadataMap, cx: &GrpcCx) -> Reply
where
    S: Stream + Send + 'static,
    S::Item: IntoGrpcItem,
{
    let items = LateItems { stream: Box::pin(stream), cx: cx.clone(), state: Late::Streaming };
    let body = EncodeBody::new_server(
        ProstEncoder::<<S::Item as IntoGrpcItem>::Message>::new(BufferSettings::default()),
        Tracked::new(items, cx.exec().clone()),
        None,
        SingleMessageCompressionOverride::default(),
        None,
    );
    with_metadata(tonic::body::Body::new(body), metadata)
}

fn with_metadata(body: tonic::body::Body, metadata: MetadataMap) -> Reply {
    let mut reply = Reply::new(body);
    let headers = reply.headers_mut();
    for (name, value) in metadata.into_headers().iter() {
        if !RESERVED.contains(&name.as_str()) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
    reply
}

/// `status` as a trailers-only reply.
pub(crate) fn status_reply(status: Status) -> Reply {
    status.into_http()
}

/// `status` as a trailers-only response.
pub(crate) fn status_response(status: Status) -> Response {
    status.into_http()
}

/// `response` as a gRPC response: one with HTTP status 200 is left as it is; any other, which a
/// pre-dispatch entry answered, becomes a trailers-only response with the code the gRPC
/// specification's table gives its status, keeping its headers beside the protocol's own.
fn as_grpc(response: Response) -> Response {
    let status = response.status();
    if status == StatusCode::OK {
        return response;
    }
    let (parts, _) = response.into_parts();
    let mut translated = status_response(Status::new(code_for_http(status), format!("a pre-dispatch entry answered HTTP {status}")));
    let headers = translated.headers_mut();
    for (name, value) in &parts.headers {
        if !RESERVED.contains(&name.as_str()) {
            headers.append(name.clone(), value.clone());
        }
    }
    translated
}

/// UNIMPLEMENTED with `grpc-accept-encoding: identity` for a request compressed with an encoding
/// this server does not decompress, which the gRPC compression specification prescribes; `None`
/// for an uncompressed request.
fn unsupported_encoding(headers: &HeaderMap) -> Option<Response> {
    let encoding = headers.get(GRPC_ENCODING)?;
    if encoding.as_bytes() == b"identity" {
        return None;
    }
    let message = format!("this server does not decompress `{}`", String::from_utf8_lossy(encoding.as_bytes()));
    let mut response = status_response(Status::unimplemented(message));
    response.headers_mut().insert(GRPC_ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    Some(response)
}

/// The caller's `grpc-timeout`: a `TimeoutValue` of one to eight digits and a `TimeoutUnit` in
/// `H`, `M`, `S`, `m`, `u` or `n`. A value off that grammar is ignored, logged at `debug`, and the
/// call runs without a deadline.
pub(crate) fn grpc_timeout(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(GRPC_TIMEOUT)?;
    let parsed = value.to_str().ok().and_then(parse_timeout);
    if parsed.is_none() {
        tracing::debug!(value = ?value, "a `grpc-timeout` off the specification's grammar is ignored");
    }
    parsed
}

fn parse_timeout(text: &str) -> Option<Duration> {
    let unit_at = text.len().checked_sub(1)?;
    if !text.is_char_boundary(unit_at) {
        return None;
    }
    let (digits, unit) = text.split_at(unit_at);
    if digits.is_empty() || digits.len() > 8 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    Some(match unit {
        "H" => Duration::from_secs(amount * 3600),
        "M" => Duration::from_secs(amount * 60),
        "S" => Duration::from_secs(amount),
        "m" => Duration::from_millis(amount),
        "u" => Duration::from_micros(amount),
        "n" => Duration::from_nanos(amount),
        _ => return None,
    })
}

/// The span for a call to `path`: named `package.Service/Method`, with `rpc.system = "grpc"`,
/// `rpc.service` and `rpc.method`.
fn call_span(path: &str) -> Span {
    let name = path.strip_prefix('/').unwrap_or(path);
    let call_span = span::call(<Grpc as Transport>::KEY, name, None);
    call_span.record(span::RPC_SYSTEM, "grpc");
    if let Some((service, method)) = name.split_once('/') {
        call_span.record(span::RPC_SERVICE, service);
        call_span.record(span::RPC_METHOD, method);
    }
    call_span
}

fn record_status(call_span: &Span, code: Code) {
    call_span.record(span::RPC_GRPC_STATUS_CODE, i32::from(code));
}

/// `users.v1.UserService` for the path `/users.v1.UserService/GetUser`.
fn service_of(path: &str) -> Option<&str> {
    path.strip_prefix('/')?.split_once('/').map(|(service, _)| service)
}

/// The span's `ulo.handler`: `UsersGrpc::get_user`.
struct HandlerName<'a>(&'a MountedHandler<Grpc>);

impl fmt::Display for HandlerName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.0.controller(), self.0.name())
    }
}

/// Fires `CancelReason::ClientCancelled` when the call's future is dropped before it answered: a
/// caller resetting its stream drops it there.
struct CancelOnDrop(Option<ExecutionRef>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(exec) = self.0.take() {
            exec.cancel_with(CancelReason::ClientCancelled);
        }
    }
}

enum Raced {
    /// The pipeline answered first; the sleep, still pending, goes on with the body.
    Done(Response, Option<BoxFuture<'static, ()>>),
    Expired,
}

/// `pipeline` raced against the call's deadline. The pipeline is polled first, so an answer ready
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

/// `fut` raced against the grace, polled first: `None` when the grace runs out, `fut` dropped at
/// its current await. With no grace, `fut` runs to its end.
async fn within<F: Future>(fut: F, grace: Option<&mut BoxFuture<'static, ()>>) -> Option<F::Output> {
    match grace {
        Some(sleep) => {
            tokio::select! {
                biased;
                output = fut => Some(output),
                () = sleep => None,
            }
        }
        None => Some(fut.await),
    }
}

/// The error handlers' reply to a passed deadline, as the response to write when it is a single
/// reply: a trailers-only status, or a body carrying at most one message that ends within the
/// grace. The body is read whole and written as read; one failing while it is read answers
/// DEADLINE_EXCEEDED. `Err` for a stream: a body whose bytes run past its first message, or one
/// still open when the grace runs out, dropped unwritten.
///
/// A message is read off its gRPC length prefix, not off the reply's construction: on the wire a
/// server stream of one message and a unary reply are the same bytes, and `Reply` is an erased
/// response, whether `GrpcCx::reply`, `GrpcCx::reply_stream` or a hand-written body built it.
async fn single(reply: Reply, mut grace: Option<&mut BoxFuture<'static, ()>>) -> Result<Response, ()> {
    let (parts, mut body) = reply.into_parts();
    if body.is_end_stream() {
        return Ok(Response::from_parts(parts, HttpBody::new(body)));
    }
    let mut data = BytesMut::new();
    let mut trailers = None;
    loop {
        let next = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx));
        let Some(frame) = within(next, grace.as_deref_mut()).await.ok_or(())? else { break };
        let frame = match frame {
            Ok(frame) => frame,
            Err(err) => {
                tracing::debug!(error = %err, "the reply an error handler answered a timed-out call with failed while it was read");
                return Ok(status_response(status::deadline_exceeded()));
            }
        };
        match frame.into_data() {
            Ok(bytes) => {
                data.extend_from_slice(&bytes);
                if past_first_message(&data) {
                    return Err(());
                }
            }
            Err(frame) => {
                trailers = frame.into_trailers().ok();
                break;
            }
        }
    }
    let body = Buffered { data: Some(data.freeze()).filter(|data| !data.is_empty()), trailers };
    Ok(Response::from_parts(parts, HttpBody::new(body)))
}

/// Whether `data`, the start of a reply body, holds a byte after its first length-prefixed
/// message: a compressed-flag byte and a four-byte big-endian length, then that many bytes.
fn past_first_message(data: &[u8]) -> bool {
    match data {
        [_, a, b, c, d, rest @ ..] => rest.len() > u32::from_be_bytes([*a, *b, *c, *d]) as usize,
        _ => false,
    }
}

/// A reply body read whole: its data in one frame, then its trailers.
struct Buffered {
    data: Option<Bytes>,
    trailers: Option<HeaderMap>,
}

impl http_body::Body for Buffered {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let this = self.get_mut();
        if let Some(data) = this.data.take() {
            return Poll::Ready(Some(Ok(Frame::data(data))));
        }
        Poll::Ready(this.trailers.take().map(|trailers| Ok(Frame::trailers(trailers))))
    }

    fn is_end_stream(&self) -> bool {
        self.data.is_none() && self.trailers.is_none()
    }
}

/// A reply stream's items with the late path for an `Err` item.
struct LateItems<S> {
    stream: Pin<Box<S>>,
    /// Held while the stream runs, which keeps the execution's instances and its cancellation
    /// signal alive.
    cx: GrpcCx,
    state: Late,
}

enum Late {
    Streaming,
    /// An `Err` item in the error handlers, with the canonical form of the original kept for an
    /// error handler answering `Ok`.
    Recovering { original: CallError, outcome: BoxFuture<'static, LateOutcome> },
    Done,
}

impl<S> LateItems<S> {
    /// The status that ends the stream for `error`, the stream reported cut off.
    fn failed(&self, error: BoxError) -> Status {
        self.cx.exec().report_stream_end(StreamOutcome::CutOff(None));
        status::render(error, self.cx.exec())
    }
}

impl<S> Stream for LateItems<S>
where
    S: Stream,
    S::Item: IntoGrpcItem,
{
    type Item = Result<<S::Item as IntoGrpcItem>::Message, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match &mut this.state {
                Late::Streaming => match ready!(this.stream.as_mut().poll_next(cx)) {
                    Some(item) => match item.into_grpc_item() {
                        Ok(message) => return Poll::Ready(Some(Ok(message))),
                        Err(error) => {
                            let original = error.summary();
                            match this.cx.inner.handler.clone() {
                                Some(handler) => {
                                    let exec = this.cx.exec().clone();
                                    let call_cx = this.cx.clone();
                                    let outcome: BoxFuture<'static, LateOutcome> = Box::pin(async move {
                                        ulo::dispatch_late(&handler, &exec, &call_cx, BoxError::from(error)).await
                                    });
                                    this.state = Late::Recovering { original, outcome };
                                }
                                None => {
                                    this.state = Late::Done;
                                    return Poll::Ready(Some(Err(this.failed(BoxError::from(error)))));
                                }
                            }
                        }
                    },
                    None => {
                        this.state = Late::Done;
                        return Poll::Ready(None);
                    }
                },
                Late::Recovering { original, outcome } => {
                    let error: BoxError = match ready!(outcome.as_mut().poll(cx)) {
                        LateOutcome::End => {
                            this.state = Late::Done;
                            return Poll::Ready(None);
                        }
                        LateOutcome::Render(error) => error,
                        LateOutcome::Ignored => {
                            tracing::warn!(
                                handler = this.cx.inner.handler.as_ref().map(|handler| handler.name()),
                                "an error handler answered `Ok` to an error raised after the reply stream began; its reply is dropped and the original error is written"
                            );
                            Box::new(original.summary())
                        }
                        _ => Box::new(original.summary()),
                    };
                    this.state = Late::Done;
                    return Poll::Ready(Some(Err(this.failed(error))));
                }
                Late::Done => return Poll::Ready(None),
            }
        }
    }
}

/// The outermost reply body: it keeps the execution and the in-flight permit until hyper drops it,
/// carries the deadline a reply answered before, and records the trailers' `grpc-status` on the
/// span. A drop before the body's end is the caller abandoning the call, which fires
/// `CancelReason::ClientCancelled`.
struct CallBody {
    inner: HttpBody,
    exec: ExecutionRef,
    deadline: Option<BoxFuture<'static, ()>>,
    call_span: Span,
    _permit: Permit,
    ended: bool,
}

impl CallBody {
    fn new(inner: HttpBody, exec: ExecutionRef, deadline: Option<BoxFuture<'static, ()>>, call_span: Span, permit: Permit) -> Self {
        let ended = inner.is_end_stream();
        CallBody { inner, exec, deadline, call_span, _permit: permit, ended }
    }

    /// Ends the body with `status` in its trailers, the rest of the reply dropped unwritten.
    fn end_with(&mut self, status: Status) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        self.ended = true;
        self.deadline = None;
        record_status(&self.call_span, status.code());
        drop(std::mem::take(&mut self.inner));
        let mut trailers = HeaderMap::new();
        if let Err(unwritable) = status.add_header(&mut trailers) {
            tracing::warn!(error = %unwritable, "a status could not be written as trailers");
        }
        Poll::Ready(Some(Ok(Frame::trailers(trailers))))
    }
}

impl http_body::Body for CallBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        if let Some(deadline) = this.deadline.as_mut() {
            if deadline.as_mut().poll(cx).is_ready() {
                this.exec.cancel_with(CancelReason::Deadline);
                return this.end_with(status::deadline_exceeded());
            }
        }
        // A reply stream that panics is ended here rather than in hyper's connection task.
        let polled = match catch_unwind(AssertUnwindSafe(|| Pin::new(&mut this.inner).poll_frame(cx))) {
            Ok(polled) => polled,
            Err(_) => {
                tracing::error!("a gRPC reply body panicked while it was written; it ends with INTERNAL");
                this.exec.report_stream_end(StreamOutcome::CutOff(this.exec.cancel_reason()));
                let panicked = std::mem::take(&mut this.inner);
                // A stream that panicked may panic again in its drop.
                let _ = catch_unwind(AssertUnwindSafe(move || drop(panicked)));
                return this.end_with(Status::internal("internal error"));
            }
        };
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(trailers) = frame.trailers_ref() {
                    if let Some(code) = trailers.get(GRPC_STATUS) {
                        record_status(&this.call_span, Code::from_bytes(code.as_bytes()));
                    }
                    this.ended = true;
                } else if this.inner.is_end_stream() {
                    this.ended = true;
                }
            }
            Poll::Ready(Some(Err(_)) | None) => this.ended = true,
            Poll::Pending => {}
        }
        if this.ended {
            this.deadline = None;
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.ended || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for CallBody {
    // Runs before `inner` drops, so a `Tracked` stream inside reports the reason.
    fn drop(&mut self) {
        if !self.ended {
            self.exec.cancel_with(CancelReason::ClientCancelled);
        }
    }
}
