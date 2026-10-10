//! The WebSocket conformance suite against the upgrade hand-off on the HTTP server's port, served by
//! `ulo-http-hyper`: each scenario on a multi-thread tokio runtime of its own, a client reaching the
//! HTTP server over TCP.

use std::future::Future;
use std::io;

use tokio::net::TcpStream;
use ulo::app::Connected;
use ulo::{App, BoundAddr};
use ulo_http::Upgraded;
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, ws_conformance_suite};

struct HttpPort;

impl Host for HttpPort {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Http;

    fn runtime() -> Tokio {
        Tokio::current()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| ulo_ws_conformance::startup_failed!("a tokio runtime for the scenario: {error}"))
            .block_on(fut)
    }

    fn bind(app: App<Connected>) -> App<Connected> {
        app.bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(HttpPort);
