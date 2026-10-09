//! The drain window over a link the test drives: a call the link hands over during the drain is
//! answered before the server closes, though it opens no execution for the core to wait for, and
//! what the drain's deadline leaves unanswered is logged at `warn` with how much there was.
//!
//! The tests run on current-thread runtimes, so a server's tasks run on its test's thread, and
//! the [`Capture`] installed as the global subscriber tells one test's events from another's by
//! the thread. A thread-local subscriber would not do: tracing caches whether a callsite is
//! enabled from the first thread to reach it, and with one such subscriber in the process a test
//! without it reaching the `warn` first would disable it for the test that has one.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use futures_util::stream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::{App, AppHandle, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, Frame, Link, Outbound, Pattern, Payload, ReplyPath};
use ulo_transport::{Classify, ErrorKind};

const ADD: &str = "drain.add";

/// How long a test lets a server that does not wait for the call run on before handing the call
/// over: long enough for a close that waits for nothing to finish first.
const WINDOW: Duration = Duration::from_millis(50);

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[injectable]
struct Adder;

#[routes]
impl Adder {
    #[ulo_rpc::message("drain.add")]
    async fn add(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0 + 1)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Adder>();
    }
}

/// What the test sees of its link and does through it.
#[derive(Default)]
struct Script {
    inbound: Mutex<Option<Inbound>>,
    /// The inbound stream's sender; `None` once the test ends the stream.
    sender: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
    drained: watch::Sender<bool>,
    /// Deliveries the link's `close` hands over before it returns, after the server raised its
    /// close signal and before `serve` has run again.
    at_close: Mutex<Vec<Delivery>>,
}

/// A link whose inbound stream the test feeds and ends, and whose `drain` returns at once.
struct Scripted(Arc<Script>);

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).miss_signal(true)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        lock(&self.0.inbound).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {
        self.0.drained.send_replace(true);
    }

    async fn close(&self) -> Result<(), BoxError> {
        let pending = std::mem::take(&mut *lock(&self.0.at_close));
        for delivery in pending {
            self.0.deliver(delivery);
        }
        Ok(())
    }
}

impl Script {
    fn deliver(&self, delivery: Delivery) {
        if let Some(sender) = lock(&self.sender).as_ref() {
            let _ = sender.send(delivery);
        }
    }
}

/// One server over the scripted link, serving until closed.
struct Running {
    handle: AppHandle,
    serving: JoinHandle<()>,
    script: Arc<Script>,
}

impl Running {
    async fn start(drain: Duration) -> Running {
        capture();
        let (sender, mut receiver) = mpsc::unbounded_channel::<Delivery>();
        let script = Arc::new(Script::default());
        *lock(&script.inbound) = Some(Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx))));
        *lock(&script.sender) = Some(sender);
        let app = App::builder(Root)
            .runtime(ulo_tokio::Tokio::current())
            .drain_timeout(drain)
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_rpc::Server::new(Scripted(Arc::clone(&script))))
            .listen()
            .await
            .expect("the server binds");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { handle, serving, script }
    }

    /// Starts the app's close and waits until the server has called the link's `drain`.
    async fn drain(&self) -> JoinHandle<()> {
        let handle = self.handle.clone();
        let closing = tokio::spawn(async move {
            let _ = handle.close(Signal::new("drain window test")).await;
        });
        let _ = self.script.drained.subscribe().wait_for(|drained| *drained).await;
        closing
    }

    /// Waits until the app stops serving or [`WINDOW`] passes, whichever is first: a server
    /// whose drain does not wait for the call has closed by then.
    async fn window(&mut self) {
        tokio::select! {
            _ = &mut self.serving => {}
            () = tokio::time::sleep(WINDOW) => {}
        }
    }

    fn deliver(&self, delivery: Delivery) {
        self.script.deliver(delivery);
    }

    /// Ends the link's inbound stream, as a link does once nothing more will arrive.
    fn end_inbound(&self) {
        lock(&self.script.sender).take();
    }

    async fn stopped(self, closing: JoinHandle<()>) {
        closing.await.expect("the close task completes");
        if !self.serving.is_finished() {
            self.serving.await.expect("the serving task completes");
        }
    }
}

/// A request for [`ADD`] whose replies `frames` receives, each once `gate` holds `true`.
fn request(id: u64, frames: mpsc::UnboundedSender<Frame>, gate: watch::Receiver<bool>) -> Delivery {
    let reply = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let frames = frames.clone();
        let mut gate = gate.clone();
        Box::pin(async move {
            gate.wait_for(|open| *open).await?;
            frames.send(frame)?;
            Ok(())
        })
    });
    let frame = Frame::Req { id, pattern: ADD.to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
    Delivery { frame, reply: Some(reply), ack: Ack::none() }
}

fn open_gate() -> watch::Receiver<bool> {
    watch::channel(true).1
}

fn refused_unavailable(frame: &Result<Frame, mpsc::error::TryRecvError>, id: u64) -> bool {
    matches!(frame, Ok(Frame::Err { id: answered, error }) if *answered == id && error.kind == ErrorKind::Unavailable)
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_the_link_hands_over_after_its_drain_returned_is_refused_before_close() {
    let mut running = Running::start(Duration::from_secs(5)).await;
    let (frames, mut replies) = mpsc::unbounded_channel();

    let closing = running.drain().await;
    running.window().await;
    running.deliver(request(1, frames, open_gate()));
    running.end_inbound();
    running.stopped(closing).await;

    let frame = replies.try_recv();
    assert!(refused_unavailable(&frame, 1), "the call handed over during the drain was not refused `unavailable`: {frame:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_refusal_spawned_before_the_inbound_stream_ended_is_sent_before_close() {
    let mut running = Running::start(Duration::from_secs(5)).await;
    let (frames, mut replies) = mpsc::unbounded_channel();
    let (gate, opened) = watch::channel(false);

    let closing = running.drain().await;
    running.deliver(request(1, frames, opened));
    running.end_inbound();
    running.window().await;
    gate.send_replace(true);
    running.stopped(closing).await;

    let frame = replies.try_recv();
    assert!(refused_unavailable(&frame, 1), "the refusal spawned during the drain was not sent: {frame:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn the_drains_deadline_logs_what_close_leaves_unanswered() {
    let running = Running::start(Duration::from_millis(200)).await;
    let (frames, mut replies) = mpsc::unbounded_channel();
    let (_gate, shut) = watch::channel(false);
    {
        let mut at_close = lock(&running.script.at_close);
        at_close.push(request(2, frames.clone(), open_gate()));
        at_close.push(request(3, frames.clone(), open_gate()));
    }

    // The refusal waits on a gate never opened and the inbound stream never ends, so the drain
    // runs to its deadline.
    let closing = running.drain().await;
    running.deliver(request(1, frames, shut));
    running.stopped(closing).await;

    assert!(replies.try_recv().is_err(), "a call the drain's deadline cut off was answered");
    let warnings = capture().warnings(thread::current().id());
    let expected = "the RPC server closed before answering every call that reached it; the drain's deadline passed first \
                    link=\"scripted\" refusals_unsent=1 deliveries_unread=2";
    assert!(warnings.iter().any(|line| line == expected), "no `warn` counting what close left unanswered: {warnings:?}");
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The process's subscriber, installed by the first test to start.
fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone()).expect("no other subscriber is installed");
        capture
    })
}

/// Every `warn` and `error` event, as its message followed by `name=value` for each other field,
/// with the thread it was emitted on.
#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<(ThreadId, String)>>>,
    next_span: Arc<AtomicU64>,
}

impl Capture {
    fn warnings(&self, on: ThreadId) -> Vec<String> {
        lock(&self.lines).iter().filter(|(thread, _)| *thread == on).map(|(_, line)| line.clone()).collect()
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

    fn event(&self, event: &TraceEvent<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        lock(&self.lines).push((thread::current().id(), format!("{}{}", line.message, line.fields)));
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
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}
