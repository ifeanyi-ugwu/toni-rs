//! The application every scenario runs, the client beside it, and the waits the scenarios share.
//!
//! The handlers are split across three controllers so a link refusing a shape or a payload kind
//! in `prepare` still runs every scenario it carries: the streaming controller is mounted only
//! where the link's `shapes` carry every streamed shape, the binary one only where it declares
//! `binary`.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use ulo::app::{Bound as Serving, Connected};
use ulo::{
    App, AppHandle, BoxError, Bound, CancelReason, Dep, ErrorHandler, ExecutionRef, Guard, Module, ModuleDef,
    ModuleIdentity, Shape, Shutdown, ShutdownError, Signal, StartupError, injectable, routes,
};
use ulo_rpc::{
    CallHeaders, Capabilities, Data, Inbound, Link, Payload, Reply, Rpc, RpcClient, RpcClientModule, RpcCx, RpcError,
};
use ulo_transport::{CallError, Classify, ErrorKind, Tracked};

use crate::{Broker, report};

pub(crate) const ADD: &str = "conformance.add";
pub(crate) const ECHO: &str = "conformance.echo";
pub(crate) const CONFLICT: &str = "conformance.conflict";
pub(crate) const GUARDED: &str = "conformance.guarded";
pub(crate) const BOOM: &str = "conformance.boom";
pub(crate) const HEADERS: &str = "conformance.headers";
pub(crate) const EVENT: &str = "conformance.event";
pub(crate) const TALLY: &str = "conformance.tally";
pub(crate) const STALL: &str = "conformance.stall";
pub(crate) const HOLD: &str = "conformance.hold";
pub(crate) const NEVER: &str = "conformance.never";
pub(crate) const COUNT: &str = "conformance.count";
pub(crate) const SUM: &str = "conformance.sum";
pub(crate) const DOUBLE: &str = "conformance.double";
pub(crate) const TICKS: &str = "conformance.ticks";
pub(crate) const UNTIL_DRAIN: &str = "conformance.until_drain";
pub(crate) const BYTES: &str = "conformance.bytes";
pub(crate) const NOBODY: &str = "conformance.nobody";
pub(crate) const NOBODY_EVENT: &str = "conformance.nobody.event";
pub(crate) const SUBSTITUTED: &str = "conformance.substituted";
pub(crate) const SUBSTITUTED_STREAM: &str = "conformance.substituted_stream";
pub(crate) const UNENCODABLE: &str = "conformance.unencodable";
pub(crate) const UNENCODABLE_STREAM: &str = "conformance.unencodable_stream";
pub(crate) const CONTEXT: &str = "conformance.context";

/// A payload no codec reads: not UTF-8, so no JSON text, and a lone CBOR break code, so no CBOR
/// item. A reply carrying it fails the link's frame encoding.
const GARBLED: &[u8] = &[0xff];

/// What [`Substitute`] answers a call's `Refusal::Missing` with.
pub(crate) const SUBSTITUTE: Sum = Sum { sum: 42 };

/// The items [`SubstituteStream`] answers a stream's `Refusal::Missing` with.
pub(crate) const SUBSTITUTE_ITEMS: [u32; 3] = [7, 8, 9];

/// The header the headers scenario sends and reads back.
pub(crate) const HEADER: &str = "x-conformance";

/// The server's drain window: longer than any call a scenario holds in flight across it.
const DRAIN: Duration = Duration::from_secs(5);

/// The client module's timeout, which a scenario's own `.timeout(..)` replaces.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Add {
    pub(crate) a: i64,
    pub(crate) b: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sum {
    pub(crate) sum: i64,
}

/// What the server's handlers record for a scenario to read: one per server instance.
#[derive(Clone, Default)]
pub(crate) struct Probe(Arc<ProbeState>);

#[derive(Default)]
pub(crate) struct ProbeState {
    events: AtomicUsize,
    last_event: Mutex<Option<u32>>,
    tallies: AtomicUsize,
    /// The ticking stream's execution's reason, written when the stream is dropped.
    cancelled: Mutex<Option<Option<CancelReason>>>,
    stopped: AtomicBool,
    /// How many calls have reached the handler that never answers.
    unanswered: AtomicUsize,
    /// The stalled call's execution's reason, written when its handler's future is dropped or
    /// returns.
    stalled: Mutex<Option<Option<CancelReason>>>,
    /// When the held call's handler returned its answer, which the link then publishes.
    held_answered: Mutex<Option<Instant>>,
}

impl Probe {
    pub(crate) fn events(&self) -> usize {
        self.0.events.load(Ordering::SeqCst)
    }

    pub(crate) fn last_event(&self) -> Option<u32> {
        *lock(&self.0.last_event)
    }

    pub(crate) fn tallies(&self) -> usize {
        self.0.tallies.load(Ordering::SeqCst)
    }

    pub(crate) fn cancelled(&self) -> Option<Option<CancelReason>> {
        *lock(&self.0.cancelled)
    }

    pub(crate) fn stopped(&self) -> bool {
        self.0.stopped.load(Ordering::SeqCst)
    }

    pub(crate) fn unanswered(&self) -> usize {
        self.0.unanswered.load(Ordering::SeqCst)
    }

    pub(crate) fn stalled(&self) -> Option<Option<CancelReason>> {
        *lock(&self.0.stalled)
    }

    pub(crate) fn held_answered(&self) -> Option<Instant> {
        *lock(&self.0.held_answered)
    }
}

/// Records its execution's cancel reason when dropped: a handler's future dropped at a deadline,
/// or a reply stream dropped at a `cancel`, reports what ended it.
struct Recorder {
    exec: ExecutionRef,
    probe: Probe,
    stream: bool,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let reason = self.exec.cancel_reason();
        if self.stream {
            *lock(&self.probe.0.cancelled) = Some(reason);
            self.probe.0.stopped.store(true, Ordering::SeqCst);
        } else {
            *lock(&self.probe.0.stalled) = Some(reason);
        }
    }
}

/// A handler's own failure: `Conflict` for the domain-error scenario, `BadRequest` for a
/// streamed item that does not decode, `NotFound` for the one an error handler answers with a
/// value of its own.
#[derive(Debug)]
pub(crate) enum Refusal {
    Conflict,
    BadItem,
    Missing,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Conflict => f.write_str("the conformance resource already exists"),
            Refusal::BadItem => f.write_str("a streamed item did not decode"),
            Refusal::Missing => f.write_str("the conformance resource does not exist"),
        }
    }
}

impl Error for Refusal {}

impl Classify for Refusal {
    fn classify(&self) -> ErrorKind {
        match self {
            Refusal::Conflict => ErrorKind::Conflict,
            Refusal::BadItem => ErrorKind::BadRequest,
            Refusal::Missing => ErrorKind::NotFound,
        }
    }
}

/// Whether `err` is a handler's `Refusal::Missing`.
fn missing(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().and_then(|call| call.source_as::<Refusal>()).is_some_and(|refusal| matches!(refusal, Refusal::Missing))
}

/// Answers a `Refusal::Missing` with [`SUBSTITUTE`], encoded by the link's codec; anything else
/// passes on.
struct Substitute;

impl ErrorHandler<Rpc> for Substitute {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        if !missing(&err) {
            return Err(err);
        }
        Ok(cx.reply(&SUBSTITUTE)?)
    }
}

/// Answers a `Refusal::Missing` with a stream of [`SUBSTITUTE_ITEMS`]; anything else passes on.
struct SubstituteStream;

impl ErrorHandler<Rpc> for SubstituteStream {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        if !missing(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::iter(SUBSTITUTE_ITEMS.map(Ok::<_, Refusal>))))
    }
}

/// Refuses every call.
pub(crate) struct Deny;

impl Guard<Rpc> for Deny {
    async fn can_activate(&self, _cx: &RpcCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

#[injectable]
pub(crate) struct CoreController {
    probe: Dep<Probe>,
}

#[routes]
impl CoreController {
    #[ulo_rpc::message("conformance.add")]
    async fn add(&self, req: Payload<Add>) -> Result<Sum, Refusal> {
        Ok(Sum { sum: req.0.a + req.0.b })
    }

    #[ulo_rpc::message("conformance.echo")]
    async fn echo(&self, text: Payload<String>) -> Result<String, Refusal> {
        Ok(text.0)
    }

    #[ulo_rpc::message("conformance.conflict")]
    async fn conflict(&self) -> Result<(), Refusal> {
        Err(Refusal::Conflict)
    }

    #[ulo_rpc::message("conformance.substituted")]
    #[error_handlers(value = Substitute)]
    async fn substituted(&self) -> Result<Sum, Refusal> {
        Err(Refusal::Missing)
    }

    #[ulo_rpc::message("conformance.guarded")]
    #[guards(value = Deny)]
    async fn guarded(&self) -> Result<(), Refusal> {
        Ok(())
    }

    #[ulo_rpc::message("conformance.boom")]
    async fn boom(&self) -> Result<(), Refusal> {
        panic!("the conformance handler panics")
    }

    #[ulo_rpc::message("conformance.headers")]
    async fn headers(&self, headers: CallHeaders) -> Result<Option<String>, Refusal> {
        Ok(headers.get(HEADER).map(str::to_owned))
    }

    /// Answers with what only the call's own context knows: the pattern it named, the link it
    /// arrived on and the header it carried.
    #[ulo_rpc::message("conformance.context")]
    async fn context(&self, cx: RpcCx) -> Result<(String, String, Option<String>), Refusal> {
        Ok((cx.pattern().to_owned(), cx.link().name().to_owned(), cx.headers().get(HEADER).map(str::to_owned)))
    }

    /// Answers with a payload the link's codec cannot encode, the server's own failure.
    #[ulo_rpc::message("conformance.unencodable")]
    async fn unencodable(&self) -> Result<Data, Refusal> {
        Ok(Data::new(GARBLED))
    }

    #[ulo_rpc::event("conformance.event")]
    async fn event(&self, value: Payload<u32>) -> Result<(), Refusal> {
        *lock(&self.probe.0.last_event) = Some(value.0);
        self.probe.0.events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[ulo_rpc::event("conformance.tally")]
    async fn tally(&self, _value: Payload<u32>) -> Result<(), Refusal> {
        self.probe.0.tallies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Waits until its execution is cancelled, by `deadline-ms` or the caller's `cancel`.
    #[ulo_rpc::message("conformance.stall")]
    async fn stall(&self, exec: ExecutionRef) -> Result<(), Refusal> {
        let _recorder = Recorder { exec: exec.clone(), probe: (*self.probe).clone(), stream: false };
        tokio::select! {
            () = exec.cancelled() => {}
            () = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
        Ok(())
    }

    /// Answers nothing while its server serves: it returns when its execution is cancelled or the
    /// server drains, so the server's stop does not wait out the drain window for it.
    #[ulo_rpc::message("conformance.never")]
    async fn never(&self, exec: ExecutionRef) -> Result<(), Refusal> {
        self.probe.0.unanswered.fetch_add(1, Ordering::SeqCst);
        tokio::select! {
            () = exec.cancelled() => {}
            () = exec.draining() => {}
        }
        Ok(())
    }

    #[ulo_rpc::message("conformance.hold")]
    async fn hold(&self, millis: Payload<u64>) -> Result<u64, Refusal> {
        tokio::time::sleep(Duration::from_millis(millis.0)).await;
        *lock(&self.probe.0.held_answered) = Some(Instant::now());
        Ok(millis.0)
    }
}

#[injectable]
pub(crate) struct StreamController {
    probe: Dep<Probe>,
}

#[routes]
impl StreamController {
    #[ulo_rpc::message("conformance.count")]
    async fn count(&self, n: Payload<u32>) -> impl Stream<Item = Result<u32, Refusal>> {
        stream::iter((1..=n.0).map(Ok))
    }

    #[ulo_rpc::message("conformance.sum")]
    async fn sum(&self, items: Inbound<i64>) -> Result<i64, Refusal> {
        let mut items = items;
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item.map_err(|_| Refusal::BadItem)?;
        }
        Ok(total)
    }

    #[ulo_rpc::message("conformance.double")]
    async fn double(&self, items: Inbound<i64>) -> impl Stream<Item = Result<i64, Refusal>> {
        items.map(|item| item.map(|n| n * 2).map_err(|_| Refusal::BadItem))
    }

    #[ulo_rpc::message("conformance.substituted_stream")]
    #[error_handlers(value = SubstituteStream)]
    async fn substituted_stream(&self) -> Result<stream::Empty<Result<u32, Refusal>>, Refusal> {
        Err(Refusal::Missing)
    }

    /// One item, then a payload the link's codec cannot encode.
    #[ulo_rpc::message("conformance.unencodable_stream")]
    async fn unencodable_stream(&self, cx: RpcCx) -> Result<Reply, Refusal> {
        let first = cx.codec().encode(&1u32).expect("a `u32` encodes in every codec");
        let items: BoxStream<'static, Result<Data, BoxError>> = Box::pin(stream::iter([Ok(first), Ok(Data::new(GARBLED))]));
        Ok(Reply::Many(Tracked::new(items, cx.exec().clone())))
    }

    /// Ticks until the stream is dropped, recording the reason when it is.
    #[ulo_rpc::message("conformance.ticks")]
    async fn ticks(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, Refusal>> {
        let recorder = Recorder { exec, probe: (*self.probe).clone(), stream: true };
        stream::unfold((0u64, recorder), |(n, recorder)| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Ok(n), (n + 1, recorder)))
        })
    }

    /// Ticks until the server drains, then ends cleanly.
    #[ulo_rpc::message("conformance.until_drain")]
    async fn until_drain(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, Refusal>> {
        stream::unfold((0u64, exec), |(n, exec)| async move {
            if exec.is_draining() {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            if exec.is_draining() { None } else { Some((Ok(n), (n + 1, exec))) }
        })
    }
}

#[injectable]
pub(crate) struct BinaryController;

#[routes]
impl BinaryController {
    #[ulo_rpc::message("conformance.bytes")]
    async fn bytes(&self, data: Payload<Bytes>) -> Result<Bytes, Refusal> {
        Ok(data.0)
    }
}

/// Which controllers a server mounts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mounts {
    pub(crate) streaming: bool,
    pub(crate) binary: bool,
}

impl Mounts {
    /// What the link carries: every scenario it can run.
    pub(crate) fn carried(capabilities: &Capabilities) -> Mounts {
        Mounts { streaming: streams(capabilities), binary: capabilities.binary }
    }
}

/// Whether the link carries every streamed shape.
pub(crate) fn streams(capabilities: &Capabilities) -> bool {
    [Shape::ServerStreaming, Shape::ClientStreaming, Shape::Bidi].iter().all(|shape| capabilities.shapes.contains(shape))
}

struct ServerRoot {
    probe: Probe,
    mounts: Mounts,
}

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.probe.clone());
        m.controller::<CoreController>();
        if self.mounts.streaming {
            m.controller::<StreamController>();
        }
        if self.mounts.binary {
            m.controller::<BinaryController>();
        }
    }
}

/// One running server instance and what its handlers record.
pub(crate) struct Server {
    pub(crate) handle: AppHandle,
    pub(crate) probe: Probe,
    serving: tokio::task::JoinHandle<()>,
    /// How `serve` returned, once it has: a server whose link failed after `listen` stops serving
    /// here, and [`ready`] reports it.
    ended: Arc<Mutex<Option<String>>>,
}

impl Server {
    pub(crate) async fn stop(self) {
        let _ = self.handle.close(Signal::new("conformance")).await;
        let _ = self.serving.await;
    }
}

async fn bound<B: Broker>(broker: &B, probe: Probe, mounts: Mounts) -> Result<App<Serving>, StartupError> {
    App::builder(ServerRoot { probe, mounts })
        .timer(ulo_tokio::Timer)
        .drain_timeout(DRAIN)
        .wire()?
        .connect()
        .await?
        .bind(ulo_rpc::Server::new(broker.link()))
        .listen()
        .await
}

/// A server mounting what the link carries, serving until stopped.
pub(crate) async fn server<B: Broker>(broker: &B) -> Server {
    let probe = Probe::default();
    let mounts = Mounts::carried(&broker.link().capabilities());
    let app = bound(broker, probe.clone(), mounts)
        .await
        .unwrap_or_else(|error| panic!("the conformance server did not start: {}", report(&error)));
    let handle = app.handle();
    let ended = Arc::new(Mutex::new(None));
    let serving = tokio::spawn({
        let ended = Arc::clone(&ended);
        async move {
            let outcome = match app.serve(std::future::pending::<Signal>()).await {
                Ok(shutdown) => format!("it shut down on `{}`", shutdown.signal),
                Err(error) => format!("its shutdown failed: {}", report(&error)),
            };
            *lock(&ended) = Some(outcome);
        }
    });
    Server { handle, probe, serving, ended }
}

/// Asserts that a server mounting `mounts` is refused at startup as a `Configure` error: what a
/// link's capabilities exclude is refused in `prepare`, before anything binds.
pub(crate) async fn refused_at_startup<B: Broker>(broker: &B, mounts: Mounts) {
    match bound(broker, Probe::default(), mounts).await {
        Err(StartupError::Configure(_)) => {}
        Err(other) => panic!("expected a `Configure` refusal for {mounts:?}, got: {}", report(&other)),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("conformance")).await;
            panic!("a server mounting {mounts:?} started on a link whose capabilities exclude it");
        }
    }
}

/// Imports `RpcClientModule` over the broker's client-side link.
struct ClientRoot<L> {
    link: Mutex<Option<L>>,
}

impl<L: Link> Module for ClientRoot<L> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = lock(&self.link).take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(CALL_TIMEOUT)));
        }
    }
}

/// The caller's side: an app holding the client, kept alive beside it until `close`.
pub(crate) struct Client {
    pub(crate) rpc: RpcClient,
    app: App<Connected>,
}

impl Client {
    /// Closes the client's app, whose `RpcClientModule` closes the link while the broker is still
    /// there to answer it.
    pub(crate) async fn close(self) {
        let _ = self.closed().await;
    }

    /// Closes the client's app as [`close`](Self::close) does, answering its report: a destroy hook
    /// that timed out is one of its failures.
    pub(crate) async fn closed(self) -> Result<Shutdown, ShutdownError> {
        self.app.close(Signal::new("conformance")).await
    }
}

pub(crate) async fn client<B: Broker>(broker: &B) -> Client {
    let app = App::builder(ClientRoot { link: Mutex::new(Some(broker.client_link())) })
        .timer(ulo_tokio::Timer)
        .wire()
        .unwrap_or_else(|error| panic!("the conformance client did not wire: {}", report(&error)))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the conformance client did not connect: {}", report(&error)));
    let rpc =
        app.get::<RpcClient>().await.unwrap_or_else(|error| panic!("the conformance client has no `RpcClient`: {}", report(&error)));
    Client { rpc: (*rpc).clone(), app }
}

/// One scenario's environment: a broker, one server and one client, the server answering.
pub(crate) struct Fixture<B: Broker> {
    pub(crate) broker: B,
    pub(crate) server: Server,
    pub(crate) client: Client,
}

impl<B: Broker> Fixture<B> {
    pub(crate) async fn start() -> Fixture<B> {
        let broker = B::start().await;
        let server = server(&broker).await;
        let client = client(&broker).await;
        ready(&broker, &client.rpc, &[&server]).await;
        Fixture { broker, server, client }
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        self.broker.link().capabilities()
    }

    pub(crate) fn rpc(&self) -> &RpcClient {
        &self.client.rpc
    }

    pub(crate) async fn stop(self) {
        self.server.stop().await;
        self.client.close().await;
    }
}

/// Waits until a unary call succeeds, within the broker's boot budget: a broker subscribes after
/// `bind` returns on some links, and a consumer group takes its partitions later still. On
/// failure it reports the last call's outcome and how each of `servers` stopped serving, if one
/// did.
pub(crate) async fn ready<B: Broker>(broker: &B, rpc: &RpcClient, servers: &[&Server]) {
    let deadline = Instant::now() + broker.budget().boot;
    loop {
        let outcome = rpc.request::<_, Sum>(ADD, &Add { a: 1, b: 2 }).timeout(Duration::from_millis(500)).await;
        match outcome {
            Ok(Sum { sum: 3 }) => return,
            outcome if Instant::now() >= deadline => {
                let stopped: Vec<String> = servers.iter().filter_map(|server| lock(&server.ended).clone()).collect();
                let stopped = if stopped.is_empty() {
                    "every server still serving".to_owned()
                } else {
                    format!("a server stopped serving: {}", stopped.join("; "))
                };
                panic!("the server did not answer within the boot budget: {outcome:?}, {stopped}")
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Polls `done` until it holds or `within` passes; answers whether it held.
pub(crate) async fn eventually(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The wait for an answer that may be the caller's own `Timeout`.
pub(crate) fn miss_wait<B: Broker>(broker: &B) -> Duration {
    broker.budget().settle.max(Duration::from_secs(1))
}

/// Asserts `outcome` failed with `kind`, and returns the error for further checks.
pub(crate) fn failed<T: fmt::Debug>(outcome: Result<T, RpcError>, kind: ErrorKind) -> RpcError {
    match outcome {
        Err(error) if error.kind() == kind => error,
        other => panic!("expected an error of kind {kind:?}, got: {other:?}"),
    }
}

/// Runs `call` to completion within `within`, failing the scenario when it does not.
pub(crate) async fn within<T>(within: Duration, what: &str, call: impl Future<Output = T>) -> T {
    tokio::time::timeout(within, call).await.unwrap_or_else(|_| panic!("{what} did not finish within {within:?}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
