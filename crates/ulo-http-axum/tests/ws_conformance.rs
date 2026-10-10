//! The WebSocket conformance suite against `WsModule`'s hand-off inside an axum host, the app the
//! router's `fallback_service`: each scenario on a multi-thread tokio runtime of its own, a client
//! reaching axum's listener over TCP. axum declares `upgrades`, so the suite's gateways on the HTTP
//! port are reached through the upgrade `Embed::take_upgrade` takes out of each request.

use std::future::Future;
use std::io;
use std::net::SocketAddr;

use axum::Router;
use axum::serve::{Listener, ListenerExt};
use tokio::net::{TcpListener, TcpStream};
use tower::Layer;
use ulo::app::{Bound, Connected};
use ulo::{App, BoundAddr, Signal};
use ulo_http::Upgraded;
use ulo_http::embed::Embed;
use ulo_http_axum::{Axum, Embedded, Handle, HostLayer};
use ulo_http_conformance::{Counted, ReadCount};
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, Serving, report, startup_failed, ws_conformance_suite};

struct AxumHost {
    embedded: Handle,
    read_count: ReadCount,
}

/// The host's listener, each connection it accepts counted at its first read.
struct Counting {
    tcp: TcpListener,
    read_count: ReadCount,
}

impl Listener for Counting {
    type Io = Counted<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, addr) = Listener::accept(&mut self.tcp).await;
        (self.read_count.wrap(stream), addr)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Listener::local_addr(&self.tcp)
    }
}

impl Host for AxumHost {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Http;
    // axum's graceful shutdown is hyper's, which closes a connection idle between requests.
    const CLOSES_IDLE_AT_DRAIN: bool = true;

    fn upgrades() -> bool {
        <Axum as Embed>::limits().upgrades
    }

    fn runtime() -> Tokio {
        Tokio::current()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| startup_failed!("a tokio runtime for the scenario: {error}"))
            .block_on(fut)
    }

    fn bind(app: App<Connected>) -> (App<Connected>, Self) {
        let server = Embedded::new();
        let embedded = server.handle();
        (app.bind(server), AxumHost { embedded, read_count: ReadCount::default() })
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn serve(&self, app: App<Bound>) -> Serving {
        let router = Router::new().fallback_service(HostLayer.layer(self.embedded.service()));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        // axum gives `ConnectInfo<SocketAddr>` for a `TcpListener` and for a listener under
        // `tap_io`, so the counting listener goes under a `tap_io` that does nothing.
        let listener = Counting { tcp: listener, read_count: self.read_count.clone() }.tap_io(|_| {});
        let serve = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>());
        let embedded = self.embedded.clone();
        Serving {
            addresses: vec![BoundAddr::new("http", addr)],
            until_closed: Box::pin(async move {
                let _ = ulo_http_axum::run(app, &embedded, serve, std::future::pending::<Signal>()).await;
            }),
        }
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(AxumHost);
