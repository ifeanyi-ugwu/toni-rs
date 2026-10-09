//! What a caller receives when an error handler answers a call whose `deadline-ms` passed: a
//! single reply as the error handler wrote it, and a stream ended at once with `err` of kind
//! `timeout`, its items never written, with a `warn` line telling the author why.
//!
//! The tests run on a current-thread runtime, so the server's tasks emit their events on the test's
//! thread, where [`Capture`] is the default subscriber.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt, stream};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::app::Connected;
use ulo::{App, BoxError, Bound, ErrorHandler, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc::{Reply, Rpc, RpcClient, RpcClientModule, RpcCx};
use ulo_rpc_tcp::Tcp;
use ulo_transport::{CallError, ErrorKind};

const DEADLINE: Duration = Duration::from_millis(200);
const GRACE: Duration = Duration::from_millis(500);

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(5);

const WARNING: &str = "an error handler answered a timed-out call with a stream; the stream was ended at the deadline";

/// Answers a `Timeout` with a stream that never ends.
struct EndlessOnTimeout;

impl ErrorHandler<Rpc> for EndlessOnTimeout {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::repeat_with(|| Ok::<_, CallError>(1u64))))
    }
}

/// Answers a `Timeout` with one reply, the string `"claimed"`.
struct OneOnTimeout;

impl ErrorHandler<Rpc> for OneOnTimeout {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply("claimed")?)
    }
}

fn timed_out(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().is_some_and(|call| call.kind() == ErrorKind::Timeout)
}

#[injectable]
struct Clock;

#[routes]
impl Clock {
    /// Answers once its execution is cancelled, which the deadline does first.
    #[ulo_rpc::message("clock.stall_streamed")]
    #[error_handlers(value = EndlessOnTimeout)]
    async fn stall_streamed(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, CallError>> {
        exec.cancelled().await;
        stream::empty()
    }

    #[ulo_rpc::message("clock.stall_answered")]
    #[error_handlers(value = OneOnTimeout)]
    async fn stall_answered(&self, exec: ExecutionRef) -> Result<String, CallError> {
        exec.cancelled().await;
        Ok(String::new())
    }
}

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Clock>();
    }
}

struct ClientRoot {
    link: Mutex<Option<Tcp>>,
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.link.lock().unwrap_or_else(PoisonError::into_inner).take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(PATIENCE)));
        }
    }
}

/// A server on a loopback port the OS chose and a client app reaching it.
struct Running {
    server: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: App<Connected>,
    rpc: RpcClient,
}

impl Running {
    async fn start() -> Running {
        let server = App::builder(ServerRoot)
            .runtime(ulo_tokio::Tokio::current())
            .wire()
            .expect("the server app wires")
            .connect()
            .await
            .expect("the server app connects")
            .bind(ulo_rpc::Server::new(Tcp::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))).timeout_grace(Bound::After(GRACE)))
            .listen()
            .await
            .expect("the server listens");
        let addr = match server.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected the server to report one bound address, got {other:?}"),
        };
        let handle = server.handle();
        let serving = tokio::spawn(async move {
            let _ = server.serve(std::future::pending::<Signal>()).await;
        });
        let client = App::builder(ClientRoot { link: Mutex::new(Some(Tcp::new(addr))) })
            .runtime(ulo_tokio::Tokio::current())
            .wire()
            .expect("the client app wires")
            .connect()
            .await
            .expect("the client app connects");
        let rpc = (*client.get::<RpcClient>().await.expect("the client app holds an `RpcClient`")).clone();
        Running { server: handle, serving, client, rpc }
    }

    async fn stop(self) {
        let _ = self.client.close(Signal::new("test")).await;
        let _ = self.server.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

#[tokio::test]
async fn a_stream_answered_after_the_deadline_ends_at_once_with_timeout() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let began = Instant::now();
    let opened = running.rpc.stream::<_, u64>("clock.stall_streamed", &()).header("deadline-ms", DEADLINE.as_millis().to_string());
    let mut replies = within("the call's opening", opened).await.unwrap_or_else(|error| panic!("the call did not open: {error:?}"));
    let first = within("the call's first frame", replies.next()).await;
    let took = began.elapsed();
    match first {
        Some(Err(error)) => {
            assert_eq!((error.kind(), error.message()), (ErrorKind::Timeout, "the call's deadline passed"), "{error:?}");
        }
        other => panic!("expected the call to end with the server's `timeout`, got {other:?}"),
    }
    assert!(took < DEADLINE + GRACE, "the call ended after {took:?}, not at once after its {DEADLINE:?} deadline");
    assert!(within("the call's end", replies.next()).await.is_none(), "a frame followed the `err` that ended the call");
    let warned = capture.warnings();
    assert!(
        warned.iter().any(|line| line.contains(WARNING) && line.contains("pattern=clock.stall_streamed")),
        "the server logged no warning naming the pattern: {warned:?}"
    );
    running.stop().await;
}

#[tokio::test]
async fn a_single_reply_answered_after_the_deadline_is_the_reply() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let call = running.rpc.request::<_, String>("clock.stall_answered", &()).header("deadline-ms", DEADLINE.as_millis().to_string());
    let reply = within("the call", call).await;
    assert_eq!(reply.map_err(|error| (error.kind(), error.message().to_owned())), Ok("claimed".to_owned()));
    let warned = capture.warnings();
    assert!(!warned.iter().any(|line| line.contains(WARNING)), "a single reply was logged as a stream: {warned:?}");
    running.stop().await;
}

async fn within<F: IntoFuture>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(PATIENCE, fut).await {
        Ok(output) => output,
        Err(_) => panic!("{what} did not happen within {PATIENCE:?}"),
    }
}

/// Every `warn` and `error` event on the thread it is the default subscriber of, as its message
/// followed by `name=value` for each other field.
#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<String>>>,
    next_span: Arc<AtomicU64>,
}

impl Capture {
    fn warnings(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_span() || *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push(format!("{}{}", line.message, line.fields));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields.push_str(&format!(" {}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}
