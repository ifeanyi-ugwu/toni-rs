//! Scenarios on the link itself, below the server and the client: a `cancel` sent after its
//! request reaches the server after it, on a link declaring `ordered_control`; and a streamed
//! request cancelled before the server acknowledged its `open` reaches the server as the `open`
//! and then the `cancel`, so the server holds nothing for it.
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
