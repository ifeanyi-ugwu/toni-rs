//! The payoff of [`BindTarget::Listener`]: a socket outliving the process that
//! serves on it.
//!
//! A supervisor keeps the listening socket and hands a descriptor to each
//! application generation it starts. Because the socket is never closed, the
//! kernel keeps completing handshakes while nothing is accepting — a client that
//! connects between two generations waits instead of being refused, and the next
//! generation answers it. That is what turns a rebuild into latency rather than a
//! wall of connection errors.

use std::net::TcpListener;

use serial_test::serial;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use toni::toni_factory::ToniFactory;
use toni::{controller, get, module, routes, Body as ToniBody};
use toni_axum::AxumAdapter;

#[controller("/generation")]
pub struct GenerationController {}

#[routes]
impl GenerationController {
    #[get("/who")]
    fn who(&self) -> ToniBody {
        ToniBody::text("served")
    }
}

#[module(controllers: [GenerationController])]
struct HandoffModule;

/// Start one generation on a descriptor for `listener`, returning its bound
/// address and a handle that stops it.
async fn start_generation(listener: TcpListener) -> (std::net::SocketAddr, toni::ShutdownHandle) {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<toni::ShutdownHandle>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = ToniFactory::create(HandoffModule).await;
        app.use_http_adapter(AxumAdapter::new(), listener).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(bound.http.expect("HTTP adapter not bound"));
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });

    (addr_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_connection_made_between_generations_is_answered() {
    // The supervisor's socket, held open across both generations.
    let supervisor = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = supervisor.local_addr().unwrap();

    let (first_addr, first) = start_generation(supervisor.try_clone().unwrap()).await;
    assert_eq!(first_addr, addr);
    first.shutdown();
    first.completed().await;

    // Nothing is accepting now. The handshake still completes and the request
    // sits in the socket's queue, because the supervisor's socket is listening.
    let mut queued = tokio::net::TcpStream::connect(addr).await.unwrap();
    queued
        .write_all(b"GET /generation/who HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let (second_addr, second) = start_generation(supervisor.try_clone().unwrap()).await;
    assert_eq!(second_addr, addr, "the port survived the generation change");

    let mut response = String::new();
    queued.read_to_string(&mut response).await.unwrap();
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "a connection opened while nothing was accepting went unanswered: {response}"
    );
    assert!(
        response.ends_with("served"),
        "expected the second generation to answer: {response}"
    );

    second.shutdown();
    second.completed().await;
}
