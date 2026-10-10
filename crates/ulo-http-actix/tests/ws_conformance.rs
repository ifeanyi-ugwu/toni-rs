//! The WebSocket conformance suite against an actix-web host, which declares `upgrades: false`: the
//! suite's app, serving gateways on the HTTP port, must be refused at `listen()` before the host
//! serves anything, rather than advertising gateways whose upgrades never arrive. No other scenario
//! applies.

use std::future::Future;
use std::io;

use tokio::net::TcpStream;
use ulo::app::Connected;
use ulo::{App, BoundAddr};
use ulo_http::Upgraded;
use ulo_http::embed::Embed;
use ulo_http_actix::{Actix, Embedded};
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, startup_failed, ws_conformance_suite};

struct ActixHost;

impl Host for ActixHost {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Http;

    fn upgrades() -> bool {
        <Actix as Embed>::limits().upgrades
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
        (app.bind(Embedded::new()), ActixHost)
    }

    fn connections_read(&self) -> Option<usize> {
        None
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(ActixHost; without_upgrades);
