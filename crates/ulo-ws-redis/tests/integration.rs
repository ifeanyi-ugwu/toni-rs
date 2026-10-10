//! Broadcast across processes through a real Redis: two app instances, each its own process with
//! its own `WsModule` over the Redis adapter, a room with a member connected to each, and a room
//! broadcast from either instance reaching the member on the other. Redis runs in a container.
//!
//! The second instance is this test binary run again as a child process, through the ignored test
//! `peer_instance`, which the parent starts with the Redis URL in `ULO_WS_REDIS_PEER`. The child
//! prints the address its server bound and serves until its stdin closes.

#![cfg(feature = "integration")]

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddr};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, Dep, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc_conformance::relay::{reachable, unshadowed};
use ulo_rpc_conformance::{report, startup_failed};
use ulo_ws::{BroadcastError, ConnectCx, ConnectRefused, OnConnect, Payload, Rooms, WsModule};
use ulo_ws_redis::Redis;

/// The environment variable carrying the Redis URL to the child process.
const PEER: &str = "ULO_WS_REDIS_PEER";

/// The line the child prints its server's address on.
const ADDR_LINE: &str = "ulo-ws-redis peer listening on ";

/// The room every connection joins.
const ROOM: &str = "members";

/// How long any one wait lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long the parent waits for the child to start and for both subscriptions to carry a
/// broadcast: the child process starts its own runtime, app and Redis subscription.
const BOOT: Duration = Duration::from_secs(60);

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[injectable]
struct Members;

#[routes]
#[ulo_ws::gateway(path = "/members", port = own)]
impl Members {
    /// Answered once the connection's connect phase, its join included, has finished.
    #[ulo_ws::message("ready")]
    fn ready(&self) {}

    /// To every member of the room, on every instance.
    #[ulo_ws::message("shout")]
    async fn shout(&self, text: Payload<String>, rooms: Dep<Rooms>) -> Result<(), BroadcastError> {
        rooms.to_room(ROOM).emit("shouted", &text.0).await
    }
}

impl OnConnect for Members {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        cx.conn().join(ROOM).await;
        Ok(())
    }
}

struct Root {
    url: String,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Redis::url(&self.url)));
        m.controller::<Members>();
    }
}

/// One instance serving on an ephemeral port until stopped.
struct Instance {
    addr: SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Instance {
    async fn start(url: &str) -> Instance {
        let app: App<Serving> = App::builder(Root { url: url.to_owned() })
            .runtime(ulo_tokio::Tokio::current())
            .drain_timeout(Duration::from_secs(2))
            .wire()
            .unwrap_or_else(|error| panic!("the instance did not wire: {}", report(&error)))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the instance did not connect: {}", report(&error)))
            .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the instance did not listen: {}", report(&error)));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Instance { addr, handle, serving }
    }

    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

/// The child process, killed if the test ends without stopping it.
struct Peer {
    child: Child,
    addr: SocketAddr,
}

impl Peer {
    /// This binary run again as `peer_instance`, once it reports its server's address.
    async fn start(url: &str) -> Peer {
        let mut child = Command::new(std::env::current_exe().expect("the test binary's path"))
            .args(["peer_instance", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env(PEER, url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|error| panic!("the peer process did not start: {error}"));
        let stdout = child.stdout.take().expect("the peer's stdout is piped");
        let (found, address) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            let mut found = Some(found);
            for line in lines.by_ref().map_while(Result::ok) {
                // libtest writes `test peer_instance ... ` ahead of it on the same line.
                if let Some((_, addr)) = line.split_once(ADDR_LINE)
                    && let Some(found) = found.take()
                {
                    let _ = found.send(addr.trim().to_owned());
                }
            }
        });
        let peer_addr = match tokio::time::timeout(BOOT, address).await {
            Ok(Ok(addr)) => addr.parse().unwrap_or_else(|error| panic!("the peer printed `{addr}`, not an address: {error}")),
            Ok(Err(_)) => {
                let _ = child.kill();
                panic!("the peer process ended before it reported its address");
            }
            Err(_) => {
                let _ = child.kill();
                panic!("the peer process did not report its address within {BOOT:?}");
            }
        };
        Peer { child, addr: peer_addr }
    }

    /// Closes the child's stdin, on which it stops its app and exits, and waits for it.
    async fn stop(mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                assert!(status.success(), "the peer process exited with {status}");
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the peer process did not exit within {PATIENCE:?} of its stdin closing");
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start_redis() -> (ContainerAsync<GenericImage>, String) {
    const PORT: u16 = 6379;
    let (container, addrs) = unshadowed("the Redis container", || async {
        let container = GenericImage::new("redis", "7-alpine")
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .start()
            .await
            .unwrap_or_else(|error| startup_failed!("the Redis container did not start: {}", report(&error)));
        let port = container
            .get_host_port_ipv4(PORT)
            .await
            .unwrap_or_else(|error| startup_failed!("the Redis port is not mapped: {}", report(&error)));
        (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
    })
    .await;
    reachable(addrs[0], Duration::from_secs(10)).await;
    (container, format!("redis://{}", addrs[0]))
}

/// A member of the room on the instance at `addr`, its join done.
async fn member(addr: SocketAddr) -> Socket {
    let (mut socket, _) = tokio::time::timeout(PATIENCE, tokio_tungstenite::connect_async(format!("ws://{addr}/members")))
        .await
        .unwrap_or_else(|_| panic!("connecting to {addr} did not finish within {PATIENCE:?}"))
        .unwrap_or_else(|error| panic!("connecting to {addr} failed: {error}"));
    send(&mut socket, json!({ "event": "ready", "id": 0 })).await;
    assert_eq!(next(&mut socket).await, json!({ "id": 0, "complete": true }), "the member's `ready`");
    socket
}

async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::text(message.to_string())).await.unwrap_or_else(|error| panic!("sending {message} failed: {error}"));
}

/// The next text frame as JSON, within `PATIENCE`.
async fn next(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .unwrap_or_else(|_| panic!("no frame arrived within {PATIENCE:?}"))
            .unwrap_or_else(|| panic!("the connection ended"))
            .unwrap_or_else(|error| panic!("reading a frame failed: {error}"));
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap_or_else(|error| panic!("a frame is not JSON: {error}: {text}"));
        }
    }
}

/// The data of the next `shouted` broadcast `socket` receives within `within`, skipping the
/// answers to its own calls; `None` when none arrives.
async fn shouted(socket: &mut Socket, within: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next()).await.ok()??.ok()?;
        let Message::Text(text) = message else { continue };
        let value: Value = serde_json::from_str(&text).ok()?;
        if value["event"] == "shouted" {
            return value["data"].as_str().map(str::to_owned);
        }
    }
}

/// A room broadcast sent from either instance reaches the room's member on the other process.
///
/// A broadcast published before the other instance's subscription is in place is lost, as Redis
/// Pub/Sub delivers to the subscribers present. So the first direction shouts again every 200 ms
/// until the member on the other process receives one, within `BOOT`; once it has, both
/// subscriptions are in place, and the reverse direction is shown with a single broadcast.
#[tokio::test(flavor = "multi_thread")]
async fn a_room_broadcast_reaches_the_member_on_the_other_process() {
    let (_redis, url) = start_redis().await;
    let local = Instance::start(&url).await;
    let peer = Peer::start(&url).await;
    let mut here = member(local.addr).await;
    let mut there = member(peer.addr).await;

    let deadline = Instant::now() + BOOT;
    let mut sent = 0u32;
    let reached = loop {
        sent += 1;
        send(&mut here, json!({ "event": "shout", "data": format!("from here {sent}"), "id": sent })).await;
        if let Some(text) = shouted(&mut there, Duration::from_millis(200)).await {
            break text;
        }
        assert!(Instant::now() < deadline, "none of {sent} broadcasts from this process reached the member on the other within {BOOT:?}");
    };
    assert!(reached.starts_with("from here "), "the member on the other process received `{reached}`");

    send(&mut there, json!({ "event": "shout", "data": "from there", "id": 1 })).await;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let received = shouted(&mut here, deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .unwrap_or_else(|| panic!("the broadcast from the other process did not reach this process's member within {PATIENCE:?}"));
        // This process's member also receives its own retries; they come first.
        if received == "from there" {
            break;
        }
        assert!(received.starts_with("from here "), "this process's member received `{received}`");
    }

    let _ = here.close(None).await;
    let _ = there.close(None).await;
    peer.stop().await;
    local.stop().await;
}

/// The second process: an instance on the Redis URL in `ULO_WS_REDIS_PEER`, which prints its
/// address and serves until its stdin closes. Run by the test above; with the variable unset it
/// does nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "the second process of `a_room_broadcast_reaches_the_member_on_the_other_process`"]
async fn peer_instance() {
    let Ok(url) = std::env::var(PEER) else { return };
    let instance = Instance::start(&url).await;
    println!("{ADDR_LINE}{}", instance.addr);
    let closed = tokio::task::spawn_blocking(|| {
        let mut rest = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut rest);
    });
    let _ = tokio::time::timeout(Duration::from_secs(300), closed).await;
    instance.stop().await;
}
