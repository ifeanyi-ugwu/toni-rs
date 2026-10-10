//! The WebSocket conformance suite against `WsModule`'s hand-off inside a poem host, the app's
//! endpoint nested at `/`: each scenario on a multi-thread tokio runtime of its own, a client
//! reaching poem's listener over TCP. poem declares `upgrades`, and its adapter sets the upgrade on
//! the request it builds for the app.

use std::future::Future;
use std::io::{self, Result as IoResult};

use poem::Route;
use poem::http::uri::Scheme;
use poem::listener::{Acceptor, TcpAcceptor};
use poem::web::{LocalAddr, RemoteAddr};
use tokio::net::TcpStream;
use ulo::app::{Bound, Connected};
use ulo::{App, BoundAddr, Signal};
use ulo_http::Upgraded;
use ulo_http::embed::Embed;
use ulo_http_conformance::{Counted, ReadCount};
use ulo_http_poem::{Embedded, Handle, Poem, endpoint};
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, Serving, report, startup_failed, ws_conformance_suite};

struct PoemHost {
    embedded: Handle,
    read_count: ReadCount,
}

/// poem's acceptor, each connection it accepts counted at its first read.
struct Counting {
    tcp: TcpAcceptor,
    read_count: ReadCount,
}

impl Acceptor for Counting {
    type Io = Counted<<TcpAcceptor as Acceptor>::Io>;

    fn local_addr(&self) -> Vec<LocalAddr> {
        self.tcp.local_addr()
    }

    async fn accept(&mut self) -> IoResult<(Self::Io, LocalAddr, RemoteAddr, Scheme)> {
        let (stream, local, remote, scheme) = self.tcp.accept().await?;
        Ok((self.read_count.wrap(stream), local, remote, scheme))
    }
}

impl Host for PoemHost {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Http;
    // poem serves through hyper's graceful shutdown, which closes a connection idle between
    // requests.
    const CLOSES_IDLE_AT_DRAIN: bool = true;

    fn upgrades() -> bool {
        <Poem as Embed>::limits().upgrades
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
        (app.bind(server), PoemHost { embedded, read_count: ReadCount::default() })
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn serve(&self, app: App<Bound>) -> Serving {
        let route = Route::new().nest("/", endpoint(&self.embedded));
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        listener.set_nonblocking(true).unwrap_or_else(|error| startup_failed!("the listener did not turn non-blocking: {}", report(&error)));
        let acceptor = TcpAcceptor::from_std(listener).unwrap_or_else(|error| startup_failed!("poem did not adopt the listener: {}", report(&error)));
        let host = poem::Server::new_with_acceptor(Counting { tcp: acceptor, read_count: self.read_count.clone() });
        let embedded = self.embedded.clone();
        Serving {
            addresses: vec![BoundAddr::new("http", addr)],
            until_closed: Box::pin(async move {
                let _ = ulo_http_poem::run(app, &embedded, host, route, std::future::pending::<Signal>()).await;
            }),
        }
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(PoemHost);
