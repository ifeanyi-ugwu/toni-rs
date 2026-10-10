//! A check on the link itself, below the server and the client: a `cancel` sent after its request
//! reaches the server after it. Not stamped by [`conformance_suite!`](crate::conformance_suite):
//! a link crate whose request and `cancel` travel one lane end to end, TCP's connection or Redis's
//! publisher and Pub/Sub connection, calls it from a test of its own. On a broker whose `cancel`
//! travels a control lane of its own, the broker may deliver it ahead of its request, so there
//! the order is not observable.

use std::collections::HashSet;
use std::time::Duration;

use futures_util::StreamExt;
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern, ReplyTo};

use crate::{Broker, report};

/// The pattern the requests name; the server's link listens on it and nothing handles it.
const ORDERED: &str = "conformance.ordered";

/// Request and `cancel` pairs sent, each pair's two sends started together.
const ROUNDS: u64 = 200;

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

/// Sends `ROUNDS` requests, each followed by its `cancel`, the two sends of a round polled together
/// and the request's first, and requires the server's link to deliver every request and then its
/// `cancel`. A `cancel` reaching the server first names no call it holds and is dropped, so the
/// round shows as a request with no `cancel` after it.
pub async fn cancel_follows_its_request<B: Broker>() {
    let broker = B::start().await;
    let app = App::builder(Empty)
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| crate::startup_failed!("the ordering check's app did not wire: {}", report(&error)))
        .connect()
        .await
        .unwrap_or_else(|error| crate::startup_failed!("the ordering check's app did not connect: {}", report(&error)));
    let mut server = broker.link();
    server.prepare(&app.handle()).await.unwrap_or_else(|error| crate::startup_failed!("the server's link did not prepare: {error}"));
    let mut inbound =
        server.listen(&[Pattern::from(ORDERED)]).await.unwrap_or_else(|error| crate::startup_failed!("the server's link did not listen: {error}"));
    let client = broker.client_link(&server.bound());
    let outbound = client.connect().await.unwrap_or_else(|error| crate::startup_failed!("the client's link did not connect: {error}"));

    let pattern = Pattern::from(ORDERED);
    for id in 1..=ROUNDS {
        let request = Frame::Req { id, pattern: ORDERED.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
        let request = (outbound.send)(pattern.clone(), request, Some(ReplyTo { id }));
        let cancel = (outbound.send)(pattern.clone(), Frame::Cancel { id }, None);
        let (requested, cancelled) = futures_util::join!(request, cancel);
        requested.unwrap_or_else(|error| panic!("request {id} was not sent: {error}"));
        cancelled.unwrap_or_else(|error| panic!("the cancel of request {id} was not sent: {error}"));
    }

    let (mut requests, mut cancels) = (0u64, 0u64);
    let mut held = HashSet::new();
    while requests + cancels < 2 * ROUNDS {
        let Ok(delivered) = tokio::time::timeout(PATIENCE, inbound.next()).await else {
            panic!(
                "{requests} requests and {cancels} cancels of {ROUNDS} each reached the server within {PATIENCE:?} of the last; \
                 a cancel that overtook its request is dropped there"
            );
        };
        let delivery = delivered.expect("the server's inbound stream ended");
        match delivery.frame {
            Frame::Req { id, .. } => {
                held.insert(id);
                requests += 1;
            }
            Frame::Cancel { id } => {
                assert!(held.remove(&id), "the server received a cancel for a call it does not hold: {id}");
                cancels += 1;
            }
            other => panic!("the server received a frame other than a request or a cancel: {other:?}"),
        }
    }
    assert!(held.is_empty(), "requests with no cancel after them: {held:?}");

    let _ = client.close().await;
    let _ = server.close().await;
    let _ = app.close(Signal::new("conformance")).await;
}
