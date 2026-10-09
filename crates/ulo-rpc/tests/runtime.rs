//! The runtime the RPC transport is given, never looked for: the server refuses an app with none,
//! and a client built outside an app closes its link from a task on the runtime it holds when it
//! is dropped, or, when that runtime drops the task unrun, releases its connection and warns.
//!
//! The [`Capture`] installed as the global subscriber tells one test's events from another's by
//! the thread, for the reason `drain_window.rs` gives: tracing caches a callsite's interest from
//! the first thread to reach it.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, StartupError};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, DeliveryMode, Link, Outbound, Pattern, RpcClient};
use ulo_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(5);

const UNFINISHED: &str = "the RPC client was dropped and its runtime dropped the link's close before it finished; \
                          its connection was released link=\"observed\"";

/// What the test sees of its link.
#[derive(Default)]
struct Seen {
    /// The connection's `send` was dropped, which is the client releasing it.
    released: AtomicBool,
    closed: AtomicBool,
    closing: Notify,
}

/// A link whose client side connects at once and never replies, recording its release and close.
struct Observed(Arc<Seen>);

impl Link for Observed {
    const NAME: &'static str = "observed";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        Ok(Box::pin(futures_util::stream::pending()))
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        let held = Released(Arc::clone(&self.0));
        Ok(Outbound {
            send: Box::new(move |_pattern, _frame, _reply_to| {
                let _ = &held;
                Box::pin(async { Ok(()) })
            }),
            replies: Box::pin(futures_util::stream::pending()),
        })
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        self.0.closed.store(true, Ordering::SeqCst);
        self.0.closing.notify_one();
        Ok(())
    }
}

/// Held by the connection's `send`, so it drops when the client releases the connection.
struct Released(Arc<Seen>);

impl Drop for Released {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn the_server_refuses_an_app_with_no_runtime() {
    let bound = App::builder(Root)
        .timer(ulo_tokio::Timer)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Observed(Arc::default())))
        .listen()
        .await;
    let Err(StartupError::Configure(errors)) = bound else {
        panic!("an RPC server on an app with a timer and no runtime was not refused in `prepare`");
    };
    let text = errors.to_string();
    assert!(
        text.contains("the RPC server is bound on an app with no runtime") && text.contains("set one with `.runtime(..)`"),
        "the refusal does not name the missing `.runtime(..)`: {text}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_dropped_client_closes_its_link_on_its_runtime() {
    capture();
    let seen = Arc::new(Seen::default());
    let client = RpcClient::new(Observed(Arc::clone(&seen)), Arc::new(Tokio::current()));
    client.emit("runtime.connect", &1u32).await.expect("the event is sent, which connects");

    let closing = seen.closing.notified();
    drop(client);
    assert!(seen.released.load(Ordering::SeqCst), "the dropped client did not release its connection");
    tokio::time::timeout(PATIENCE, closing).await.expect("the dropped client did not close its link on its runtime");
    assert!(seen.closed.load(Ordering::SeqCst), "the link's close did not run");
    let warnings = capture().warnings(thread::current().id());
    assert!(warnings.is_empty(), "a close that finished warned: {warnings:?}");
}

#[test]
fn a_dropped_client_whose_runtime_has_shut_down_releases_its_connection_and_warns() {
    capture();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime for the test");
    let seen = Arc::new(Seen::default());
    let client = RpcClient::new(Observed(Arc::clone(&seen)), Arc::new(Tokio::from_handle(runtime.handle().clone())));
    runtime.block_on(async { client.emit("runtime.connect", &1u32).await }).expect("the event is sent, which connects");
    drop(runtime);

    drop(client);
    assert!(seen.released.load(Ordering::SeqCst), "the dropped client did not release its connection");
    assert!(!seen.closed.load(Ordering::SeqCst), "the link's close ran on a runtime that had shut down");
    let warnings = capture().warnings(thread::current().id());
    assert!(warnings.iter().any(|line| line == UNFINISHED), "no `warn` that the link's close did not run: {warnings:?}");
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
