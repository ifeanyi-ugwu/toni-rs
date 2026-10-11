//! The embedding conformance suite against an axum host, nested under `PREFIX` with
//! `nest_service` and as the router's `fallback_service`.

use std::io;
use std::net::SocketAddr;

use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::serve::{Listener, ListenerExt};
use axum::response::Response;
use http::HeaderValue;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower::Layer;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_axum::{Axum, Embedded, HostLayer};
use ulo_http_conformance::{
    Counted, HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, ReadCount, report, routing_label, startup_failed,
};

struct AxumHost {
    base_url: String,
    read_count: ReadCount,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
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

/// The host's middleware, around everything the router serves: the header's value into the
/// request's extensions, and the app's `Routing` onto the response.
async fn host_middleware(mut req: Request, next: Next) -> Response {
    let value = req.headers().get(HOST_VALUE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
    if let Some(value) = value {
        req.extensions_mut().insert(HostValue(value));
    }
    let mut response = next.run(req).await;
    if let Some(label) = response.extensions().get::<Routing>().map(routing_label) {
        response.headers_mut().insert(ROUTING_HEADER, HeaderValue::from_str(&label).expect("a label is a header value"));
    }
    response
}

impl Host for AxumHost {
    type Harness = ulo_http_conformance::OnTokio;

    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        }
        .peer_addr(true);
        let embedded = server.handle();
        let app = app.bind(server).listen().await.unwrap_or_else(|error| startup_failed!("the app did not listen inside axum: {}", report(&error)));
        let service = HostLayer.layer(embedded.service());
        let router = match mode {
            Mode::Nested => Router::new().nest_service(PREFIX, service),
            Mode::Fallback => Router::new().fallback_service(service),
        }
        .layer(middleware::from_fn(host_middleware));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        let read_count = ReadCount::default();
        // axum gives `ConnectInfo<SocketAddr>` for a `TcpListener` and for a listener under
        // `tap_io`, so the counting listener goes under a `tap_io` that does nothing.
        let listener = Counting { tcp: listener, read_count: read_count.clone() }.tap_io(|_| {});
        let serve = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>());
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_axum::run(app, &embedded, serve, signal).await;
        });
        AxumHost { base_url: format!("http://{addr}"), read_count, stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Axum as Embed>::limits()
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(AxumHost);
