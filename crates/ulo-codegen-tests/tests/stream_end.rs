//! `on_stream_end` on gRPC reports the reply the dispatcher writes, decided by its trailers:
//! `Completed` for `grpc-status: 0`, `CutOff` for any other status. A stream an interceptor
//! discards was never the reply and reports nothing, so it cannot hide the outcome of the reply
//! that replaced it; a stream the error handlers answer after the deadline was the reply, dropped
//! unwritten, and reports `CutOff(Deadline)`.
//!
//! Each handler registers a callback that sends the outcome on a channel the test holds the other
//! end of. The callback owns the channel's only sender, so the channel closes when the call's
//! execution ends: a test reads `None` for a stream that reported nothing, with no wait for a
//! report that might still come. The calls go through a plain HTTP/2 client, which leaves the
//! deadline to the server.

mod support;

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::mpsc;
use tonic::Code;
use ulo::{BoxError, Bound, CancelReason, ErrorHandler, ExecutionRef, Interceptor, Module, ModuleDef, ModuleIdentity, Next, StreamOutcome, injectable, routes};
use ulo_codegen_tests::probe::ends_client::EndsClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_transport::{CallError, ErrorKind};

use support::{PlainHttp2, Running};

const DISCARDED: &str = "discarded";
const REPLACED: &str = "replaced";
const WRITTEN: &str = "written";
const FAILED: &str = "failed";
const EXPIRED: &str = "expired";
const ABANDONED: &str = "abandoned";

/// The deadline the expiring call is given, and the grace the server gives its error handlers.
const DEADLINE: Duration = Duration::from_millis(200);
const GRACE: Duration = Duration::from_millis(300);

/// How long a test waits for the call's execution to end; it ends once the reply is written.
const PATIENCE: Duration = Duration::from_secs(5);

/// The sender for each method's call, taken by the handler that reports for it.
fn outcomes() -> MutexGuard<'static, HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>> {
    static OUTCOMES: OnceLock<Mutex<HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>>> = OnceLock::new();
    OUTCOMES.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers the callback that reports `method`'s outcome, owning the channel's one sender.
fn report_for(method: &'static str, exec: &ExecutionRef) {
    let sender = outcomes().remove(method).expect("each method is called once");
    exec.on_stream_end(move |outcome| {
        let _ = sender.send(outcome);
    });
}

fn ticks() -> impl Stream<Item = Result<Tick, CallError>> {
    stream::iter([Ok(Tick { n: 1 }), Ok(Tick { n: 2 })])
}

/// Runs the call, drops the stream it answers and answers one message instead.
struct AnswerOne;

impl Interceptor<Grpc> for AnswerOne {
    async fn intercept(&self, cx: &GrpcCx, next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(cx.reply(Tick { n: 0 }))
    }
}

/// Runs the call, drops the stream it answers and answers a stream of its own.
struct AnswerStream;

impl Interceptor<Grpc> for AnswerStream {
    async fn intercept(&self, cx: &GrpcCx, next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(cx.reply_stream(stream::iter([Ok::<_, CallError>(Tick { n: 7 }), Ok(Tick { n: 8 })])))
    }
}

/// Answers the `Timeout` a passed deadline offers with a stream without end.
struct EndlessOnTimeout;

impl ErrorHandler<Grpc> for EndlessOnTimeout {
    async fn handle(&self, _err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        Ok(cx.reply_stream(stream::repeat_with(|| Ok::<_, CallError>(Tick { n: 1 }))))
    }
}

#[injectable]
struct EndsService;

#[routes]
impl EndsService {
    #[ulo_grpc::method(probe::ends::Discarded)]
    #[interceptors(value = AnswerOne)]
    fn discarded(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        report_for(DISCARDED, &exec);
        ticks()
    }

    #[ulo_grpc::method(probe::ends::Replaced)]
    #[interceptors(value = AnswerStream)]
    fn replaced(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        report_for(REPLACED, &exec);
        ticks()
    }

    #[ulo_grpc::method(probe::ends::Written)]
    fn written(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        report_for(WRITTEN, &exec);
        ticks()
    }

    /// One tick, then an item's error, which ends the reply in non-zero trailers.
    #[ulo_grpc::method(probe::ends::Failed)]
    fn failed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        report_for(FAILED, &exec);
        stream::iter([Ok(Tick { n: 1 }), Err(CallError::new(ErrorKind::Unavailable, "the ticks ran out"))])
    }

    #[ulo_grpc::method(probe::ends::Expired)]
    #[error_handlers(value = EndlessOnTimeout)]
    async fn expired(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        report_for(EXPIRED, &exec);
        tokio::time::sleep(Duration::from_secs(30)).await;
        stream::empty()
    }

    /// One tick, then nothing until the caller goes away.
    #[ulo_grpc::method(probe::ends::Abandoned)]
    fn abandoned(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        report_for(ABANDONED, &exec);
        stream::once(async { Ok(Tick { n: 1 }) }).chain(stream::pending())
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<EndsService>();
    }
}

/// The ticks a server-streaming reply delivered, and the status it ended with: `Ok` for
/// `grpc-status: 0`.
type Delivered = (Vec<u32>, Code);

/// Calls `method` and reads its reply to the end; answers what it delivered and what its
/// execution reported once it ended, `None` for nothing.
async fn call<F, Fut>(method: &'static str, timeout: Option<Duration>, send: F) -> (Delivered, Option<StreamOutcome>)
where
    F: FnOnce(EndsClient<PlainHttp2>, tonic::Request<Ticks>) -> Fut,
    Fut: Future<Output = Result<tonic::Response<tonic::Streaming<Tick>>, tonic::Status>>,
{
    let (sender, mut reported) = mpsc::unbounded_channel();
    outcomes().insert(method, sender);
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0").timeout_grace(Bound::After(GRACE))).await;
    let (http2, origin) = app.plain_http2();
    let mut request = tonic::Request::new(Ticks::default());
    if let Some(timeout) = timeout {
        request.set_timeout(timeout);
    }
    let delivered = match support::within("the call", send(EndsClient::with_origin(http2, origin), request)).await {
        Err(status) => (Vec::new(), status.code()),
        Ok(reply) => {
            let mut items = reply.into_inner();
            let mut ticks = Vec::new();
            let code = loop {
                match support::within("the next item", items.next()).await {
                    Some(Ok(tick)) => ticks.push(tick.n),
                    Some(Err(status)) => break status.code(),
                    None => break Code::Ok,
                }
            };
            (ticks, code)
        }
    };
    let ended = async {
        let first = reported.recv().await;
        if first.is_some() {
            assert_eq!(reported.recv().await, None, "{method}: the execution reported its stream's end twice");
        }
        first
    };
    let outcome = tokio::time::timeout(PATIENCE, ended).await.unwrap_or_else(|_| panic!("{method}: the execution did not end within {PATIENCE:?}"));
    app.stop().await;
    (delivered, outcome)
}

#[tokio::test]
async fn a_stream_an_interceptor_discards_reports_nothing() {
    let (delivered, outcome) = call(DISCARDED, None, |mut client, request| async move { client.discarded(request).await }).await;
    assert_eq!(delivered, (vec![0], Code::Ok), "the interceptor's answer was not the reply");
    assert_eq!(outcome, None, "the stream the interceptor discarded reported its end");
}

#[tokio::test]
async fn the_stream_replacing_a_discarded_one_reports_its_own_end() {
    let (delivered, outcome) = call(REPLACED, None, |mut client, request| async move { client.replaced(request).await }).await;
    assert_eq!(delivered, (vec![7, 8], Code::Ok), "the interceptor's stream was not the reply");
    assert_eq!(outcome, Some(StreamOutcome::Completed), "the reply's own end was not what the execution reported");
}

#[tokio::test]
async fn a_written_stream_reports_completed_at_its_trailers() {
    let (delivered, outcome) = call(WRITTEN, None, |mut client, request| async move { client.written(request).await }).await;
    assert_eq!(delivered, (vec![1, 2], Code::Ok));
    assert_eq!(outcome, Some(StreamOutcome::Completed));
}

#[tokio::test]
async fn a_stream_ending_in_an_error_reports_cut_off() {
    let (delivered, outcome) = call(FAILED, None, |mut client, request| async move { client.failed(request).await }).await;
    assert_eq!(delivered, (vec![1], Code::Unavailable), "the item's error did not end the reply in its trailers");
    assert_eq!(outcome, Some(StreamOutcome::CutOff(None)), "a reply ended in non-zero trailers did not report `CutOff`");
}

#[tokio::test]
async fn a_stream_the_error_handlers_answer_after_the_deadline_reports_cut_off() {
    let (delivered, outcome) = call(EXPIRED, Some(DEADLINE), |mut client, request| async move { client.expired(request).await }).await;
    assert_eq!(delivered, (Vec::new(), Code::DeadlineExceeded), "the timed-out call was not answered DEADLINE_EXCEEDED");
    assert_eq!(outcome, Some(StreamOutcome::CutOff(Some(CancelReason::Deadline))), "the stream dropped at the deadline did not report it");
}

#[tokio::test]
async fn a_stream_the_caller_abandons_before_its_trailers_reports_cut_off() {
    let (sender, mut reported) = mpsc::unbounded_channel();
    outcomes().insert(ABANDONED, sender);
    let app = Running::start(Root, ulo_grpc::Server::new("127.0.0.1:0")).await;
    let (http2, origin) = app.plain_http2();
    let mut client = EndsClient::with_origin(http2, origin);
    let reply = support::within("the call", client.abandoned(Ticks::default())).await.expect("the call is answered");
    let mut items = reply.into_inner();
    let first = support::within("the first item", items.next()).await;
    assert_eq!(first.map(|item| item.map(|tick| tick.n).map_err(|status| status.code())), Some(Ok(1)));
    drop((items, client));
    let outcome = tokio::time::timeout(PATIENCE, reported.recv()).await.expect("the execution ended once the caller left");
    assert_eq!(outcome, Some(StreamOutcome::CutOff(Some(CancelReason::ClientCancelled))), "a reply dropped before its trailers did not report it");
    app.stop().await;
}
