//! Per-delivery execution (transports DESIGN §5.2): one execution per call, `deadline-ms` as its
//! deadline, `cancel` as `ClientCancelled`, the four shapes, `dispatch_late` for an item's error,
//! and the split per `Capabilities` between the whole frame and the payload with native
//! correlation. `Server::serve` runs it for every delivery the link's inbound stream yields.
//!
//! The serve loop routes every frame in arrival order before anything awaits: a `req`, `evt` or
//! `open` opens its execution and registers its id there, so an `in`, `in_end` or `cancel` the
//! link delivers right after it always finds the call; the call itself then runs in its own task.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinSet;
use tracing::{Instrument, Span};
use ulo::{
    AppHandle, BoxError, BoxFuture, CancelReason, EndStream, ExecOptions, Execution, ExecutionRef, LateOutcome,
    MountedHandler, Shape, Timer, Transport,
};
use ulo_transport::{Admission, CallError, Detail, Details, ErrorKind, Permit, Tracked, span};

use crate::__private::{HandlerFn, Kind};
use crate::codec::Codec;
use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{Ack, Capabilities, Delivery, DeliveryMode, FrameTooLarge, Pattern, ReplyPath};
use crate::transport::{Body, CallHeaders, CxInner, LinkInfo, NoHandler, Reply, Rpc, RpcCx};

/// The `ErrorInfo` domain of every reason this transport writes.
pub(crate) const DOMAIN: &str = "ulo.rpc";

/// What `Server::prepare` built, read by every delivery.
pub(crate) struct Shared {
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    pub(crate) routes: HashMap<String, Route>,
    pub(crate) codec: Codec,
    pub(crate) capabilities: Capabilities,
    /// `Link::NAME`.
    pub(crate) link: &'static str,
    pub(crate) admission: Admission,
    pub(crate) calls: Mutex<HashMap<u64, Live>>,
    pub(crate) serial: AtomicU64,
    /// Events no handler took, counted for the log line each one writes.
    pub(crate) unhandled_events: AtomicU64,
}

/// One pattern's handler.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) handler: MountedHandler<Rpc>,
    pub(crate) call: HandlerFn,
    pub(crate) kind: Kind,
    pub(crate) shape: Shape,
}

/// A call in flight, by its id: what a `cancel` cancels and where an `in` goes.
pub(crate) struct Live {
    /// Tells this registration from a later one under the same id.
    serial: u64,
    exec: ExecutionRef,
    /// `None` once `in_end` arrived, and for a call whose request is not streamed.
    inbound: Option<UnboundedSender<Data>>,
}

impl Shared {
    fn calls(&self) -> MutexGuard<'_, HashMap<u64, Live>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Which frame opened a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrived {
    Req,
    Evt,
    Open,
}

struct Incoming {
    id: Option<u64>,
    arrived: Arrived,
    pattern: String,
    headers: CallHeaders,
    body: Body,
    feed: Option<UnboundedSender<Data>>,
}

/// Routes one delivery, in arrival order: a call is opened and spawned into `tasks`; a frame for a
/// call in flight reaches it at once.
pub(crate) fn accept(shared: &Arc<Shared>, delivery: Delivery, tasks: &mut JoinSet<()>) {
    let Delivery { frame, reply, ack } = delivery;
    match frame {
        Frame::Req { id, pattern, headers, data } => {
            let incoming = Incoming { id: Some(id), arrived: Arrived::Req, pattern, headers, body: Body::Data(data), feed: None };
            start(shared, incoming, reply, ack, tasks);
        }
        Frame::Evt { pattern, headers, data } => {
            let incoming = Incoming { id: None, arrived: Arrived::Evt, pattern, headers, body: Body::Data(data), feed: None };
            start(shared, incoming, reply, ack, tasks);
        }
        Frame::Open { id, pattern, headers } => {
            // Unbounded: the reserved `credit` frame is the window a bound needs, and blocking here
            // would stall every call on the link behind one slow handler.
            let (feed, items) = mpsc::unbounded_channel();
            let incoming =
                Incoming { id: Some(id), arrived: Arrived::Open, pattern, headers, body: Body::Stream(items), feed: Some(feed) };
            start(shared, incoming, reply, ack, tasks);
        }
        Frame::In { id, data } => {
            let feed = shared.calls().get(&id).and_then(|live| live.inbound.clone());
            if let Some(feed) = feed {
                let _ = feed.send(data);
            }
            ack.ack();
        }
        Frame::InEnd { id } => {
            if let Some(live) = shared.calls().get_mut(&id) {
                live.inbound = None;
            }
            ack.ack();
        }
        Frame::Cancel { id } => {
            let reason = if reply.is_some() { CancelReason::ClientCancelled } else { CancelReason::Disconnected };
            let exec = shared.calls().get(&id).map(|live| live.exec.clone());
            if let Some(exec) = exec {
                exec.cancel_with(reason);
            }
            ack.ack();
        }
        other => {
            tracing::debug!(kind = other.kind(), link = shared.link, "a frame the server does not take arrived and was dropped");
            ack.ack();
        }
    }
}

fn start(shared: &Arc<Shared>, incoming: Incoming, reply: Option<ReplyPath>, ack: Ack, tasks: &mut JoinSet<()>) {
    let link = LinkInfo { name: shared.link, peer: reply.as_ref().and_then(ReplyPath::peer_addr) };
    // An event's reply path carries its peer and nothing else.
    let reply = if incoming.arrived == Arrived::Evt { None } else { reply };
    let Some(route) = shared.routes.get(incoming.pattern.as_str()) else {
        return unhandled(shared, incoming, link, reply, ack, tasks);
    };
    let answered = incoming.id.zip(reply);
    let Some(permit) = shared.admission.try_admit() else {
        match answered {
            Some((id, reply)) => {
                let error = refusal("the server is over its in-flight limit").with(Detail::RetryAfter(shared.admission.shed_retry_after()));
                tasks.spawn(send_error(reply, id, error));
            }
            // Left unsettled, so a broker redelivers it rather than dropping it.
            None => tracing::warn!(pattern = incoming.pattern.as_str(), link = shared.link, "an event over the in-flight limit was left unacknowledged"),
        }
        return;
    };
    let deadline = deadline_of(&incoming.headers);
    let opts = match deadline {
        Some(after) => ExecOptions::new().deadline(shared.timer.now() + after),
        None => ExecOptions::new(),
    };
    let Ok(exec) = Execution::open(route.handler.module(), opts) else {
        if let Some((id, reply)) = answered {
            tasks.spawn(send_error(reply, id, refusal("the server is draining")));
        }
        return;
    };
    exec.seed(incoming.headers.clone());
    exec.seed(link.clone());
    let handle = exec.handle();
    let registered = incoming.id.map(|id| {
        let serial = shared.serial.fetch_add(1, Ordering::Relaxed);
        shared.calls().insert(id, Live { serial, exec: handle.clone(), inbound: incoming.feed });
        Registered { shared: Arc::clone(shared), id, serial }
    });
    let cx = context(shared, handle, &incoming.pattern, incoming.headers, link, incoming.body);
    let call_span = call_span(shared, &incoming.pattern, Some(&route.handler));
    let call = Call {
        shared: Arc::clone(shared),
        route: route.clone(),
        exec,
        cx,
        arrived: incoming.arrived,
        answered,
        ack,
        deadline,
        _registered: registered,
        _permit: permit,
    };
    tasks.spawn(call.run().instrument(call_span));
}

fn context(shared: &Shared, exec: ExecutionRef, pattern: &str, headers: CallHeaders, link: LinkInfo, body: Body) -> RpcCx {
    RpcCx::new(CxInner {
        exec,
        pattern: Pattern::from(pattern),
        headers,
        link,
        app: shared.app.clone(),
        timer: Arc::clone(&shared.timer),
        codec: shared.codec,
        body: Mutex::new(Some(body)),
    })
}

/// A request or streamed request naming a pattern nothing handles: the global error handlers
/// receive `Unavailable` whose source is [`NoHandler`], answered `reason: "pattern_unhandled"`
/// when none claims it. An event is logged, counted and rejected, so a broker cannot loop it.
fn unhandled(shared: &Arc<Shared>, incoming: Incoming, link: LinkInfo, reply: Option<ReplyPath>, ack: Ack, tasks: &mut JoinSet<()>) {
    let Some((id, reply)) = incoming.id.zip(reply).filter(|_| incoming.arrived != Arrived::Evt) else {
        let count = shared.unhandled_events.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(pattern = incoming.pattern.as_str(), link = shared.link, unhandled = count, "an event no handler takes was rejected");
        ack.reject();
        return;
    };
    let Ok(exec) = Execution::open(&shared.app.root(), ExecOptions::new()) else {
        tasks.spawn(send_error(reply, id, refusal("the server is draining")));
        return;
    };
    exec.seed(incoming.headers.clone());
    exec.seed(link.clone());
    let error = CallError::new(ErrorKind::Unavailable, format!("no handler for pattern `{}`", incoming.pattern))
        .with_details(Details::new().with(reason("pattern_unhandled")))
        .with_source(NoHandler::new(&incoming.pattern));
    let cx = context(shared, exec.handle(), &incoming.pattern, incoming.headers, link, incoming.body);
    let shared = Arc::clone(shared);
    let call_span = call_span(&shared, cx.pattern(), None);
    tasks.spawn(
        async move {
            let handle = exec.handle();
            let outcome = ulo::recover::<Rpc>(None, &handle, &cx, BoxError::from(error)).await;
            answer(&shared, None, &handle, &cx, &reply, id, Ended::Done(outcome), &mut None).await;
            ack.ack();
            drop(exec);
        }
        .instrument(call_span),
    );
}

/// One call, run in its own task.
struct Call {
    shared: Arc<Shared>,
    route: Route,
    /// Held for the call's length: the execution ends when it and every handle drop.
    exec: Execution,
    cx: RpcCx,
    arrived: Arrived,
    /// The id and reply path of a call that answers; `None` for an event.
    answered: Option<(u64, ReplyPath)>,
    ack: Ack,
    deadline: Option<Duration>,
    _registered: Option<Registered>,
    _permit: Permit,
}

impl Call {
    async fn run(self) {
        let Call { shared, route, exec, cx, arrived, answered, ack, deadline, _registered, _permit } = self;
        let handle = exec.handle();
        let mut deadline = deadline.map(|after| shared.timer.sleep(after));
        let outcome = match mismatch(arrived, &route, cx.pattern()) {
            Some(text) => {
                let error = BoxError::from(CallError::new(ErrorKind::BadRequest, text));
                Ended::Done(ulo::recover(Some(&route.handler), &handle, &cx, error).await)
            }
            None => {
                let call = Arc::clone(&route.call);
                let dispatching = ulo::dispatch(&route.handler, &handle, &cx, move |cx| (*call)(cx));
                until_ended(&handle, &mut deadline, dispatching).await
            }
        };
        match answered {
            Some((id, reply)) => {
                // A request to an event handler is answered with an empty `res` once it ran.
                let outcome = match outcome {
                    Ended::Done(Ok(_)) if route.kind == Kind::Event => Ended::Done(Ok(Reply::None)),
                    outcome => outcome,
                };
                answer(&shared, Some(&route.handler), &handle, &cx, &reply, id, outcome, &mut deadline).await;
                ack.ack();
            }
            None => settle_event(&shared, cx.pattern(), &handle, outcome, ack),
        }
        drop(exec);
    }
}

/// A frame that does not fit the handler's shape: `req` or `evt` to a streamed-request pattern,
/// `open` to one that takes one payload or to an event handler.
fn mismatch(arrived: Arrived, route: &Route, pattern: &str) -> Option<String> {
    let streamed = matches!(route.shape, Shape::ClientStreaming | Shape::Bidi);
    match arrived {
        Arrived::Req | Arrived::Evt if streamed => Some(format!("pattern `{pattern}` takes a streamed request; open it with `open`")),
        Arrived::Open if !streamed || route.kind == Kind::Event => {
            Some(format!("pattern `{pattern}` takes one payload; send it with `req` or `evt`"))
        }
        _ => None,
    }
}

/// An event's acknowledgment once its handler completed: acknowledged when it succeeded, rejected
/// without requeue when it failed or its deadline passed, and left unsettled when the call was
/// cancelled otherwise, so a broker redelivers it.
fn settle_event(shared: &Shared, pattern: &str, exec: &ExecutionRef, outcome: Ended<Result<Reply, BoxError>>, ack: Ack) {
    match outcome {
        Ended::Done(Ok(_)) => ack.ack(),
        Ended::Done(Err(err)) => {
            let error = render(err, exec);
            tracing::warn!(pattern, link = shared.link, kind = %error.kind, message = %error.message, "an event's handler failed; the event was rejected");
            ack.reject();
        }
        Ended::Cancelled(Some(CancelReason::Deadline)) => ack.reject(),
        Ended::Cancelled(_) => {}
    }
}

/// Writes a call's outcome on its reply path.
#[allow(clippy::too_many_arguments)]
async fn answer(
    shared: &Shared,
    handler: Option<&MountedHandler<Rpc>>,
    exec: &ExecutionRef,
    cx: &RpcCx,
    reply: &ReplyPath,
    id: u64,
    outcome: Ended<Result<Reply, BoxError>>,
    deadline: &mut Option<BoxFuture<'static, ()>>,
) {
    match outcome {
        Ended::Done(Ok(Reply::None)) => {
            send(reply, exec, id, Frame::Res { id, data: Data::default() }).await;
        }
        Ended::Done(Ok(Reply::One(data))) => {
            send(reply, exec, id, Frame::Res { id, data }).await;
        }
        Ended::Done(Ok(Reply::Many(stream))) => stream_reply(shared, handler, exec, cx, reply, id, stream, deadline).await,
        Ended::Done(Err(err)) => {
            send(reply, exec, id, Frame::Err { id, error: render(err, exec) }).await;
        }
        Ended::Cancelled(reason) => cancelled(reply, exec, id, reason).await,
    }
}

/// A streamed reply: every `Ok` item as `item`, the clean end as `end`. An `Err` item runs the
/// error handlers on the late path and is written as `err`, which ends the stream, unless they
/// answer `EndStream`. The stream is dropped once its end is written, which reports it.
#[allow(clippy::too_many_arguments)]
async fn stream_reply(
    shared: &Shared,
    handler: Option<&MountedHandler<Rpc>>,
    exec: &ExecutionRef,
    cx: &RpcCx,
    reply: &ReplyPath,
    id: u64,
    mut stream: Tracked<BoxStream<'static, Result<Data, BoxError>>>,
    deadline: &mut Option<BoxFuture<'static, ()>>,
) {
    if !shared.capabilities.shapes.iter().any(|shape| matches!(shape, Shape::ServerStreaming | Shape::Bidi)) {
        let error = ErrorBody::new(ErrorKind::Internal, format!("the {} link carries no streamed reply", shared.link), Details::new());
        send(reply, exec, id, Frame::Err { id, error }).await;
        return;
    }
    loop {
        match until_ended(exec, deadline, stream.next()).await {
            Ended::Done(Some(Ok(data))) => {
                if !send(reply, exec, id, Frame::Item { id, data }).await {
                    return;
                }
            }
            Ended::Done(Some(Err(err))) => {
                let frame = match late(handler, exec, cx, err).await {
                    Some(error) => Frame::Err { id, error },
                    None => Frame::End { id },
                };
                send(reply, exec, id, frame).await;
                return;
            }
            Ended::Done(None) => {
                send(reply, exec, id, Frame::End { id }).await;
                return;
            }
            Ended::Cancelled(reason) => return cancelled(reply, exec, id, reason).await,
        }
    }
}

/// An item's error through `dispatch_late`: the error to write, or `None` to end the stream
/// cleanly on `EndStream`. With no handler, a miss's recovered stream, it renders as it stands.
async fn late(handler: Option<&MountedHandler<Rpc>>, exec: &ExecutionRef, cx: &RpcCx, err: BoxError) -> Option<ErrorBody> {
    let Some(handler) = handler else {
        return (!err.is::<EndStream>()).then(|| render(err, exec));
    };
    // `dispatch_late` takes the error by value; the original is kept to write when an error
    // handler answers `Ok`, which on the late path is ignored.
    let original = match err.downcast_ref::<CallError>() {
        Some(call) => call.summary(),
        None => CallError::new(ErrorKind::Internal, "internal error"),
    };
    match ulo::dispatch_late(handler, exec, cx, err).await {
        LateOutcome::End => None,
        LateOutcome::Render(err) => Some(render(err, exec)),
        LateOutcome::Ignored => {
            tracing::warn!(
                handler = handler.name(),
                "an error handler answered `Ok` to an error raised after the reply stream began; its reply is dropped and the original error is written"
            );
            Some(body_of(&original))
        }
        _ => Some(body_of(&original)),
    }
}

/// What a cancelled call writes: nothing to a caller that left or cancelled, `timeout` at the
/// deadline, `unavailable` at the drain's end or an explicit cancel.
async fn cancelled(reply: &ReplyPath, exec: &ExecutionRef, id: u64, reason: Option<CancelReason>) {
    let error = match reason {
        Some(CancelReason::ClientCancelled | CancelReason::Disconnected) => return,
        Some(CancelReason::Deadline) => timed_out(),
        _ => ErrorBody::new(ErrorKind::Unavailable, "the call was cancelled", Details::new()),
    };
    send(reply, exec, id, Frame::Err { id, error }).await;
}

/// Sends one frame; `false` when it did not reach the caller. A frame over the link's limit is
/// replaced by an `err` of kind `internal`; any other failure means the caller is gone, and the
/// call is cancelled `Disconnected`.
async fn send(reply: &ReplyPath, exec: &ExecutionRef, id: u64, frame: Frame) -> bool {
    match reply.send(frame).await {
        Ok(()) => true,
        Err(err) if err.is::<FrameTooLarge>() => {
            let error = ErrorBody::new(ErrorKind::Internal, "the reply is larger than the link carries", Details::new());
            if reply.send(Frame::Err { id, error }).await.is_err() {
                exec.cancel_with(CancelReason::Disconnected);
            }
            false
        }
        Err(err) => {
            tracing::debug!(error = %err, "a reply frame could not be sent; the caller is gone");
            exec.cancel_with(CancelReason::Disconnected);
            false
        }
    }
}

/// A refusal written without an execution: over the in-flight limit, or once the drain began.
async fn send_error(reply: ReplyPath, id: u64, error: ErrorBody) {
    if let Err(err) = reply.send(Frame::Err { id, error }).await {
        tracing::debug!(error = %err, "a refusal could not be sent; the caller is gone");
    }
}

fn refusal(message: &str) -> ErrorBody {
    ErrorBody::new(ErrorKind::Unavailable, message, Details::new())
}

impl ErrorBody {
    fn with(mut self, detail: Detail) -> Self {
        self.details.push(detail);
        self
    }
}

/// An error no handler claimed, as the `err` frame's `e`: `timeout` when the call's deadline
/// passed, otherwise `CallError::from_boxed`'s kind, message and details.
pub(crate) fn render(err: BoxError, exec: &ExecutionRef) -> ErrorBody {
    if exec.cancel_reason() == Some(CancelReason::Deadline) {
        return timed_out();
    }
    body_of(&CallError::from_boxed(err))
}

fn body_of(error: &CallError) -> ErrorBody {
    ErrorBody::new(error.kind(), error.message(), error.details().clone())
}

fn timed_out() -> ErrorBody {
    ErrorBody::new(ErrorKind::Timeout, "the call's deadline passed", Details::new())
}

/// The `ErrorInfo` detail carrying `reason`, under this transport's domain.
pub(crate) fn reason(reason: &str) -> Detail {
    Detail::ErrorInfo { reason: reason.to_owned(), domain: DOMAIN.to_owned(), metadata: Default::default() }
}

/// `deadline-ms`, the remaining time in milliseconds. A value that does not parse sets none.
fn deadline_of(headers: &CallHeaders) -> Option<Duration> {
    headers.get("deadline-ms")?.trim().parse::<u64>().ok().map(Duration::from_millis)
}

fn call_span(shared: &Shared, pattern: &str, handler: Option<&MountedHandler<Rpc>>) -> Span {
    let name = handler.map(|handler| format!("{}::{}", handler.controller(), handler.name()));
    let call_span = span::call(<Rpc as Transport>::KEY, pattern, name.as_deref());
    call_span.record(span::RPC_SYSTEM, "ulo");
    call_span.record(span::RPC_METHOD, pattern);
    // TCP and UDP record none: `messaging.system` names a broker.
    if shared.capabilities.delivery != DeliveryMode::Addressed {
        call_span.record(span::MESSAGING_SYSTEM, shared.link);
    }
    call_span
}

/// A call's registration, removed when the call ends.
struct Registered {
    shared: Arc<Shared>,
    id: u64,
    serial: u64,
}

impl Drop for Registered {
    fn drop(&mut self) {
        let mut calls = self.shared.calls();
        if calls.get(&self.id).is_some_and(|live| live.serial == self.serial) {
            calls.remove(&self.id);
        }
    }
}

/// How a raced future ended: done, or the execution cancelled first, its deadline included.
enum Ended<T> {
    Done(T),
    Cancelled(Option<CancelReason>),
}

/// `fut` raced against the execution's cancellation and its deadline. A deadline that passes
/// cancels the execution `Deadline`, so every later race ends on the cancellation, which is
/// polled first, and the expired sleep is never polled again.
async fn until_ended<F: Future>(exec: &ExecutionRef, deadline: &mut Option<BoxFuture<'static, ()>>, fut: F) -> Ended<F::Output> {
    tokio::select! {
        biased;
        () = exec.cancelled() => Ended::Cancelled(exec.cancel_reason()),
        out = fut => Ended::Done(out),
        () = expiry(deadline) => {
            exec.cancel_with(CancelReason::Deadline);
            Ended::Cancelled(Some(CancelReason::Deadline))
        }
    }
}

async fn expiry(deadline: &mut Option<BoxFuture<'static, ()>>) {
    match deadline {
        Some(sleep) => sleep.await,
        None => std::future::pending().await,
    }
}
