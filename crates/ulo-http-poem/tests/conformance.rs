//! The embedding conformance suite against a poem host, nested under `PREFIX` with `Route::nest`
//! and as the fallback, nested at `/`, which strips nothing.

use std::io::Result as IoResult;

use poem::http::uri::Scheme;
use poem::listener::{Acceptor, TcpAcceptor};
use poem::web::{LocalAddr, RemoteAddr};
use poem::{Endpoint, EndpointExt, IntoResponse, Route};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_conformance::{
    Counted, HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, ReadCount, report, routing_label, startup_failed,
};
use ulo_http_poem::{Embedded, Poem, endpoint};

struct PoemHost {
    base_url: String,
    read_count: ReadCount,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
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

/// The host's middleware, around the route: the header's value into the request's extensions,
/// and the app's `Routing` onto the response.
async fn host_middleware<E: Endpoint>(next: E, mut req: poem::Request) -> poem::Result<poem::Response> {
    let value = req.headers().get(HOST_VALUE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
    if let Some(value) = value {
        req.extensions_mut().insert(HostValue(value));
    }
    let mut response = next.call(req).await?.into_response();
    if let Some(label) = response.extensions().get::<Routing>().map(routing_label) {
        response.headers_mut().insert(ROUTING_HEADER, label.parse().expect("a label is a header value"));
    }
    Ok(response)
}

impl Host for PoemHost {
    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        };
        let embedded = server.handle();
        let app = app.bind(server).listen().await.unwrap_or_else(|error| startup_failed!("the app did not listen inside poem: {}", report(&error)));
        let route = match mode {
            Mode::Nested => Route::new().nest(PREFIX, endpoint(&embedded)),
            Mode::Fallback => Route::new().nest("/", endpoint(&embedded)),
        }
        .around(host_middleware);
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        listener.set_nonblocking(true).unwrap_or_else(|error| startup_failed!("the listener did not turn non-blocking: {}", report(&error)));
        let acceptor = TcpAcceptor::from_std(listener)
            .unwrap_or_else(|error| startup_failed!("poem did not adopt the listener: {}", report(&error)));
        let read_count = ReadCount::default();
        let host = poem::Server::new_with_acceptor(Counting { tcp: acceptor, read_count: read_count.clone() });
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_poem::run(app, &embedded, host, route, signal).await;
        });
        PoemHost { base_url: format!("http://{addr}"), read_count, stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Poem as Embed>::limits()
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(PoemHost);
