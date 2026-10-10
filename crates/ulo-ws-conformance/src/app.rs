//! The application every scenario runs: its gateways, declared once per `Port`, and the [`Probe`]
//! its hooks and handlers write to, which a scenario reads back.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use event_listener::Event;
use futures_util::future::{Either, select};
use futures_util::{Stream, stream};
use ulo::{BoxError, ExecutionRef, Guard, Interceptor, Module, ModuleDef, ModuleIdentity, Next, StreamOutcome, Timer};
use ulo_transport::{Classify, ErrorKind};
use ulo_ws::{ConnectCx, ConnectRefused, DisconnectReason, Frame, Port, Reply, Ws, WsConnect, WsCx, WsModule};

use crate::PATIENCE;

/// The upgrade header `/strict`'s connect guard admits on, and the value it admits.
pub(crate) const TOKEN: (&str, &str) = ("x-token", "open");

/// The upgrade header whose absence `/strict`'s `OnConnect` answers `Unauthorized`.
pub(crate) const USER: &str = "x-user";

/// The reason `/strict`'s `OnConnect` refuses with.
pub(crate) const WHO: &str = "who are you?";

/// The upgrade header naming how `/refused` refuses the connection: [`BY_GUARD`], [`BUSY`],
/// [`FAULT`], [`PANIC`] or [`OWN_CODE`]. Absent, it admits.
pub(crate) const REFUSE: &str = "x-refuse";
pub(crate) const BY_GUARD: &str = "guard";
pub(crate) const BUSY: &str = "busy";
pub(crate) const FAULT: &str = "fault";
pub(crate) const PANIC: &str = "panic";
pub(crate) const OWN_CODE: &str = "code";

/// The close code and reason `/refused`'s `OnConnect` sets with `ConnectRefused::code`: the one
/// graphql-transport-ws's reference server refuses a connection with when no subprotocol agreed.
pub(crate) const OWN: (u16, &str) = (4406, "no subprotocol agreed");

/// `/small`'s `message_limit`, in bytes.
pub(crate) const MESSAGE_LIMIT: usize = 128;

/// The two `/streams` events whose stream end a scenario reads.
pub(crate) const WRITTEN: &str = "written";
pub(crate) const DISCARDED: &str = "discarded";

/// `fut`'s output, or a failure naming `what` once [`PATIENCE`] has passed on `timer`'s clock.
pub(crate) async fn within<F: Future>(timer: &dyn Timer, what: &str, fut: F) -> F::Output {
    match select(pin!(fut), timer.sleep(PATIENCE)).await {
        Either::Left((output, _)) => output,
        Either::Right(_) => panic!("{what} did not happen within {PATIENCE:?}"),
    }
}

/// Items the application writes while a scenario runs, which the scenario reads back or waits for.
pub(crate) struct Record<T> {
    inner: Arc<RecordInner<T>>,
}

struct RecordInner<T> {
    items: Mutex<Vec<T>>,
    pushed: Event,
}

impl<T> Clone for Record<T> {
    fn clone(&self) -> Self {
        Record { inner: Arc::clone(&self.inner) }
    }
}

impl<T: Clone> Record<T> {
    fn new() -> Self {
        Record { inner: Arc::new(RecordInner { items: Mutex::new(Vec::new()), pushed: Event::new() }) }
    }

    pub(crate) fn push(&self, item: T) {
        self.inner.items.lock().unwrap_or_else(PoisonError::into_inner).push(item);
        self.inner.pushed.notify(usize::MAX);
    }

    pub(crate) fn snapshot(&self) -> Vec<T> {
        self.inner.items.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Every item once `done` holds of them, failing after [`PATIENCE`].
    pub(crate) async fn until(&self, timer: &dyn Timer, what: &str, done: impl Fn(&[T]) -> bool) -> Vec<T> {
        within(timer, what, async {
            loop {
                // Registered before the items are read, so a push in between wakes it.
                let pushed = self.inner.pushed.listen();
                let items = self.snapshot();
                if done(&items) {
                    return items;
                }
                pushed.await;
            }
        })
        .await
    }

    /// Every item once at least `n` have been written.
    pub(crate) async fn at_least(&self, timer: &dyn Timer, n: usize, what: &str) -> Vec<T> {
        self.until(timer, what, |items| items.len() >= n).await
    }
}

/// A gate `/serial`'s `hold` waits behind, opened by its `open` or by the scenario, and the order
/// things happened in.
#[derive(Clone)]
pub(crate) struct Gate {
    opened: Arc<(AtomicBool, Event)>,
    pub(crate) log: Record<&'static str>,
}

impl Gate {
    pub(crate) fn open(&self, by: &'static str) {
        self.log.push(by);
        self.opened.0.store(true, Ordering::SeqCst);
        self.opened.1.notify(usize::MAX);
    }

    async fn hold(&self) -> &'static str {
        self.log.push("hold started");
        loop {
            let opened = self.opened.1.listen();
            if self.opened.0.load(Ordering::SeqCst) {
                break;
            }
            opened.await;
        }
        self.log.push("hold done");
        "held"
    }
}

/// What a `/streams` execution reported: its stream's outcome, and that the execution ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamReport {
    Outcome(&'static str, StreamOutcome),
    Ended(&'static str),
}

/// Pushes `Ended` when the execution drops the `on_stream_end` callback holding it, whether or
/// not the callback ran: the positive signal a scenario waits for before reading an absence.
struct Reporter {
    event: &'static str,
    record: Record<StreamReport>,
}

impl Drop for Reporter {
    fn drop(&mut self) {
        self.record.push(StreamReport::Ended(self.event));
    }
}

/// What the application records for a scenario: each `on_disconnect` as (path, reason), the gate,
/// and the stream reports.
#[derive(Clone)]
pub(crate) struct Probe {
    pub(crate) departures: Record<(String, DisconnectReason)>,
    pub(crate) gate: Gate,
    pub(crate) streams: Record<StreamReport>,
}

impl Probe {
    pub(crate) fn new() -> Self {
        Probe {
            departures: Record::new(),
            gate: Gate { opened: Arc::new((AtomicBool::new(false), Event::new())), log: Record::new() },
            streams: Record::new(),
        }
    }

    fn report(&self, event: &'static str, exec: &ExecutionRef) {
        let reporter = Reporter { event, record: self.streams.clone() };
        exec.on_stream_end(move |outcome| reporter.record.push(StreamReport::Outcome(reporter.event, outcome)));
    }

    fn depart(&self, path: &str, why: DisconnectReason) {
        self.departures.push((path.to_owned(), why));
    }
}

/// A handler's failure in a stream item, which no scenario provokes.
#[derive(Debug)]
pub(crate) struct Fault;

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fault")
    }
}

impl Error for Fault {}

impl Classify for Fault {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

fn ticks() -> impl Stream<Item = Result<u32, Fault>> {
    stream::iter([Ok(1), Ok(2)])
}

/// Admits an upgrade whose header `name` equals `value`.
pub(crate) struct HeaderIs {
    name: &'static str,
    value: &'static str,
}

impl Guard<WsConnect> for HeaderIs {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(cx.head().headers().get(self.name).is_some_and(|value| value == self.value))
    }
}

/// Refuses an upgrade whose [`REFUSE`] header names [`BY_GUARD`].
pub(crate) struct NotRefusedByGuard;

impl Guard<WsConnect> for NotRefusedByGuard {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(cx.head().headers().get(REFUSE).is_none_or(|how| how != BY_GUARD))
    }
}

/// `/strict`'s connection phase: no [`USER`] header is `Unauthorized`.
fn strict(cx: &ConnectCx) -> Result<(), ConnectRefused> {
    match cx.head().headers().get(USER) {
        Some(_) => Ok(()),
        None => Err(ConnectRefused::kind(ErrorKind::Unauthorized, WHO)),
    }
}

/// `/refused`'s connection phase, by its [`REFUSE`] header.
fn refused(cx: &ConnectCx) -> Result<(), ConnectRefused> {
    let how = cx.head().headers().get(REFUSE).and_then(|how| how.to_str().ok()).unwrap_or_default();
    match how {
        BUSY => Err(ConnectRefused::kind(ErrorKind::TooManyRequests, "try again later")),
        FAULT => Err(ConnectRefused::kind(ErrorKind::Internal, "the connect hook failed")),
        PANIC => panic!("the connect hook panicked, as the scenario asked"),
        OWN_CODE => Err(ConnectRefused::code(OWN.0, OWN.1).unwrap_or_else(|error| panic!("{} is a close code a frame carries: {error}", OWN.0))),
        _ => Ok(()),
    }
}

/// Runs the message, drops the stream it answers and answers one frame instead.
pub(crate) struct AnswerOne;

impl Interceptor<Ws> for AnswerOne {
    async fn intercept(&self, _cx: &WsCx, next: Next<'_, Ws>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(Reply::One(Frame::text("0")))
    }
}

/// `Connection::send` `n` times, the frames `"1"` to `"n"` as they stand.
async fn burst(cx: &WsCx, n: u32) {
    for i in 1..=n {
        let _ = cx.conn().send(Frame::text(i.to_string())).await;
    }
}

/// The suite's gateways, each served on the port `$port` names. Every limit is the gateway's own,
/// so the scenarios read the same settings on every server whatever defaults it carries.
macro_rules! gateways {
    ($port:ident) => {
        use std::time::Duration;

        use futures_util::Stream;
        use ulo::{Dep, ExecutionRef, ModuleDef, injectable, routes};
        use ulo_ws::{ConnectCx, ConnectRefused, Connection, DisconnectReason, OnConnect, OnDisconnect, Payload, UpgradeHead, WsCx};

        use super::{AnswerOne, Fault, HeaderIs, NotRefusedByGuard, Probe, TOKEN};

        /// The plain gateway: a round trip, a stream, and every `on_disconnect` recorded.
        #[injectable]
        pub(crate) struct Echo {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(path = "/echo", port = $port)]
        impl Echo {
            #[ulo_ws::message("echo")]
            fn echo(&self, text: Payload<String>) -> String {
                text.0
            }

            #[ulo_ws::message("count")]
            fn count(&self, up_to: Payload<u32>) -> impl Stream<Item = Result<u32, Fault>> {
                futures_util::stream::iter((1..=up_to.0).map(Ok))
            }
        }

        /// Speaks two versions of a protocol; the 101 echoes the first it lists among those the
        /// client offered.
        #[injectable]
        pub(crate) struct Versioned;

        #[routes]
        #[ulo_ws::gateway(path = "/versioned", port = $port, subprotocols = ["v2.chat", "v1.chat"])]
        impl Versioned {
            #[ulo_ws::message("which")]
            fn which(&self, head: Dep<UpgradeHead>) -> Option<String> {
                head.subprotocol().map(str::to_owned)
            }
        }

        /// Refuses before the upgrade: no token is 403, a token without a user is 401.
        #[injectable]
        pub(crate) struct Strict;

        #[routes]
        #[ulo_ws::gateway(
            path = "/strict",
            port = $port,
            refuse = handshake,
            connect_guards(value = HeaderIs { name: TOKEN.0, value: TOKEN.1 })
        )]
        impl Strict {
            #[ulo_ws::message("echo")]
            fn echo(&self, text: Payload<String>) -> String {
                text.0
            }
        }

        impl OnConnect for Strict {
            async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
                super::strict(cx)
            }
        }

        /// Refuses after the upgrade, as its `x-refuse` header names.
        #[injectable]
        pub(crate) struct Refused {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(path = "/refused", port = $port, connect_guards(value = NotRefusedByGuard))]
        impl Refused {
            #[ulo_ws::message("echo")]
            fn echo(&self, text: Payload<String>) -> String {
                text.0
            }
        }

        impl OnConnect for Refused {
            async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
                super::refused(cx)
            }
        }

        /// A 128-byte message limit and room for one connection.
        #[injectable]
        pub(crate) struct Small {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(path = "/small", port = $port, message_limit = 128, max_connections = 1)]
        impl Small {
            #[ulo_ws::message("echo")]
            fn echo(&self, text: Payload<String>) -> String {
                text.0
            }
        }

        /// One message in flight per connection.
        #[injectable]
        pub(crate) struct Serial {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(path = "/serial", port = $port, max_inflight = 1)]
        impl Serial {
            #[ulo_ws::message("hold")]
            async fn hold(&self) -> &'static str {
                self.probe.gate.hold().await
            }

            #[ulo_ws::message("open")]
            fn open(&self) -> &'static str {
                self.probe.gate.open("opened by a message");
                "opened"
            }

            #[ulo_ws::message("noop")]
            fn noop(&self) {}
        }

        /// One queued outbound message per connection.
        #[injectable]
        pub(crate) struct Outbound {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(path = "/outbound", port = $port, max_outbound = 1)]
        impl Outbound {
            #[ulo_ws::message("burst")]
            async fn burst(&self, n: Payload<u32>, cx: WsCx) {
                super::burst(&cx, n.0).await;
            }

            #[ulo_ws::message("count")]
            fn count(&self, up_to: Payload<u32>) -> impl Stream<Item = Result<u32, Fault>> {
                futures_util::stream::iter((1..=up_to.0).map(Ok))
            }

            #[ulo_ws::message("echo")]
            fn echo(&self, text: Payload<String>) -> String {
                text.0
            }
        }

        /// A Ping every 100 ms, and 150 ms for its Pong.
        #[injectable]
        pub(crate) struct Heartbeat {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(
            path = "/heartbeat",
            port = $port,
            ping_interval = ulo::Bound::After(Duration::from_millis(100)),
            pong_timeout = ulo::Bound::After(Duration::from_millis(150)),
        )]
        impl Heartbeat {
            #[ulo_ws::message("echo")]
            fn echo(&self, text: Payload<String>) -> String {
                text.0
            }
        }

        /// Two streamed answers reporting their end: one written, one an interceptor discards.
        #[injectable]
        pub(crate) struct Streams {
            probe: Dep<Probe>,
        }

        #[routes]
        #[ulo_ws::gateway(path = "/streams", port = $port)]
        impl Streams {
            #[ulo_ws::message("written")]
            fn written(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Fault>> {
                self.probe.report(super::WRITTEN, &exec);
                super::ticks()
            }

            #[ulo_ws::message("discarded")]
            #[interceptors(value = AnswerOne)]
            fn discarded(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Fault>> {
                self.probe.report(super::DISCARDED, &exec);
                super::ticks()
            }
        }

        impl OnDisconnect for Echo {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.probe.depart(conn.info().path(), why);
            }
        }

        impl OnDisconnect for Refused {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.probe.depart(conn.info().path(), why);
            }
        }

        impl OnDisconnect for Small {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.probe.depart(conn.info().path(), why);
            }
        }

        impl OnDisconnect for Outbound {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.probe.depart(conn.info().path(), why);
            }
        }

        impl OnDisconnect for Heartbeat {
            async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
                self.probe.depart(conn.info().path(), why);
            }
        }

        pub(crate) fn register(m: &mut ModuleDef<'_>) {
            m.controller::<Echo>();
            m.controller::<Versioned>();
            m.controller::<Strict>();
            m.controller::<Refused>();
            m.controller::<Small>();
            m.controller::<Serial>();
            m.controller::<Outbound>();
            m.controller::<Heartbeat>();
            m.controller::<Streams>();
        }
    };
}

/// The gateways a standalone server serves.
mod own {
    gateways!(own);
}

/// The gateways the HTTP server's port serves.
mod http {
    gateways!(http);
}

/// The suite's root module: `WsModule`, the probe, and the gateways on `port`.
pub(crate) struct SuiteModule {
    pub(crate) probe: Probe,
    pub(crate) port: Port,
}

impl Module for SuiteModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(self.probe.clone());
        match self.port {
            Port::Own => own::register(m),
            Port::Http => http::register(m),
        }
    }
}
