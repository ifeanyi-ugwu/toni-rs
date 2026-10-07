//! The RPC conformance suite over the MQTT v5 link, each scenario against a Mosquitto broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts. Mosquitto
//! 2.x supports MQTT v5 shared subscriptions; the image's `/mosquitto-no-auth.conf` listens on
//! every interface and allows anonymous clients, which the default configuration does not.
//!
//! Beside the suite, two tests drive the link through `Link` directly, for orderings inside it
//! that no scenario reaches on its own.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{FutureExt, StreamExt};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo::BoundAddr;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::link::Inbound;
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern};
use ulo_rpc_mqtt::Mqtt;

const PORT: u16 = 1883;

struct Mosquitto {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for Mosquitto {
    type Link = Mqtt;

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the Mosquitto container", || async {
            let container = GenericImage::new("eclipse-mosquitto", "2.0.18")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stderr(" running"))
                .with_cmd(["mosquitto", "-c", "/mosquitto-no-auth.conf"])
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the Mosquitto container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the MQTT port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        Mosquitto { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Mqtt {
        Mqtt::url(format!("mqtt://{}", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Mqtt {
        Mqtt::url(format!("mqtt://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(Mosquitto);

/// How long a test waits for a frame the server's link should deliver.
const PATIENCE: Duration = Duration::from_secs(5);

async fn next_frame(inbound: &mut Inbound) -> Option<Frame> {
    tokio::time::timeout(PATIENCE, inbound.next()).await.ok().flatten().map(|delivery| delivery.frame)
}

/// The client side's `close` writes what was queued before it. rumqttc's `publish` only queues a
/// request for the event loop, and on a current-thread runtime nothing between the queueing and
/// `close` lets the loop run, so a `close` that ended the loop without awaiting it would lose the
/// event every time.
#[tokio::test]
async fn client_close_writes_what_was_queued_before_it() {
    const EVENT: &str = "link.event";
    let broker = Mosquitto::start().await;
    let server = broker.link().group("client_close");
    let mut inbound = server.listen(&[Pattern::from(EVENT)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect().await.expect("the client's link connects");

    let event = Frame::Evt { pattern: EVENT.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"7")) };
    let mut send = (outbound.send)(Pattern::from(EVENT), event, None);
    // One poll queues the publish; the rest of the send waits for the event loop to write it.
    assert!((&mut send).now_or_never().is_none(), "the event loop wrote the publish before it could run");
    client.close().await.expect("the client's link closes");
    drop(send);

    match next_frame(&mut inbound).await {
        Some(Frame::Evt { pattern, data, .. }) => {
            assert_eq!(pattern, EVENT);
            assert_eq!(data.as_bytes(), b"7");
        }
        other => panic!("the event queued before the client's close did not reach the server, got: {other:?}"),
    }
    server.close().await.expect("the server's link closes");
}

/// A draining server delivers the `cancel` that releases the last call it holds before its
/// inbound stream ends. The link releases a call on its `cancel` before delivering the frame, and
/// the drain ends the inbound stream once no call is held, so the two have to be ordered.
#[tokio::test(flavor = "multi_thread")]
async fn drain_delivers_the_cancel_that_releases_the_last_call() {
    const HELD: &str = "link.held";
    let broker = Mosquitto::start().await;
    let server = broker.link().group("drain_cancel");
    let mut inbound = server.listen(&[Pattern::from(HELD)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect().await.expect("the client's link connects");

    let request = Frame::Req { id: 1, pattern: HELD.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(HELD), request, None).await.expect("the request is published");
    let held = match next_frame(&mut inbound).await {
        Some(Frame::Req { id, .. }) => id,
        other => panic!("the request did not reach the server, got: {other:?}"),
    };

    server.drain().await;
    (outbound.send)(Pattern::from(HELD), Frame::Cancel { id: 1 }, None).await.expect("the cancel is published");
    assert_eq!(
        next_frame(&mut inbound).await,
        Some(Frame::Cancel { id: held }),
        "the cancel of the call the draining server held was not delivered",
    );
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}
