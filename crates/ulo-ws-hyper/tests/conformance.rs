//! The WebSocket conformance suite against the standalone server, the reference: each scenario on
//! a multi-thread tokio runtime of its own, a client reaching the server over TCP.

use std::future::Future;
use std::io;

use tokio::net::TcpStream;
use ulo::app::Connected;
use ulo::{App, BoundAddr};
use ulo_http::Upgraded;
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, ws_conformance_suite};

struct Standalone;

impl Host for Standalone {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Own;

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
        app.bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(Standalone);
