//! `on_stream_end` reports the reply the dispatcher writes. A stream an interceptor discards was
//! never the reply and reports nothing, so it cannot hide the outcome of the reply that replaced
//! it; a stream the error handlers answer after the deadline was the reply, dropped unwritten, and
//! reports `CutOff(Deadline)`.
//!
//! Each handler registers a callback that sends the outcome on a channel the test holds the other
//! end of. The callback owns the channel's only sender, so the channel closes when the execution
//! ends: a test reads `None` for a stream that reported nothing, with no wait for a report that
//! might still come.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::{Stream, stream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use ulo::{
    App, AppHandle, BoxError, CancelReason, ErrorHandler, ExecutionRef, Interceptor, Module, ModuleDef, ModuleIdentity, Next, Signal,
    StreamOutcome, injectable, routes,
};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, Frame, Link, Outbound, Pattern, Reply, ReplyPath, Rpc, RpcCx};
use ulo_transport::{Classify, ErrorKind};

const DISCARDED: &str = "stream_end.discarded";
const REPLACED: &str = "stream_end.replaced";
const WRITTEN: &str = "stream_end.written";
const EXPIRED: &str = "stream_end.expired";

/// How long a test waits for the execution to end; it ends as soon as the reply is written.
const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Late;

impl fmt::Display for Late {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("late")
    }
}

impl Error for Late {}

impl Classify for Late {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// The sender for each pattern's call, taken by the handler that reports for it.
fn outcomes() -> MutexGuard<'static, HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>> {
    static OUTCOMES: OnceLock<Mutex<HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>>> = OnceLock::new();
    OUTCOMES.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers the callback that reports `pattern`'s outcome, owning the channel's one sender.
fn report_for(pattern: &'static str, exec: &ExecutionRef) {
    let sender = outcomes().remove(pattern).expect("each pattern is called once");
    exec.on_stream_end(move |outcome| {
        let _ = sender.send(outcome);
    });
}

fn ticks() -> impl Stream<Item = Result<u32, Late>> {
    stream::iter([Ok(1), Ok(2)])
}

/// Runs the call, drops the stream it answers and answers one payload instead.
struct AnswerOne;

impl Interceptor<Rpc> for AnswerOne {
    async fn intercept(&self, cx: &RpcCx, next: Next<'_, Rpc>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(cx.reply(&0u32)?)
    }
}

/// Runs the call, drops the stream it answers and answers a stream of its own.
struct AnswerStream;

impl Interceptor<Rpc> for AnswerStream {
    async fn intercept(&self, cx: &RpcCx, next: Next<'_, Rpc>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(cx.reply_stream(ticks()))
    }
}

/// Answers the `Timeout` a passed deadline offers with a stream.
struct StreamOnTimeout;

impl ErrorHandler<Rpc> for StreamOnTimeout {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        let _ = err;
        Ok(cx.reply_stream(ticks()))
    }
}

#[injectable]
struct Streams;

#[routes]
impl Streams {
    #[ulo_rpc::message("stream_end.discarded")]
    #[interceptors(value = AnswerOne)]
    async fn discarded(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(DISCARDED, &exec);
        ticks()
    }

    #[ulo_rpc::message("stream_end.replaced")]
    #[interceptors(value = AnswerStream)]
    async fn replaced(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(REPLACED, &exec);
        ticks()
    }

    #[ulo_rpc::message("stream_end.written")]
    async fn written(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(WRITTEN, &exec);
        ticks()
    }

    #[ulo_rpc::message("stream_end.expired")]
    #[error_handlers(value = StreamOnTimeout)]
    async fn expired(&self, exec: ExecutionRef) -> Result<u32, Late> {
        report_for(EXPIRED, &exec);
        std::future::pending::<()>().await;
        Ok(0)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Streams>();
    }
}

/// A link whose inbound stream the test feeds.
struct Scripted(Mutex<Option<Inbound>>);

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).miss_signal(true)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

struct Running {
    handle: AppHandle,
    serving: JoinHandle<()>,
    deliveries: mpsc::UnboundedSender<Delivery>,
}

impl Running {
    async fn start() -> Running {
        let (deliveries, mut receiver) = mpsc::unbounded_channel::<Delivery>();
        let inbound: Inbound = Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx)));
        let app = App::builder(Root)
            .timer(ulo_tokio::Timer)
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_rpc::Server::new(Scripted(Mutex::new(Some(inbound)))))
            .listen()
            .await
            .expect("the server binds");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { handle, serving, deliveries }
    }

    /// Calls `pattern` with `headers` and returns every frame of its reply, and what its
    /// execution reported once it ended: `None` for nothing.
    async fn call(&self, pattern: &'static str, headers: CallHeaders) -> (Vec<Frame>, Option<StreamOutcome>) {
        let (sender, mut reported) = mpsc::unbounded_channel();
        outcomes().insert(pattern, sender);
        let (frames, mut replies) = mpsc::unbounded_channel();
        let reply = ReplyPath::new(move |frame: Frame| {
            let sent = frames.send(frame).map_err(BoxError::from);
            Box::pin(async move { sent })
        });
        let frame = Frame::Req { id: 1, pattern: pattern.to_owned(), headers, data: Data::new(b"null".as_slice()) };
        self.deliveries.send(Delivery { frame, reply: Some(reply), ack: Ack::none() }).expect("the server reads its link");
        let ended = async {
            let first = reported.recv().await;
            if first.is_some() {
                assert_eq!(reported.recv().await, None, "{pattern}: the execution reported its stream's end twice");
            }
            first
        };
        let outcome = tokio::time::timeout(PATIENCE, ended)
            .await
            .unwrap_or_else(|_| panic!("{pattern}: the execution did not end within {PATIENCE:?}"));
        let mut written = Vec::new();
        while let Ok(frame) = replies.try_recv() {
            written.push(frame);
        }
        (written, outcome)
    }

    /// Ends the inbound stream, as a link does once nothing more will arrive, so the drain has
    /// nothing to wait for, and closes the app.
    async fn stop(self) {
        let Running { handle, serving, deliveries } = self;
        drop(deliveries);
        let _ = handle.close(Signal::new("stream end test")).await;
        let _ = serving.await;
    }
}

fn data(n: u32) -> Data {
    Data::new(n.to_string().into_bytes())
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_an_interceptor_discards_reports_nothing() {
    let running = Running::start().await;
    let (frames, outcome) = running.call(DISCARDED, CallHeaders::new()).await;
    assert_eq!(frames, vec![Frame::Res { id: 1, data: data(0) }], "the interceptor's answer was not the reply");
    assert_eq!(outcome, None, "the stream the interceptor discarded reported its end");
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_stream_replacing_a_discarded_one_reports_its_own_end() {
    let running = Running::start().await;
    let (frames, outcome) = running.call(REPLACED, CallHeaders::new()).await;
    let expected = vec![Frame::Item { id: 1, data: data(1) }, Frame::Item { id: 1, data: data(2) }, Frame::End { id: 1 }];
    assert_eq!(frames, expected, "the interceptor's stream was not the reply");
    assert_eq!(outcome, Some(StreamOutcome::Completed), "the reply's own end was not what the execution reported");
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_written_stream_reports_completed() {
    let running = Running::start().await;
    let (frames, outcome) = running.call(WRITTEN, CallHeaders::new()).await;
    assert_eq!(frames.last(), Some(&Frame::End { id: 1 }), "the stream was not written to its end: {frames:?}");
    assert_eq!(outcome, Some(StreamOutcome::Completed));
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_the_error_handlers_answer_after_the_deadline_reports_cut_off() {
    let running = Running::start().await;
    let mut headers = CallHeaders::new();
    headers.insert("deadline-ms", "20");
    let (frames, outcome) = running.call(EXPIRED, headers).await;
    assert!(
        matches!(frames.as_slice(), [Frame::Err { id: 1, error }] if error.kind == ErrorKind::Timeout),
        "the timed-out call was not answered `timeout`: {frames:?}",
    );
    assert_eq!(outcome, Some(StreamOutcome::CutOff(Some(CancelReason::Deadline))), "the stream dropped at the deadline did not report it");
    running.stop().await;
}

