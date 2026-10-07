//! The embedding conformance suite against an axum host, nested under `PREFIX` with
//! `nest_service` and as the router's `fallback_service`.

use std::net::SocketAddr;

use axum::Router;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use http::HeaderValue;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower::Layer;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_axum::{Axum, Embedded, HostLayer};
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, report, startup_failed, routing_label};

struct AxumHost {
    base_url: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
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
        let serve = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>());
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_axum::run(app, &embedded, serve, signal).await;
        });
        AxumHost { base_url: format!("http://{addr}"), stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Axum as Embed>::limits()
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(AxumHost);
