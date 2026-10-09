//! `ulo_rpc::Server<L>`: the core's `Server` for [`Rpc`] over one link (transports DESIGN §5.3).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::pin::pin;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::{Either, select};
use futures_util::{FutureExt, StreamExt};
use ulo::{Bound, BoundAddr, BoxError, DrainToken, MountedHandler, Mounted, Shape, Spawn, TypeName};
use ulo_transport::prepare::{Failure, Failures, Names, zero_bound, zero_count};
use ulo_transport::{Admission, Count, TaskSet};

use crate::__private::{Kind, RpcHandler};
use crate::codec::Codec;
use crate::dispatch::{self, Route, Settling, Shared};
use crate::frame::PayloadKind;
use crate::link::{Delivery, Inbound, Link, Pattern};
use crate::transport::Rpc;
use crate::watch::Watch;

/// `timeout_grace` at `Bound::Default`, the HTTP server's.
const DEFAULT_TIMEOUT_GRACE: Duration = Duration::from_secs(1);

/// The RPC server over link `L`: `app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")))`.
///
/// `prepare` calls `Link::prepare` and `Link::max_inflight`, and refuses two handlers for one
/// pattern, a handler whose shape the link's capabilities do not carry (a streamed shape on UDP),
/// an `#[event]` handler with a streamed shape, a `Binary` payload on a link declaring
/// `binary: false`, `max_inflight(Count::Max(0))` and `timeout_grace(Bound::After(Duration::ZERO))`. `bind` calls `Link::listen` with every mounted
/// pattern. Over the in-flight limit a call is answered `err` of kind `unavailable`. A pattern is
/// its own, so a controller's `.at(prefix)` does not apply to it, and `wire()` refuses a prefix on
/// a controller whose handlers are all RPC handlers.
///
/// `serve` runs each call in its own task, spawned on the app's runtime, and keeps serving the
/// calls in flight once the link's inbound stream ends. `drain` calls `Link::drain`, after which a
/// new call is answered `err` of kind `unavailable`, and then waits until the inbound stream has
/// ended and every such refusal has been sent, so the core's drain window covers a call the link
/// hands over during it; the window's deadline bounds that wait. `close` aborts what is still
/// running and calls `Link::close`; a refusal not yet sent, or a delivery the inbound stream had
/// ready and `serve` had not read, is logged at `warn` with how many there were, since its caller
/// gets no answer from this server.
pub struct Server<L: Link> {
    pub(crate) link: L,
    pub(crate) max_inflight: Count,
    pub(crate) timeout_grace: Bound,
    /// Set by `prepare`.
    pub(crate) shared: Option<Arc<Shared>>,
    /// Set by `bind`, taken by `serve`.
    pub(crate) inbound: Mutex<Option<Inbound>>,
    /// Raised by `close`; `serve` aborts its calls and returns.
    pub(crate) closing: Watch<bool>,
}

impl<L: Link> Server<L> {
    pub fn new(link: L) -> Self {
        Server {
            link,
            max_inflight: Count::Default,
            timeout_grace: Bound::Default,
            shared: None,
            inbound: Mutex::new(None),
            closing: Watch::new(false),
        }
    }

    /// Calls in flight at once: 1,024 at `Count::Default` (`Count::DEFAULT_MAX_INFLIGHT`), none at
    /// `Count::Unlimited`. `Count::Max(0)` is refused in `prepare`. `prepare` hands it to the link
    /// through `Link::max_inflight`. A link declaring `native_backpressure` stops taking requests
    /// from its broker at the bound, AMQP through its prefetch and Kafka by pausing its
    /// partitions, so they wait in the broker; on every other link a request over the bound is
    /// refused `unavailable` with a `RetryAfter` detail.
    pub fn max_inflight(mut self, calls: Count) -> Self {
        self.max_inflight = calls;
        self
    }

    /// How long the error handlers may take with the `Timeout` a passed `deadline-ms` offers them
    /// before the `err` of kind `timeout` is sent instead: one second at `Bound::Default`, timed
    /// by the app's `Timer`; `Bound::Unbounded` waits for them. `Bound::After(Duration::ZERO)` is
    /// refused in `prepare`.
    pub fn timeout_grace(mut self, grace: Bound) -> Self {
        self.timeout_grace = grace;
        self
    }
}

impl<L: Link> ulo::Server for Server<L> {
    type Transport = Rpc;

    async fn prepare(&mut self, mounted: Mounted<'_, Rpc>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        if let Err(error) = self.link.prepare(mounted.app()).await {
            failures.push_error(error);
        }
        self.link.max_inflight(self.max_inflight);
        failures.extend(zero_count("max_inflight", self.max_inflight, "shed every call"));
        failures.extend(zero_bound(
            "timeout_grace",
            self.timeout_grace,
            "send the `timeout` before any error handler could answer a passed deadline",
        ));
        let capabilities = self.link.capabilities();
        let mut routes: HashMap<String, Route> = HashMap::new();
        for handler in mounted.handlers() {
            let who = Who::of(handler);
            let Some(rpc) = handler.handler::<RpcHandler>() else {
                failures.push(Failure::naming(vec![who.controller], move |names| {
                    format!("{} was not mounted by `#[ulo_rpc::message]` or `#[ulo_rpc::event]`", who.text(names))
                }));
                continue;
            };
            let pattern = rpc.pattern;
            let shape = handler.info().shape();
            if rpc.kind == Kind::Event && shape != Shape::Unary {
                failures.push(Failure::naming(vec![who.controller], move |names| {
                    format!(
                        "{} (pattern `{pattern}`) is an `#[event]` handler with a {} shape; an event is one payload answered by nothing",
                        who.text(names),
                        shape_name(shape)
                    )
                }));
            }
            if !capabilities.shapes.contains(&shape) {
                failures.push(Failure::naming(vec![who.controller], move |names| {
                    format!(
                        "{} (pattern `{pattern}`) is a {} handler, which the {} link does not carry",
                        who.text(names),
                        shape_name(shape),
                        L::NAME
                    )
                }));
            }
            if rpc.payload == PayloadKind::Binary && !capabilities.binary {
                failures.push(Failure::naming(vec![who.controller], move |names| {
                    format!(
                        "{} (pattern `{pattern}`) takes a binary payload, which the {} link's JSON codec cannot carry; \
                         set `.codec(Codec::Cbor)` on the link",
                        who.text(names),
                        L::NAME
                    )
                }));
            }
            match routes.entry(pattern.to_owned()) {
                Entry::Occupied(taken) => {
                    let other = Who::of(&taken.get().handler);
                    failures.push(Failure::naming(vec![other.controller, who.controller], move |names| {
                        format!("{} and {} both handle pattern `{pattern}`", other.text(names), who.text(names))
                    }));
                }
                Entry::Vacant(slot) => {
                    slot.insert(Route { handler: handler.clone(), call: Arc::clone(&rpc.call), kind: rpc.kind, shape });
                }
            }
        }
        failures.into_result()?;
        let limit = self.max_inflight.max_inflight();
        self.shared = Some(Arc::new(Shared {
            app: mounted.app().clone(),
            timer: Arc::clone(mounted.timer()),
            runtime: Arc::clone(mounted.runtime()),
            routes,
            codec: Codec::of(&capabilities),
            capabilities,
            link: L::NAME,
            admission: Admission::new(limit),
            grace: grace_of(self.timeout_grace),
            calls: Mutex::new(HashMap::new()),
            serial: AtomicU64::new(0),
            unhandled_events: AtomicU64::new(0),
            settling: Watch::new(Settling::default()),
        }));
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Rpc>) -> Result<(), BoxError> {
        let Some(shared) = &self.shared else {
            return Err("the RPC server was bound before `prepare` built its routes".into());
        };
        let mut patterns: Vec<Pattern> = shared.routes.keys().map(|pattern| Pattern::from(pattern.as_str())).collect();
        patterns.sort();
        let inbound = self.link.listen(&patterns).await?;
        *self.inbound.lock().unwrap_or_else(PoisonError::into_inner) = Some(inbound);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(shared) = self.shared.clone() else {
            return Ok(());
        };
        // A second `serve`, or one after `close`, finds nothing to take and returns at once.
        let Some(mut inbound) = self.inbound.lock().unwrap_or_else(PoisonError::into_inner).take() else {
            return Ok(());
        };
        let _reading = Reader::start(&shared);
        let runtime: Arc<dyn Spawn> = shared.runtime.clone();
        let mut tasks = TaskSet::new(runtime);
        loop {
            match reading(&self.closing, &mut inbound, &mut tasks).await {
                Reading::Closed => {
                    let unread = unread(&mut inbound);
                    abandon(&shared, &mut tasks, unread).await;
                    return Ok(());
                }
                Reading::Delivered(Some(delivery)) => dispatch::accept(&shared, delivery, &mut tasks),
                Reading::Delivered(None) => break,
                Reading::Reaped => {}
            }
        }
        shared.settling.modify(|settling| settling.ended = true);
        // The link's own end before the drain is a link failure, which starts the shutdown; after
        // it, the calls in flight finish under the core's drain.
        if !shared.app.is_draining() {
            return Err(format!("the {} link stopped delivering before the drain", L::NAME).into());
        }
        loop {
            match finishing(&self.closing, &mut tasks).await {
                Finishing::Closed => {
                    abandon(&shared, &mut tasks, 0).await;
                    return Ok(());
                }
                Finishing::Finished(None) => return Ok(()),
                Finishing::Finished(Some(_)) => {}
            }
        }
    }

    async fn drain(&self, token: DrainToken) {
        // No connection here has a terminal execution of its own to open: a call's execution is
        // its own, and the core's drain waits for it.
        let _ = token;
        self.link.drain().await;
        // What reaches the server during the drain opens no execution, so the core's wait for
        // live executions does not cover it. The link ends its inbound stream once nothing more
        // will arrive; the core drops this future at the drain's deadline.
        if let Some(shared) = &self.shared {
            shared.settling.wait_for(Settling::settled).await;
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.closing.modify(|closing| *closing = true);
        self.inbound.lock().unwrap_or_else(PoisonError::into_inner).take();
        self.link.close().await
    }

    /// What the link reports, so a TCP or UDP server on port 0 shows its port.
    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}

/// `timeout_grace` as a duration, one second at `Bound::Default`; `None` for `Bound::Unbounded`.
fn grace_of(bound: Bound) -> Option<Duration> {
    match bound {
        Bound::Default => Some(DEFAULT_TIMEOUT_GRACE),
        Bound::After(grace) => Some(grace),
        Bound::Unbounded => None,
    }
}

/// Resolves once `close` raised the signal.
async fn closed(closing: &Watch<bool>) {
    closing.wait_for(|closed| *closed).await;
}

/// What one turn of `serve`'s reading loop found.
enum Reading {
    Closed,
    Delivered(Option<Delivery>),
    Reaped,
}

/// One turn of `serve`'s loop while the inbound stream is read, polled in this order: `close`'s
/// signal, the next delivery, and the end of a task while any is running, which only reaps it.
/// The losers are dropped when the turn returns, which loses nothing: the stream's `next` and
/// `join_next` take nothing out until they answer.
async fn reading(closing: &Watch<bool>, inbound: &mut Inbound, tasks: &mut TaskSet) -> Reading {
    let running = !tasks.is_empty();
    let reaping = async {
        if running {
            tasks.join_next().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    match select(pin!(closed(closing)), select(inbound.next(), pin!(reaping))).await {
        Either::Left(_) => Reading::Closed,
        Either::Right((Either::Left((delivery, _)), _)) => Reading::Delivered(delivery),
        Either::Right((Either::Right(_), _)) => Reading::Reaped,
    }
}

/// What one turn of `serve`'s loop found once the inbound stream has ended.
enum Finishing {
    Closed,
    Finished(Option<ulo::TaskEnd>),
}

/// One turn once the inbound stream has ended: `close`'s signal first, then the next task's end.
async fn finishing(closing: &Watch<bool>, tasks: &mut TaskSet) -> Finishing {
    match select(pin!(closed(closing)), pin!(tasks.join_next())).await {
        Either::Left(_) => Finishing::Closed,
        Either::Right((finished, _)) => Finishing::Finished(finished),
    }
}

/// Marks the inbound stream read while `serve` runs, and not read once it returns or is dropped,
/// so a drain does not wait for a stream nothing reads.
struct Reader(Arc<Shared>);

impl Reader {
    fn start(shared: &Arc<Shared>) -> Reader {
        shared.settling.modify(|settling| settling.reading = true);
        Reader(Arc::clone(shared))
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.settling.modify(|settling| settling.reading = false);
    }
}

/// How many deliveries `inbound` has ready, taken and dropped unread. A dropped delivery's `Ack`
/// settles nothing, so a broker holding unacknowledged messages redelivers it.
fn unread(inbound: &mut Inbound) -> usize {
    let mut unread = 0;
    while let Some(Some(_)) = inbound.next().now_or_never() {
        unread += 1;
    }
    unread
}

/// `close`'s end of `serve`: aborts every task still running, after logging the refusals not yet
/// sent and the `unread` deliveries, whose callers this server leaves unanswered. A call still
/// running was cancelled at the drain's end and is counted in the shutdown's report.
async fn abandon(shared: &Shared, tasks: &mut TaskSet, unread: usize) {
    let refusals = shared.settling.read(|settling| settling.refusals);
    if refusals > 0 || unread > 0 {
        tracing::warn!(
            link = shared.link,
            refusals_unsent = refusals,
            deliveries_unread = unread,
            "the RPC server closed before answering every call that reached it; the drain's deadline passed first"
        );
    }
    tasks.abort_all();
    tasks.join_all().await;
}

/// A handler as a `prepare` failure names it, `` `Invoices::create` ``.
#[derive(Clone, Copy)]
struct Who {
    controller: TypeName,
    name: &'static str,
}

impl Who {
    fn of(handler: &MountedHandler<Rpc>) -> Who {
        Who { controller: handler.controller().key().type_name(), name: handler.name() }
    }

    fn text(&self, names: &Names<'_>) -> String {
        format!("`{}::{}`", names.of(self.controller), self.name)
    }
}

fn shape_name(shape: Shape) -> &'static str {
    match shape {
        Shape::Unary => "unary",
        Shape::ServerStreaming => "server-streaming",
        Shape::ClientStreaming => "client-streaming",
        Shape::Bidi => "bidirectional-streaming",
        _ => "streaming",
    }
}
