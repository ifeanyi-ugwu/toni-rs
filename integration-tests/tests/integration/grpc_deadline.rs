//! `ctx.deadline()` reads the caller's `grpc-timeout`.
//!
//! gRPC is the one transport whose wire carries how long the caller intends to
//! wait, and until now nothing read it — `deadline()` answered `None` on every
//! context. A guard can now refuse work it cannot finish in time, and a handler
//! can budget against the caller's patience rather than its own guess.

#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::common::NotServed;
use futures_util::Stream;
use serial_test::serial;
use toni::context::{GrpcContext, HandlerContext};
use toni::extractors::{Inbound, Payload};
use toni::ToniFactory;
use toni_macros::{controller, grpc_methods, module, new};

mod deadline_pb {
    tonic::include_proto!("toni_test.orders");
}

use deadline_pb::orders_client::OrdersClient;
use deadline_pb::orders_server::{Orders, OrdersServer};

/// What the handler saw, as time left rather than an instant, since the test
/// asserts against the budget the caller sent.
static REMAINING: Mutex<Option<Option<Duration>>> = Mutex::new(None);

#[controller]
pub struct DeadlineService {}

impl DeadlineService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(deadline_pb::orders_server::Orders)]
impl DeadlineService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<deadline_pb::CreateOrderRequest>,
        ctx: &GrpcContext,
    ) -> Result<deadline_pb::CreateOrderResponse, NotServed> {
        *REMAINING.lock().unwrap() = Some(
            ctx.deadline()
                .map(|d| d.saturating_duration_since(Instant::now())),
        );
        Ok(deadline_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<deadline_pb::WatchRequest>,
    ) -> Result<
        impl Stream<Item = Result<deadline_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<deadline_pb::CreateOrderRequest>,
    ) -> Result<deadline_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<deadline_pb::ChatMessage>,
    ) -> Result<
        impl Stream<Item = Result<deadline_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [DeadlineService])]
impl DeadlineModule {}

async fn boot() -> (u16, toni::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = toni_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(DeadlineModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("grpc must bind").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    )
}

fn order() -> deadline_pb::CreateOrderRequest {
    deadline_pb::CreateOrderRequest {
        item: "keyboard".to_string(),
        qty: 1,
    }
}

/// The handler sees the budget the caller sent, not one of its own.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_handler_reads_the_callers_timeout() {
    *REMAINING.lock().unwrap() = None;

    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(order());
    request.set_timeout(Duration::from_secs(5));
    client.create(request).await.expect("call must succeed");

    let remaining = REMAINING
        .lock()
        .unwrap()
        .expect("the handler ran")
        .expect("a caller-set timeout is a deadline");
    assert!(
        remaining > Duration::from_secs(3) && remaining <= Duration::from_secs(5),
        "the deadline must sit within the budget sent: {remaining:?}"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

/// A caller that sends none has none, rather than one the server invented.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_call_without_a_timeout_has_no_deadline() {
    *REMAINING.lock().unwrap() = None;

    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    client.create(order()).await.expect("call must succeed");

    assert_eq!(
        REMAINING.lock().unwrap().expect("the handler ran"),
        None,
        "nothing on the wire named a deadline"
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}
