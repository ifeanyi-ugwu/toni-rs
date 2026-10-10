//! The RPC conformance suite over the Redis link, each scenario against a Redis server in a
//! container of its own, since Pub/Sub channels span every database of a server. The client
//! reaches it through a relay that `disrupt` cuts.
//!
//! Beside the suite, two tests drive the link through `Link` directly, for orderings no scenario
//! reaches on its own: a request read after the drain ended the inbound stream, and a `cancel`
//! reaching the server after its request, one publisher and one Pub/Sub connection carrying both.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::BoundAddr;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern};
use ulo_rpc_redis::Redis;
use ulo_transport::ErrorKind;

const PORT: u16 = 6379;

struct RedisServer {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for RedisServer {
    type Link = Redis;

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the Redis container", || async {
            let container = GenericImage::new("redis", "7-alpine")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the Redis container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the Redis port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        RedisServer { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Redis {
        Redis::url(format!("redis://{}", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Redis {
        Redis::url(format!("redis://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(RedisServer);

#[tokio::test(flavor = "multi_thread")]
async fn cancel_follows_its_request() {
    ulo_rpc_conformance::cases::order::cancel_follows_its_request::<RedisServer>().await;
}

/// How long the test waits for a frame either side of the link should see.
const PATIENCE: Duration = Duration::from_secs(5);

/// A request Redis delivered before it processed the drain's UNSUBSCRIBE reaches the server, or
/// is refused `unavailable` by the link: the drain's watcher ends the inbound stream once no call
/// is held, and the server lane can read the request only after that. A link that held the
/// request and handed it to an ended stream would leave the caller to its own `Timeout`.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_read_after_the_drain_ended_the_inbound_stream_is_refused() {
    const DRAINED: &str = "link.drained";
    let broker = RedisServer::start().await;
    let server = broker.link();
    let mut inbound = server.listen(&[Pattern::from(DRAINED)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect().await.expect("the client's link connects");
    let mut replies = outbound.replies;

    let request = Frame::Req { id: 1, pattern: DRAINED.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    (outbound.send)(Pattern::from(DRAINED), request, None).await.expect("the request is published");
    server.drain().await;

    let delivered = tokio::time::timeout(PATIENCE, inbound.next())
        .await
        .unwrap_or_else(|_| panic!("the inbound stream neither handed the request over nor ended within {PATIENCE:?}"));
    match delivered {
        Some(delivery) => {
            let Frame::Req { id, .. } = delivery.frame else {
                panic!("the server received something other than the request: {:?}", delivery.frame);
            };
            let reply = delivery.reply.expect("a request carries its reply path");
            reply.send(Frame::Res { id, data: Data::new(b"2".as_slice()) }).await.expect("the reply is published");
            let answer = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
            assert!(matches!(answer, Some(Frame::Res { id: 1, .. })), "the server's reply did not reach the caller, got: {answer:?}");
        }
        None => {
            let answer = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
            assert!(
                matches!(&answer, Some(Frame::Err { id: 1, error }) if error.kind == ErrorKind::Unavailable),
                "a request read after the inbound stream ended was not refused `unavailable`, got: {answer:?}",
            );
        }
    }
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}
