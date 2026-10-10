//! The WebSocket conformance suite against `WsModule`'s hand-off inside a salvo host, the app's
//! handler the catch-all `{**rest}`: each scenario on a multi-thread tokio runtime of its own, a
//! client reaching salvo's listener over TCP. salvo declares `upgrades`, and its adapter sets the
//! upgrade on the request it builds for the app.

use std::future::Future;
use std::io::{self, Result as IoResult};

use salvo::Router;
use salvo::conn::tcp::{TcpAcceptor, TcpCoupler};
use salvo::conn::{Accepted, Acceptor, Holding};
use salvo::fuse::ArcFuseFactory;
use tokio::net::TcpStream;
use ulo::app::{Bound, Connected};
use ulo::{App, BoundAddr, Signal};
use ulo_http::Upgraded;
use ulo_http::embed::Embed;
use ulo_http_conformance::{Counted, ReadCount};
use ulo_http_salvo::{Closing, Embedded, Handle, Salvo, handler};
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, Serving, report, startup_failed, ws_conformance_suite};

struct SalvoHost {
    embedded: Handle,
    read_count: ReadCount,
}

/// salvo's acceptor, each connection it accepts counted at its first read.
struct Counting {
    tcp: TcpAcceptor,
    read_count: ReadCount,
}

type Accepting = <TcpAcceptor as Acceptor>::Stream;

impl Acceptor for Counting {
    type Coupler = TcpCoupler<Counted<Accepting>>;
    type Stream = Counted<Accepting>;

    fn holdings(&self) -> &[Holding] {
        self.tcp.holdings()
    }

    async fn accept(&mut self, fuse_factory: Option<ArcFuseFactory>) -> IoResult<Accepted<Self::Coupler, Self::Stream>> {
        let Accepted { stream, fusewire, local_addr, remote_addr, http_scheme, .. } = self.tcp.accept(fuse_factory).await?;
        let stream = self.read_count.wrap(stream);
        Ok(Accepted { coupler: TcpCoupler::new(), stream, fusewire, local_addr, remote_addr, http_scheme })
    }
}

impl Host for SalvoHost {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Http;
    // salvo serves through hyper's graceful shutdown, which closes a connection idle between
    // requests.
    const CLOSES_IDLE_AT_DRAIN: bool = true;

    fn upgrades() -> bool {
        <Salvo as Embed>::limits().upgrades
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
        (app.bind(server), SalvoHost { embedded, read_count: ReadCount::default() })
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn serve(&self, app: App<Bound>) -> Serving {
        let router = Router::new().push(Router::with_path("{**rest}").goal(handler(&self.embedded)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        let acceptor = TcpAcceptor::try_from(listener).unwrap_or_else(|error| startup_failed!("salvo did not adopt the listener: {}", report(&error)));
        let acceptor = Counting { tcp: acceptor, read_count: self.read_count.clone() };
        let server = salvo::Server::new(Closing::new(&self.embedded, acceptor));
        let service = salvo::Service::new(router);
        let embedded = self.embedded.clone();
        Serving {
            addresses: vec![BoundAddr::new("http", addr)],
            until_closed: Box::pin(async move {
                let _ = ulo_http_salvo::run(app, &embedded, server, service, std::future::pending::<Signal>()).await;
            }),
        }
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(SalvoHost);
