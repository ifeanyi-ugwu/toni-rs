//! A draining server that holds no call stops without waiting out its drain window, though a
//! client stays connected and could still send: the link ends its inbound stream once nothing is
//! held, and the core's drain then has nothing left to wait for.

use std::error::Error;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc::{Payload, RpcClient, RpcClientModule};
use ulo_rpc_udp::Udp;
use ulo_transport::{Classify, ErrorKind};

/// The server's drain window, far longer than the stop the test allows.
const DRAIN: Duration = Duration::from_secs(30);

/// How long the server's stop may take: a drain waiting for nothing ends in milliseconds.
const STOP: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[injectable]
struct Adder;

#[routes]
impl Adder {
    #[ulo_rpc::message("drain_end.add")]
    async fn add(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0 + 1)
    }
}

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Adder>();
    }
}

struct ClientRoot {
    link: Mutex<Option<Udp>>,
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.link.lock().unwrap().take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(Duration::from_secs(5))));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_holding_no_call_stops_without_waiting_out_its_drain() {
    let server = App::builder(ServerRoot)
        .timer(ulo_tokio::Timer)
        .drain_timeout(DRAIN)
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(Udp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = server.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = server.handle();
    let serving = tokio::spawn(async move { server.serve(std::future::pending::<Signal>()).await });

    let client = App::builder(ClientRoot { link: Mutex::new(Some(Udp::new(addr))) })
        .timer(ulo_tokio::Timer)
        .wire()
        .expect("the client wires")
        .connect()
        .await
        .expect("the client connects");
    let rpc = (*client.get::<RpcClient>().await.expect("the client holds an `RpcClient`")).clone();
    assert_eq!(rpc.request::<_, u32>("drain_end.add", &1u32).await.expect("the call is answered"), 2);

    let started = Instant::now();
    let shutdown = handle.close(Signal::new("drain end test")).await;
    let took = started.elapsed();
    assert!(shutdown.is_ok(), "the server's shutdown failed: {shutdown:?}");
    assert!(took < STOP, "the server's stop took {took:?} with no call held, against a drain window of {DRAIN:?}");
    let _ = serving.await;
    let _ = client.close(Signal::new("drain end test")).await;
}
