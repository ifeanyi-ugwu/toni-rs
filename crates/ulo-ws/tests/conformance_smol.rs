//! The WebSocket conformance suite against the upgrade hand-off on the HTTP server's port, served
//! by `ulo-http-hyper` on smol: the server on `ulo-listen-smol`'s listener, the app on
//! `ulo_smol::Smol`, a client reaching the HTTP server over `async-net`. No tokio runtime runs.

use std::future::Future;
use std::io;

use async_net::TcpStream;
use ulo::app::Connected;
use ulo::{App, BoundAddr};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_http_hyper::{HyperOn, ReadCount, ServerOn};
use ulo_listen_smol::SmolListener;
use ulo_smol::Smol;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, ws_conformance_suite};

struct HttpPortOnSmol {
    read_count: ReadCount,
}

impl Host for HttpPortOnSmol {
    type Runtime = Smol;
    type Stream = TcpStream;

    const PORT: Port = Port::Http;
    // `ulo-http-hyper` starts hyper's graceful shutdown on every connection at the drain, which closes one idle between requests.
    const CLOSES_IDLE_AT_DRAIN: bool = true;

    fn upgrades() -> bool {
        true
    }

    fn runtime() -> Smol {
        OnSmol::runtime()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        OnSmol::block_on(fut)
    }

    fn bind(app: App<Connected>) -> (App<Connected>, Self) {
        let backend = HyperOn::<SmolListener>::default();
        let read_count = backend.read_count();
        (app.bind(ServerOn::<SmolListener>::with_backend("127.0.0.1:0", backend)), HttpPortOnSmol { read_count })
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<TcpStream> {
        TcpStream::connect(addresses[0].addr).await
    }
}

ws_conformance_suite!(HttpPortOnSmol);
