//! A caller's `grpc-timeout` as the call's deadline: DEADLINE_EXCEEDED when it passes unclaimed,
//! the error handlers offered a `Timeout` under `Server::timeout_grace` before that, and
//! DEADLINE_EXCEEDED without them once the grace runs out; the handler's future dropped with the
//! reason `Deadline`; an error handler's single reply delivered and its stream ended with
//! DEADLINE_EXCEEDED, nothing of it written; a deadline passing during a streamed reply; and a
//! `grpc-timeout` off the specification's grammar ignored.
//!
//! The calls go through a plain HTTP/2 client: a tonic `Channel` enforces `grpc-timeout` itself
//! and would answer CANCELLED before the server's answer arrived.

mod support;

use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt, future, stream};
use tonic::{Code, Status};
use ulo::{BoxError, Bound, Dep, ErrorHandler, ExecutionRef, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_codegen_tests::probe::clock_client::ClockClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_transport::{CallError, ErrorKind};

use support::{Capture, PlainHttp2, Record, Running};

/// The deadline the stalling calls are given, and the grace the server gives their error handlers.
const DEADLINE: Duration = Duration::from_millis(200);
const GRACE: Duration = Duration::from_millis(300);

/// Longer than any test runs: a handler sleeping this long is still running at the deadline.
const FOREVER: Duration = Duration::from_secs(30);

const STREAM_WARNING: &str = "an error handler answered a timed-out call with a stream; the stream was ended at the deadline";

/// What the handlers and error handlers observed, in order.
#[derive(Clone)]
struct Seen(Record<String>);

/// Records the execution's cancel reason when the handler's future holding it is dropped.
struct DropProbe {
    exec: ExecutionRef,
    seen: Seen,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.seen.0.push(format!("handler dropped: {:?}", self.exec.cancel_reason()));
    }
}

/// The kind of the error it is offered, recorded, and every error claimed with UNAVAILABLE.
#[injectable]
struct Claim {
    seen: Dep<Seen>,
}

impl ErrorHandler<Grpc> for Claim {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        let kind = err.downcast_ref::<CallError>().map(CallError::kind);
        self.seen.0.push(format!("offered: {kind:?}"));
        Ok(cx.reply_status(Status::unavailable("claimed after the deadline")))
    }
}

/// Reshapes every error into an `Unavailable` `CallError`.
struct Reshape;

impl ErrorHandler<Grpc> for Reshape {
    async fn handle(&self, _err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        Err(BoxError::from(CallError::new(ErrorKind::Unavailable, "reshaped")))
    }
}

/// Takes longer than any grace to answer.
#[injectable]
struct Stuck {
    seen: Dep<Seen>,
}

impl ErrorHandler<Grpc> for Stuck {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        self.seen.0.push("stuck handler started".to_owned());
        tokio::time::sleep(FOREVER).await;
        Err(err)
    }
}

fn timed_out(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().is_some_and(|call| call.kind() == ErrorKind::Timeout)
}

/// One `Tick { n: 7 }`.
struct OneOnTimeout;

impl ErrorHandler<Grpc> for OneOnTimeout {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply(Tick { n: 7 }))
    }
}

/// Ticks without end.
struct EndlessOnTimeout;

impl ErrorHandler<Grpc> for EndlessOnTimeout {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::repeat_with(|| Ok::<_, CallError>(Tick { n: 1 }))))
    }
}

/// One tick, then nothing.
struct TrickleOnTimeout;

impl ErrorHandler<Grpc> for TrickleOnTimeout {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::once(future::ready(Ok::<_, CallError>(Tick { n: 1 }))).chain(stream::pending())))
    }
}

#[injectable]
struct ClockService {
    seen: Dep<Seen>,
}

impl ClockService {
    async fn stall(&self, exec: ExecutionRef) -> Tick {
        let _probe = DropProbe { exec, seen: (*self.seen).clone() };
        tokio::time::sleep(FOREVER).await;
        Tick { n: 0 }
    }
}

#[routes]
impl ClockService {
    #[ulo_grpc::method(probe::clock::Stall)]
    async fn stall_unclaimed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallClaimed)]
    #[error_handlers(Claim)]
    async fn stall_claimed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallReshaped)]
    #[error_handlers(value = Reshape)]
    async fn stall_reshaped(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallStuck)]
    #[error_handlers(Stuck)]
    async fn stall_stuck(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    /// The time left before the call's deadline, in whole milliseconds rounded up; `0` for a call
    /// without one.
    #[ulo_grpc::method(probe::clock::Quick)]
    fn quick(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        let left = exec.deadline().map_or(0, |deadline| {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
            u32::try_from(left.as_millis()).unwrap_or(u32::MAX).saturating_add(1)
        });
        Tick { n: left }
    }

    #[ulo_grpc::method(probe::clock::StallAnswered)]
    #[error_handlers(value = OneOnTimeout)]
    async fn stall_answered(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallStreamed)]
    #[error_handlers(value = EndlessOnTimeout)]
    async fn stall_streamed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        let _ = self.stall(exec).await;
        stream::empty()
    }

    #[ulo_grpc::method(probe::clock::StallTrickled)]
    #[error_handlers(value = TrickleOnTimeout)]
    async fn stall_trickled(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        let _ = self.stall(exec).await;
        stream::empty()
    }

    /// One tick, then nothing until the reply is ended.
    #[ulo_grpc::method(probe::clock::Drip)]
    fn drip(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> {
        stream::once(async { Ok(Tick { n: 1 }) }).chain(stream::pending())
    }
}

struct Root {
    seen: Seen,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.seen.clone());
        m.provide::<Claim>();
        m.provide::<Stuck>();
        m.controller::<ClockService>();
    }
}

async fn start() -> (Running, ClockClient<PlainHttp2>, Seen) {
    let seen = Seen(Record::new());
    let server = ulo_grpc::Server::new("127.0.0.1:0").timeout_grace(Bound::After(GRACE));
    let app = Running::start(Root { seen: seen.clone() }, server).await;
    let (http2, origin) = app.plain_http2();
    (app, ClockClient::with_origin(http2, origin), seen)
}

fn timed(timeout: Duration) -> tonic::Request<Ticks> {
    let mut request = tonic::Request::new(Ticks::default());
    request.set_timeout(timeout);
    request
}

fn failed<T: std::fmt::Debug>(reply: Result<tonic::Response<T>, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => panic!("expected the call to fail, got {:?}", reply.into_inner()),
    }
}

#[tokio::test]
async fn an_unclaimed_deadline_answers_deadline_exceeded_and_drops_the_handler_as_deadline() {
    let (app, mut client, seen) = start().await;
    let status = failed(support::within("the stalled call", client.stall(timed(DEADLINE))).await);
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert_eq!(seen.0.at_least(1, "the handler's drop").await, vec!["handler dropped: Some(Deadline)".to_owned()]);
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_claiming_the_timeout_within_the_grace_is_the_reply() {
    let (app, mut client, seen) = start().await;
    let status = failed(support::within("the stalled call", client.stall_claimed(timed(DEADLINE))).await);
    assert_eq!((status.code(), status.message()), (Code::Unavailable, "claimed after the deadline"));
    let observed = seen.0.at_least(2, "the drop and the error handler").await;
    assert_eq!(observed, vec!["handler dropped: Some(Deadline)".to_owned(), "offered: Some(Timeout)".to_owned()]);
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_reshaping_the_timeout_is_rendered_as_it_stands() {
    let (app, mut client, _) = start().await;
    let status = failed(support::within("the stalled call", client.stall_reshaped(timed(DEADLINE))).await);
    assert_eq!((status.code(), status.message()), (Code::Unavailable, "reshaped"));
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_past_the_grace_is_dropped_for_deadline_exceeded() {
    let (app, mut client, seen) = start().await;
    let began = Instant::now();
    let status = failed(support::within("the stalled call", client.stall_stuck(timed(DEADLINE))).await);
    let took = began.elapsed();
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(seen.0.snapshot().contains(&"stuck handler started".to_owned()), "the error handler was never offered the timeout");
    assert!(took >= DEADLINE + GRACE, "answered after {took:?}, before the deadline and the grace had passed");
    assert!(took < DEADLINE + GRACE + Duration::from_secs(1), "answered after {took:?}: the grace did not bound the error handler");
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_answering_one_message_within_the_grace_is_the_reply() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, mut client, _) = start().await;
    let reply = support::within("the stalled call", client.stall_answered(timed(DEADLINE))).await;
    assert_eq!(reply.map(|reply| reply.into_inner().n).map_err(|status| status.code()), Ok(7));
    let warned = capture.warnings();
    assert!(!warned.iter().any(|line| line.contains(STREAM_WARNING)), "a single reply was logged as a stream: {warned:?}");
    app.stop().await;
}

/// The failure a server-streaming call ended with: its status when the call failed outright, or
/// the panic naming what the stream carried instead.
async fn stream_failed(reply: Result<tonic::Response<tonic::Streaming<Tick>>, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => {
            let first = support::within("the first item", reply.into_inner().next()).await;
            panic!("expected the call to fail before any item, got a stream whose first item is {first:?}")
        }
    }
}

#[tokio::test]
async fn an_error_handler_answering_a_stream_is_ended_at_once_with_deadline_exceeded() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, mut client, _) = start().await;
    let began = Instant::now();
    let reply = support::within("the stalled call", client.stall_streamed(timed(DEADLINE))).await;
    let status = stream_failed(reply).await;
    let took = began.elapsed();
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(took < DEADLINE + GRACE, "answered after {took:?}, not at once after the {DEADLINE:?} deadline");
    let warned = capture.warnings();
    assert!(
        warned.iter().any(|line| line.contains(STREAM_WARNING) && line.contains("path=/codegen.probe.v1.Clock/StallStreamed")),
        "the server logged no warning naming the path: {warned:?}"
    );
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_reply_still_open_at_the_end_of_the_grace_is_ended_with_deadline_exceeded() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, mut client, _) = start().await;
    let began = Instant::now();
    let reply = support::within("the stalled call", client.stall_trickled(timed(DEADLINE))).await;
    let status = stream_failed(reply).await;
    let took = began.elapsed();
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(took >= DEADLINE + GRACE, "answered after {took:?}, before the deadline and the grace had passed");
    assert!(took < DEADLINE + GRACE + Duration::from_secs(1), "answered after {took:?}: the grace did not bound the reply");
    let warned = capture.warnings();
    assert!(warned.iter().any(|line| line.contains(STREAM_WARNING)), "the server logged no warning: {warned:?}");
    app.stop().await;
}

#[tokio::test]
async fn a_call_answered_before_its_deadline_reads_the_deadline_it_was_given() {
    let (app, mut client, _) = start().await;
    let reply = client.quick(timed(Duration::from_secs(10))).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    let left = reply.into_inner().n;
    assert!((1..=10_000).contains(&left), "{left} ms left of a 10 s deadline");
    let reply = client.quick(Ticks::default()).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert_eq!(reply.into_inner().n, 0, "a call without `grpc-timeout` has no deadline");
    app.stop().await;
}

#[tokio::test]
async fn a_grpc_timeout_off_the_grammar_is_ignored() {
    let (app, mut client, _) = start().await;
    // No unit, nine digits, an unknown unit, a negative amount.
    for value in ["10", "123456789S", "10s", "-1S"] {
        let mut request = tonic::Request::new(Ticks::default());
        request.metadata_mut().insert("grpc-timeout", value.parse().unwrap_or_else(|error| panic!("bad header {value}: {error}")));
        let reply = client.quick(request).await.unwrap_or_else(|status| panic!("Quick with `{value}` failed: {status:?}"));
        assert_eq!(reply.into_inner().n, 0, "`grpc-timeout: {value}` gave the call a deadline");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_deadline_passing_during_a_streamed_reply_ends_it_with_deadline_exceeded() {
    let (app, mut client, _) = start().await;
    let reply = client.drip(timed(Duration::from_millis(300))).await.unwrap_or_else(|status| panic!("Drip failed: {status:?}"));
    let mut items = reply.into_inner();
    let first = support::within("the first tick", items.next()).await;
    assert_eq!(first.map(|item| item.map(|tick| tick.n).map_err(|status| status.code())), Some(Ok(1)));
    let end = support::within("the reply's end", items.next()).await;
    match end {
        Some(Err(status)) => assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}"),
        other => panic!("expected the reply to end with DEADLINE_EXCEEDED, got {other:?}"),
    }
    app.stop().await;
}
