//! Scenarios on the link itself, below the server and the client: a `cancel` sent after its
//! request reaches the server after it, on a link declaring `ordered_control`; and a streamed
//! request cancelled before the server acknowledged its `open` reaches the server as the `open`
//! and then the `cancel`, so the server holds nothing for it.
//!
//! On a link that holds the `cancel`, the held `cancel` goes out when an `opened` arrives late,
//! after the call ended, and is dropped with its entry once the link's hold runs out, so a call
//! whose `opened` never comes holds nothing until the link closes.
//!
//! On a link without `ordered_control` a request's control frames travel a lane of their own and
//! the broker can deliver a `cancel` ahead of its request, so there the first scenario's order is
//! not observable and the link declares it not applicable.

use std::collections::HashSet;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Shape, Signal};
use ulo::app::Connected;
use ulo_rpc::__private::ClientProbe;
use ulo_rpc::{CallHeaders, Codec, Delivery, Frame, Link, Outbound, Pattern, ReplyTo};

use crate::{Broker, report};

/// The pattern the requests name; the server's link listens on it and nothing handles it.
const ORDERED: &str = "conformance.ordered";

/// Request and `cancel` pairs sent, each pair's two sends started together.
const ROUNDS: u64 = 200;

/// Streamed requests cancelled as they open.
const OPENS: u64 = 20;

/// How long the server's link waits for each frame.
const PATIENCE: Duration = Duration::from_secs(5);

/// The hold `hold_runs_out` gives a held `cancel`, in place of the link's own, and how late
/// `late_opened`'s `opened` arrives.
const SHORT_HOLD: Duration = Duration::from_millis(300);

/// How often a scenario reads the client probe while it waits on it.
const PROBE_POLL: Duration = Duration::from_millis(10);

/// An app with nothing in it, whose handle the server's link is prepared with.
struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// A server's link listening on [`ORDERED`] and a client's link connected to it, with no server
/// or client above them.
struct Linked<B: Broker> {
    _broker: B,
    app: App<Connected>,
    server: B::Link,
    inbound: BoxStream<'static, Delivery>,
    client: B::Link,
    outbound: Outbound,
}

impl<B: Broker> Linked<B> {
    async fn start() -> Self {
        let broker = B::start().await;
        let app = App::builder(Empty)
            .runtime(ulo_tokio::Tokio::current())
            .wire()
            .unwrap_or_else(|error| crate::startup_failed!("the ordering scenario's app did not wire: {}", report(&error)))
            .connect()
            .await
            .unwrap_or_else(|error| crate::startup_failed!("the ordering scenario's app did not connect: {}", report(&error)));
        let mut server = broker.link();
        server.prepare(&app.handle()).await.unwrap_or_else(|error| crate::startup_failed!("the server's link did not prepare: {error}"));
        let inbound = server
            .listen(&[Pattern::from(ORDERED)])
            .await
            .unwrap_or_else(|error| crate::startup_failed!("the server's link did not listen: {error}"));
        let client = broker.client_link(&server.bound());
        let outbound = client.connect().await.unwrap_or_else(|error| crate::startup_failed!("the client's link did not connect: {error}"));
        Linked { _broker: broker, app, server, inbound, client, outbound }
    }

    /// Reads `rounds` calls opened by `opens` frames, each followed by its `cancel`, from the
    /// server's link. A `cancel` reaching the server first names no call it holds and is dropped,
    /// and a `cancel` never sent leaves its call held, so either shows as a call with no `cancel`
    /// after it.
    async fn read_pairs(&mut self, rounds: u64, opens: &str) {
        let (mut opened, mut cancels) = (0u64, 0u64);
        let mut held = HashSet::new();
        while opened + cancels < 2 * rounds {
            let Ok(delivered) = tokio::time::timeout(PATIENCE, self.inbound.next()).await else {
                panic!(
                    "{opened} {opens} and {cancels} cancels of {rounds} each reached the server within {PATIENCE:?} of the last; the \
                     server holds {held:?}"
                );
            };
            let delivery = delivered.expect("the server's inbound stream ended");
            match delivery.frame {
                Frame::Req { id, .. } | Frame::Open { id, .. } => {
                    held.insert(id);
                    opened += 1;
                }
                Frame::Cancel { id } => {
                    assert!(held.remove(&id), "the server received a cancel for a call it does not hold: {id}");
                    cancels += 1;
                }
                other => panic!("the server received a frame other than {opens} or a cancel: {other:?}"),
            }
        }
        assert!(held.is_empty(), "calls with no cancel after them: {held:?}");
    }

    async fn stop(self) {
        let _ = self.client.close().await;
        let _ = self.server.close().await;
        let _ = self.app.close(Signal::new("conformance")).await;
    }
}

/// Sends `ROUNDS` requests, each followed by its `cancel`, the two sends of a round polled together
/// and the request's first, and requires the server's link to deliver every request and then its
/// `cancel`. A link without `ordered_control` fails here, and declares the scenario not applicable.
pub async fn cancel_follows_its_request<B: Broker>() {
    let mut linked = Linked::<B>::start().await;
    assert!(
        linked.server.capabilities().ordered_control,
        "the link does not declare `ordered_control`, so its `cancel` may overtake its request: declare \
         `cancel_follows_its_request` not applicable with the reason"
    );
    let pattern = Pattern::from(ORDERED);
    let codec = if linked.server.capabilities().binary { Codec::Cbor } else { Codec::Json };
    let null = codec.encode(&()).expect("a unit encodes");
    for id in 1..=ROUNDS {
        let request = Frame::Req { id, pattern: ORDERED.to_owned(), headers: CallHeaders::new(), data: null.clone() };
        let request = (linked.outbound.send)(pattern.clone(), request, Some(ReplyTo { id }));
        let cancel = (linked.outbound.send)(pattern.clone(), Frame::Cancel { id }, None);
        let (requested, cancelled) = futures_util::join!(request, cancel);
        requested.unwrap_or_else(|error| panic!("request {id} was not sent: {error}"));
        cancelled.unwrap_or_else(|error| panic!("the cancel of request {id} was not sent: {error}"));
    }
    linked.read_pairs(ROUNDS, "requests").await;
    linked.stop().await;
}

/// Opens `OPENS` streamed requests, each cancelled in the same poll as its `open`, before the
/// server can have acknowledged it, and requires the server's link to deliver every `open` and
/// then its `cancel`, holding no call afterwards. A link holding a streamed request's control
/// frames until the server acknowledges the `open` holds the `cancel` too, and sends it once the
/// acknowledgment arrives. A link carrying no streamed request fails here, and declares the
/// scenario not applicable.
pub async fn cancel_before_opened<B: Broker>() {
    let mut linked = Linked::<B>::start().await;
    let shapes = linked.server.capabilities().shapes;
    assert!(
        shapes.contains(&Shape::ClientStreaming) || shapes.contains(&Shape::Bidi),
        "the link carries no streamed request: declare `cancel_before_opened` not applicable with the reason"
    );
    let pattern = Pattern::from(ORDERED);
    for id in 1..=OPENS {
        let open = Frame::Open { id, pattern: ORDERED.to_owned(), headers: CallHeaders::new() };
        let open = (linked.outbound.send)(pattern.clone(), open, Some(ReplyTo { id }));
        let cancel = (linked.outbound.send)(pattern.clone(), Frame::Cancel { id }, None);
        let (opened, cancelled) = futures_util::join!(open, cancel);
        opened.unwrap_or_else(|error| panic!("the open of call {id} was not sent: {error}"));
        cancelled.unwrap_or_else(|error| panic!("the cancel of call {id} was not sent: {error}"));
    }
    linked.read_pairs(OPENS, "opens").await;
    linked.stop().await;
}

/// The client probe of `B`'s client link, or a failure naming the declaration.
fn probe<B: Broker>(linked: &Linked<B>) -> &ClientProbe {
    B::probe(&linked.client).unwrap_or_else(|| {
        panic!(
            "the environment reads no client probe, so the link holds no `cancel` for an `opened`: declare the held-cancel \
             scenarios not applicable with the reason"
        )
    })
}

/// Waits until `done` holds of the probe, read every 10 ms, failing after [`PATIENCE`].
async fn probe_until(probe: &ClientProbe, what: &str, done: impl Fn(&ClientProbe) -> bool) {
    let waited = tokio::time::timeout(PATIENCE, async {
        while !done(probe) {
            tokio::time::sleep(PROBE_POLL).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what} did not happen within {PATIENCE:?}");
}

impl<B: Broker> Linked<B> {
    /// Opens streamed request `id` with the server's `opened` withheld from the client's link,
    /// waits until the server holds the call and its `opened` has reached the client, and cancels
    /// the call, as a call's timeout or drop does: the client's table then holds the `cancel` alone.
    async fn cancel_with_opened_withheld(&mut self, id: u64) {
        probe(self).withhold();
        let pattern = Pattern::from(ORDERED);
        let open = Frame::Open { id, pattern: ORDERED.to_owned(), headers: CallHeaders::new() };
        (self.outbound.send)(pattern.clone(), open, Some(ReplyTo { id }))
            .await
            .unwrap_or_else(|error| panic!("the open of call {id} was not sent: {error}"));
        let delivered = tokio::time::timeout(PATIENCE, self.inbound.next()).await.expect("the open did not reach the server");
        let frame = delivered.expect("the server's inbound stream ended").frame;
        assert!(matches!(frame, Frame::Open { .. }), "the server received {frame:?} where the open was due");
        let probe = probe(self);
        probe_until(probe, "the server's `opened` reaching the client", |probe| probe.withheld() == 1).await;
        (self.outbound.send)(pattern, Frame::Cancel { id }, None)
            .await
            .unwrap_or_else(|error| panic!("the cancel of call {id} was not sent: {error}"));
        assert_eq!(probe.calls(), Some(1), "the client's table once the call was cancelled before its `opened`");
    }
}

/// A streamed request cancelled before its `opened` reached the client, as a timeout or a drop
/// cancels it, keeps its `cancel`; the `opened` arriving 300 ms afterwards, within the
/// link's hold, sends it, and the server receives it for the call it holds. The client's table is then empty.
pub async fn late_opened<B: Broker>() {
    let mut linked = Linked::<B>::start().await;
    linked.cancel_with_opened_withheld(1).await;
    // Late by the short hold, and well inside the link's own.
    tokio::time::sleep(SHORT_HOLD).await;
    probe(&linked).release();
    let delivered = tokio::time::timeout(PATIENCE, linked.inbound.next()).await.unwrap_or_else(|_| {
        panic!("the held cancel did not reach the server within {PATIENCE:?} of the late `opened`")
    });
    let frame = delivered.expect("the server's inbound stream ended").frame;
    assert!(matches!(frame, Frame::Cancel { .. }), "the server received {frame:?} where the held cancel was due");
    assert_eq!(probe(&linked).calls(), Some(0), "the client's table once the held cancel went out");
    linked.stop().await;
}

/// A streamed request cancelled before its `opened` reached the client, whose `opened` never
/// comes, leaves no entry in the client's table once the link's hold runs out, shortened here to
/// 300 ms.
pub async fn hold_runs_out<B: Broker>() {
    let mut linked = Linked::<B>::start().await;
    probe(&linked).set_hold(SHORT_HOLD);
    let cancelled = tokio::time::Instant::now();
    linked.cancel_with_opened_withheld(1).await;
    probe_until(probe(&linked), "the held cancel's entry dropped", |probe| probe.calls() == Some(0)).await;
    assert!(cancelled.elapsed() >= SHORT_HOLD, "the held cancel was dropped before its hold ran out");
    linked.stop().await;
}
