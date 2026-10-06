//! `ulo_rpc::Server<L>`: the core's `Server` for [`Rpc`] over one link (transports DESIGN §5.3).

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::watch;
use tokio::task::JoinSet;
use ulo::{Bound, BoundAddr, BoxError, DrainToken, MountedHandler, Mounted, Shape, TypeName};
use ulo_transport::prepare::{Failure, Failures, Names, zero_bound, zero_count};
use ulo_transport::{Admission, Count};

use crate::__private::{Kind, RpcHandler};
use crate::codec::Codec;
use crate::dispatch::{self, Route, Shared};
use crate::frame::PayloadKind;
use crate::link::{Inbound, Link, Pattern};
use crate::transport::Rpc;

/// `timeout_grace` at `Bound::Default`, the HTTP server's.
const DEFAULT_TIMEOUT_GRACE: Duration = Duration::from_secs(1);

/// The RPC server over link `L`: `app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")))`.
///
/// `prepare` calls `Link::prepare` and `Link::max_inflight`, refuses two handlers for one pattern,
/// a handler whose shape the link's capabilities do not carry (a streamed shape on UDP), an
/// `#[event]` handler with a streamed shape, a `Binary` payload on a link declaring
/// `binary: false`, `max_inflight(Count::Max(0))` and `timeout_grace(Bound::After(Duration::ZERO))`.
/// `bind` calls `Link::listen` with every mounted pattern. Over the in-flight limit a call is
/// answered `err` of kind `unavailable`. A pattern is its own, so a controller's `.at(prefix)`
/// does not apply to it, and `wire()` refuses a prefix on a controller whose handlers are all RPC
/// handlers.
///
/// `serve` runs each call in its own task and keeps serving the calls in flight once the link's
/// inbound stream ends; `drain` is `Link::drain`, after which a new call is answered `err` of kind
/// `unavailable`; `close` aborts what is still running and calls `Link::close`.
pub struct Server<L: Link> {
    pub(crate) link: L,
    pub(crate) max_inflight: Count,
    pub(crate) timeout_grace: Bound,
    /// Set by `prepare`.
    pub(crate) shared: Option<Arc<Shared>>,
    /// Set by `bind`, taken by `serve`.
    pub(crate) inbound: Mutex<Option<Inbound>>,
    /// Raised by `close`; `serve` aborts its calls and returns.
    pub(crate) closing: watch::Sender<bool>,
}

impl<L: Link> Server<L> {
    pub fn new(link: L) -> Self {
        Server {
            link,
            max_inflight: Count::Default,
            timeout_grace: Bound::Default,
            shared: None,
            inbound: Mutex::new(None),
            closing: watch::Sender::new(false),
        }
    }

    /// Calls in flight at once: unbounded at `Count::Default`; over it a call is refused
    /// `unavailable`. `Count::Max(0)` is refused in `prepare`. `prepare` hands it to the link
    /// through `Link::max_inflight`, so on AMQP the per-consumer prefetch follows it, 64 under
    /// `Default` or `Unlimited`.
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
        let limit = match self.max_inflight {
            Count::Max(calls) => Some(calls as usize),
            Count::Default | Count::Unlimited => None,
        };
        self.shared = Some(Arc::new(Shared {
            app: mounted.app().clone(),
            timer: Arc::clone(mounted.timer()),
            routes,
            codec: Codec::of(&capabilities),
            capabilities,
            link: L::NAME,
            admission: Admission::new(limit),
            grace: grace_of(self.timeout_grace),
            calls: Mutex::new(HashMap::new()),
            serial: AtomicU64::new(0),
            unhandled_events: AtomicU64::new(0),
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
        let mut closing = self.closing.subscribe();
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = closed(&mut closing) => {
                    tasks.shutdown().await;
                    return Ok(());
                }
                delivery = inbound.next() => match delivery {
                    Some(delivery) => dispatch::accept(&shared, delivery, &mut tasks),
                    None => break,
                },
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        // The link's own end before the drain is a link failure, which starts the shutdown; after
        // it, the calls in flight finish under the core's drain.
        if !shared.app.is_draining() {
            return Err(format!("the {} link stopped delivering before the drain", L::NAME).into());
        }
        loop {
            tokio::select! {
                biased;
                () = closed(&mut closing) => {
                    tasks.shutdown().await;
                    return Ok(());
                }
                finished = tasks.join_next() => if finished.is_none() {
                    return Ok(());
                },
            }
        }
    }

    async fn drain(&self, token: DrainToken) {
        // No connection here has a terminal execution of its own to open: a call's execution is
        // its own, and the core's drain waits for it.
        let _ = token;
        self.link.drain().await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.closing.send_replace(true);
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

/// Resolves once `close` raised the signal, or the server holding the sender is gone. The
/// borrow `wait_for` answers is dropped here, before anything else awaits.
async fn closed(closing: &mut watch::Receiver<bool>) {
    let _ = closing.wait_for(|closed| *closed).await;
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
