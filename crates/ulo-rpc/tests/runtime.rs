//! The runtime the RPC transport is given, never looked for: `listen()` refuses an RPC server on an
//! app with none, `RpcClientModule` fails `wire()` on one naming `.runtime(..)`, and a client
//! built outside an app closes its link from a task on the runtime it holds when it is dropped,
//! or, when that runtime drops the task unrun, releases its connection and warns. A client built
//! outside an app takes its default timeout from `RpcClient::timeout`, as a module's takes it from
//! `RpcClientModule::timeout`, and both refuse a zero as `ZeroTimeout`. A link that can never
//! connect is refused as `UnusableLink`, by `RpcClient::new` where the client is built and by
//! `RpcClientModule` when the app connects.
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
use ulo::{App, Bound, BoxError, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, RuntimeMissing, StartupError, WiringError};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, DeliveryMode, Link, Outbound, Pattern, RpcClient, RpcClientModule, UnusableLink, ZeroTimeout};
use ulo_transport::ErrorKind;
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
async fn listen_refuses_an_rpc_server_on_an_app_with_no_runtime() {
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
    let Err(StartupError::Bind { source, .. }) = bound else {
        panic!("an RPC server on an app with a timer and no runtime was not refused at `listen()`");
    };
    assert!(source.downcast_ref::<RuntimeMissing>().is_some(), "the refusal is not the core's `RuntimeMissing`: {source}");
    let text = source.to_string();
    assert!(text.contains("set one with `.runtime(..)`"), "the refusal does not name the missing `.runtime(..)`: {text}");
}

/// Imports one `RpcClientModule` over a link that never replies.
struct WithClient;

impl Module for WithClient {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Observed(Arc::default())));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_client_module_on_an_app_with_a_timer_alone_fails_wiring_naming_runtime() {
    let wired = App::builder(WithClient).timer(ulo_tokio::Timer).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an `RpcClientModule` on an app with a timer alone wired");
    };
    let text = errors.to_string();
    assert!(text.contains("missing dependency `dyn Runtime`"), "the wiring error does not name `dyn Runtime`: {text}");
    assert!(
        text.contains("help: set one on the app with `.runtime(..)`"),
        "the wiring error does not name `.runtime(..)`: {text}"
    );
}

/// How long a request on `client`, over a link that never replies, waited on the paused clock
/// before it failed `Timeout`; `None` if it had not answered after a minute.
async fn timed_out_after(client: RpcClient) -> Option<Duration> {
    let started = tokio::time::Instant::now();
    let call = client.request::<_, u32>("runtime.request", &1u32);
    let answered = tokio::time::timeout(Duration::from_secs(60), call).await.ok()?;
    let error = answered.expect_err("a link that never replies answered a request");
    assert_eq!(error.kind(), ErrorKind::Timeout, "the request failed other than by its timeout: {error}");
    Some(started.elapsed())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_client_built_outside_an_app_times_out_at_its_own_timeout() {
    let client = RpcClient::new(Observed(Arc::default()), Arc::new(Tokio::current())).expect("the link is usable");
    let default = timed_out_after(client.clone()).await;
    assert_eq!(default, Some(Duration::from_secs(5)), "a client from `RpcClient::new` with no `timeout`");
    let shorter = client.clone().timeout(Bound::After(Duration::from_millis(250))).expect("a nonzero timeout is taken");
    let shorter = timed_out_after(shorter).await;
    assert_eq!(shorter, Some(Duration::from_millis(250)), "`RpcClient::timeout(Bound::After(250 ms))`");
    let unbounded = client.clone().timeout(Bound::Unbounded).expect("no timeout is taken");
    let unbounded = timed_out_after(unbounded).await;
    assert_eq!(unbounded, None, "`RpcClient::timeout(Bound::Unbounded)` timed a request out");
    let again = timed_out_after(client).await;
    assert_eq!(again, Some(Duration::from_secs(5)), "the client whose clones were given timeouts kept its own");
}

#[tokio::test(flavor = "current_thread")]
async fn a_zero_client_timeout_is_refused_where_it_is_written() {
    let client = RpcClient::new(Observed(Arc::default()), Arc::new(Tokio::current())).expect("the link is usable");
    let Err(refused) = client.timeout(Bound::After(Duration::ZERO)) else {
        panic!("`RpcClient::timeout(Bound::After(Duration::ZERO))` was taken");
    };
    assert_eq!(refused.link(), "observed");
    assert_eq!(
        refused.to_string(),
        "`RpcClient::timeout(Bound::After(Duration::ZERO))` on the observed link would time out every call; write \
         `Bound::Unbounded` to turn the timeout off"
    );
}

/// Imports one `RpcClientModule` whose timeout is zero.
struct WithZeroTimeout;

impl Module for WithZeroTimeout {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Observed(Arc::default())).timeout(Bound::After(Duration::ZERO)));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_zero_module_timeout_fails_wiring_as_the_same_refusal() {
    let wired = App::builder(WithZeroTimeout).runtime(Tokio::current()).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an `RpcClientModule` with a zero timeout wired");
    };
    let refused = errors
        .iter()
        .find_map(|error| match error {
            WiringError::ValueFailed { error, .. } => error.downcast_ref::<ZeroTimeout>(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no wiring error carries a `ZeroTimeout`: {errors}"));
    assert_eq!(refused.link(), "observed");
    assert_eq!(
        refused.to_string(),
        "`RpcClientModule::timeout(Bound::After(Duration::ZERO))` on the observed link would time out every call; write \
         `Bound::Unbounded` to turn the timeout off"
    );
}

/// What `Link::usable` refuses with on [`Unusable`].
const UNUSABLE: &str = "the unusable link has no tokio runtime: build it inside one, or give it one with `.with_handle(..)`";

/// A link whose `usable` refuses, as a tokio-based link built outside a runtime does; it counts
/// its connects.
#[derive(Default)]
struct Unusable(Arc<AtomicU64>);

impl Link for Unusable {
    const NAME: &'static str = "unusable";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
    }

    fn usable(&self) -> Result<(), BoxError> {
        Err(UNUSABLE.into())
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        Err("this link has no server side".into())
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(UNUSABLE.into())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

#[test]
fn a_client_on_a_link_it_cannot_use_is_refused_where_it_is_built() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a tokio runtime for the test");
    let connects = Arc::new(AtomicU64::new(0));
    let built = RpcClient::new(Unusable(Arc::clone(&connects)), Arc::new(Tokio::from_handle(runtime.handle().clone())));
    let Err(refused) = built else {
        panic!("`RpcClient::new` took a link whose `usable` refuses");
    };
    assert_eq!(refused.link(), "unusable");
    assert_eq!(refused.to_string(), format!("`RpcClient::new` was given a unusable link it cannot use: {UNUSABLE}"));
    assert_eq!(connects.load(Ordering::SeqCst), 0, "the refused client connected");
}

/// Imports one `RpcClientModule` over a link whose `usable` refuses.
struct WithUnusable(Arc<AtomicU64>);

impl Module for WithUnusable {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Unusable(Arc::clone(&self.0))));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_client_module_on_a_link_it_cannot_use_fails_connect() {
    let connects = Arc::new(AtomicU64::new(0));
    let wired = App::builder(WithUnusable(Arc::clone(&connects))).runtime(Tokio::current()).wire().expect("the app wires");
    let Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(error), .. })) = wired.connect().await else {
        panic!("an `RpcClientModule` over a link whose `usable` refuses connected, or failed other than in a hook");
    };
    let refused = error.downcast_ref::<UnusableLink>().unwrap_or_else(|| panic!("the hook's error is not an `UnusableLink`: {error}"));
    assert_eq!(refused.link(), "unusable");
    assert_eq!(refused.to_string(), format!("`RpcClientModule::for_root` was given a unusable link it cannot use: {UNUSABLE}"));
    assert_eq!(connects.load(Ordering::SeqCst), 0, "the refused client connected");
}

#[tokio::test(flavor = "current_thread")]
async fn a_dropped_client_closes_its_link_on_its_runtime() {
    capture();
    let seen = Arc::new(Seen::default());
    let client = RpcClient::new(Observed(Arc::clone(&seen)), Arc::new(Tokio::current())).expect("the link is usable");
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
    let client = RpcClient::new(Observed(Arc::clone(&seen)), Arc::new(Tokio::from_handle(runtime.handle().clone()))).expect("the link is usable");
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
