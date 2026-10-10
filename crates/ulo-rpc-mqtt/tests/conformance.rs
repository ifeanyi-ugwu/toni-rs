//! The RPC conformance suite over the MQTT v5 link, each scenario against a Mosquitto broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts. Mosquitto
//! 2.x supports MQTT v5 shared subscriptions; the image's `/mosquitto-no-auth.conf` listens on
//! every interface and allows anonymous clients, which the default configuration does not.
//!
//! Beside the suite, five tests drive the link through `Link` directly, for orderings inside it
//! that no scenario reaches on its own and for the flow-control window it announces.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, ThreadId};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{FutureExt, StreamExt};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::BoundAddr;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::link::Inbound;
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern};
use ulo_rpc_mqtt::Mqtt;
use ulo_transport::{Detail, ErrorKind};

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

ulo_rpc_conformance::conformance_suite!(Mosquitto; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});

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

/// A server's drain returns only once the broker has stopped routing to it: a request published
/// after it returns finds no subscriber, and the broker's PUBACK 0x10 reaches the caller as
/// `unavailable` with `reason: "no_destination"`. A drain returning while its UNSUBSCRIBE waited
/// to be written or processed would leave the request routed to the draining server.
#[tokio::test(flavor = "multi_thread")]
async fn drain_returns_once_the_broker_stops_routing_to_the_server() {
    const DRAINED: &str = "link.drained";
    let broker = Mosquitto::start().await;
    let server = broker.link().group("drain_confirmed");
    let mut inbound = server.listen(&[Pattern::from(DRAINED)]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect().await.expect("the client's link connects");
    let mut replies = outbound.replies;

    tokio::time::timeout(PATIENCE, server.drain()).await.expect("the drain returns once the broker has confirmed it");
    let request = Frame::Req { id: 1, pattern: DRAINED.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
    (outbound.send)(Pattern::from(DRAINED), request, None).await.expect("the request is published");

    let reply = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
    let no_destination = |error: &ulo_rpc::ErrorBody| {
        error.kind == ErrorKind::Unavailable
            && error.details.iter().any(|detail| matches!(detail, Detail::ErrorInfo { reason, .. } if reason == "no_destination"))
    };
    assert!(
        matches!(&reply, Some(Frame::Err { id: 1, error }) if no_destination(error)),
        "a request published once the drain returned was not answered `no_destination`, got: {reply:?}",
    );
    assert!(inbound.next().now_or_never().is_none_or(|delivery| delivery.is_none()), "the drained server received the request");
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// A drain dropped before the broker confirmed its UNSUBSCRIBEs, as the core drops it at the
/// drain's deadline, logs the filters the broker had not confirmed. On a current-thread runtime
/// the event loop cannot run between the drain's first poll and its drop, so nothing is confirmed.
#[tokio::test]
async fn a_drain_dropped_before_the_broker_confirms_logs_the_filters() {
    const UNCONFIRMED: &str = "link.unconfirmed";
    capture();
    let broker = Mosquitto::start().await;
    let server = broker.link().group("drain_unconfirmed");
    let _inbound = server.listen(&[Pattern::from(UNCONFIRMED)]).await.expect("the server's link listens");

    assert!(server.drain().now_or_never().is_none(), "the drain returned before the event loop could write its UNSUBSCRIBE");

    let warnings = capture().warnings(thread::current().id());
    let filter = format!("$share/drain_unconfirmed/{UNCONFIRMED}");
    assert!(
        warnings.iter().any(|line| {
            line.starts_with("the drain's deadline passed before the MQTT broker confirmed the unsubscribe") && line.contains(&filter)
        }),
        "no `warn` naming the unconfirmed filter `{filter}`: {warnings:?}",
    );
    server.close().await.expect("the server's link closes");
}

/// The broker puts every request routed to a server instance in flight at once, holding none back
/// for flow control: the link announces a Receive Maximum of 65,535 in its CONNECT, and Mosquitto
/// otherwise applies its own `max_inflight_messages`, 20 unset. The server reaches the broker
/// through a relay that, once armed, forwards nothing the server writes, so no PUBACK reaches the
/// broker and every request it sends stays unacknowledged.
#[tokio::test(flavor = "multi_thread")]
async fn the_broker_holds_back_no_request_for_flow_control() {
    const FLOODED: &str = "link.flooded";
    const SENT: usize = 50;
    let broker = Mosquitto::start().await;
    let relay = Muting::start(broker.server).await;
    let server = Mqtt::url(format!("mqtt://{}", relay.addr)).group("flow_control");
    let mut inbound = server.listen(&[Pattern::from(FLOODED)]).await.expect("the server's link listens");
    relay.mute();
    let client = broker.link();
    let outbound = client.connect().await.expect("the client's link connects");

    for id in 1..=SENT as u64 {
        let request = Frame::Req { id, pattern: FLOODED.to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"null")) };
        (outbound.send)(Pattern::from(FLOODED), request, None).await.expect("the request is published");
    }
    let mut received = 0;
    while received < SENT {
        match next_frame(&mut inbound).await {
            Some(Frame::Req { .. }) => received += 1,
            other => panic!("{received} of {SENT} requests reached the server unacknowledged; then: {other:?}"),
        }
    }
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// A TCP relay to `target` for one connection that, once muted, forwards what the target sends
/// and nothing the other side writes.
struct Muting {
    addr: SocketAddr,
    muted: Arc<std::sync::atomic::AtomicBool>,
}

impl Muting {
    async fn start(target: SocketAddr) -> Muting {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.expect("a loopback port");
        let addr = listener.local_addr().expect("the relay's address");
        let muted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let outward = Arc::clone(&muted);
        tokio::spawn(async move {
            let Ok((inner, _)) = listener.accept().await else { return };
            let Ok(outer) = tokio::net::TcpStream::connect(target).await else { return };
            let (mut inner_read, mut inner_write) = inner.into_split();
            let (mut outer_read, mut outer_write) = outer.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut outer_read, &mut inner_write).await;
            });
            let mut buffer = vec![0u8; 16 * 1024];
            while let Ok(read) = inner_read.read(&mut buffer).await {
                if read == 0 {
                    return;
                }
                if !outward.load(Ordering::Acquire) && outer_write.write_all(&buffer[..read]).await.is_err() {
                    return;
                }
            }
        });
        Muting { addr, muted }
    }

    fn mute(&self) {
        self.muted.store(true, Ordering::Release);
    }
}

/// The process's subscriber, installed by the first test that reads it. A thread-local one would
/// not do: tracing caches whether a callsite is enabled from the first thread to reach it, and a
/// test without the subscriber reaching the `warn` first would disable it for the one with it.
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
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().filter(|(thread, _)| *thread == on).map(|(_, line)| line.clone()).collect()
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
        let line = format!("{}{}", line.message, line.fields);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push((thread::current().id(), line));
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
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}
