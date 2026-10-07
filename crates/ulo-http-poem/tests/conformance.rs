//! The embedding conformance suite against a poem host, nested under `PREFIX` with `Route::nest`
//! and as the fallback, nested at `/`, which strips nothing.

use poem::listener::TcpAcceptor;
use poem::{Endpoint, EndpointExt, IntoResponse, Route};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, report, routing_label};
use ulo_http_poem::{Embedded, Poem, endpoint};

struct PoemHost {
    base_url: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
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
        let app = app.bind(server).listen().await.unwrap_or_else(|error| panic!("the app did not listen inside poem: {}", report(&error)));
        let route = match mode {
            Mode::Nested => Route::new().nest(PREFIX, endpoint(&embedded)),
            Mode::Fallback => Route::new().nest("/", endpoint(&embedded)),
        }
        .around(host_middleware);
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| panic!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| panic!("the host's listener has no address: {}", report(&error)));
        listener.set_nonblocking(true).unwrap_or_else(|error| panic!("the listener did not turn non-blocking: {}", report(&error)));
        let acceptor = TcpAcceptor::from_std(listener)
            .unwrap_or_else(|error| panic!("poem did not adopt the listener: {}", report(&error)));
        let host = poem::Server::new_with_acceptor(acceptor);
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_poem::run(app, &embedded, host, route, signal).await;
        });
        PoemHost { base_url: format!("http://{addr}"), stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Poem as Embed>::limits()
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(PoemHost);
