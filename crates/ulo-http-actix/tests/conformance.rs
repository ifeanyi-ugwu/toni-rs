//! The embedding conformance suite against an actix-web host, as a scope at `PREFIX` and as the
//! default service. actix's request store does not reach the app (`host_extensions: false`), so
//! the host's middleware puts `HostValue` there and an `Embedded::forward` copy carries it across.
//! The adapter builds actix-web without its `http2` feature, so the host serves HTTP/1.1 alone and
//! the HTTP/2 drain scenarios are declared not applicable.

use actix_web::HttpMessage;
use actix_web::dev::Service;
use actix_web::http::header::{HeaderName, HeaderValue};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_actix::{Actix, Embedded, scope};
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, routing_label};

struct ActixHost {
    base_url: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl Host for ActixHost {
    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        }
        .forward(|req: &actix_web::HttpRequest| req.extensions().get::<HostValue>().cloned());
        let embedded = server.handle();
        let app = app.bind(server).listen().await.expect("the app listens inside actix-web");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let addr = listener.local_addr().expect("the listener's address");
        let host = actix_web::HttpServer::new({
            let embedded = embedded.clone();
            move || {
                let host = actix_web::App::new().wrap_fn(|req, service| {
                    let value = req.headers().get(HOST_VALUE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
                    if let Some(value) = value {
                        req.extensions_mut().insert(HostValue(value));
                    }
                    let answer = service.call(req);
                    async move {
                        let mut response = answer.await?;
                        let label = response.response().extensions().get::<Routing>().map(routing_label);
                        if let Some(label) = label {
                            response.headers_mut().insert(
                                HeaderName::from_static(ROUTING_HEADER),
                                HeaderValue::from_str(&label).expect("a label is a header value"),
                            );
                        }
                        Ok(response)
                    }
                });
                match mode {
                    Mode::Nested => host.service(scope(PREFIX, &embedded)),
                    Mode::Fallback => host.default_service(scope("", &embedded)),
                }
            }
        })
        .workers(1)
        .listen(listener)
        .expect("actix-web adopts the listener");
        let (stop, stopped) = oneshot::channel::<()>();
        let signal = async move {
            let _ = stopped.await;
            Signal::new("suite")
        };
        let running = ulo_http_actix::run(app, &embedded, host, signal);
        let serving = tokio::spawn(async move {
            let _ = running.await;
        });
        ActixHost { base_url: format!("http://{addr}"), stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Actix as Embed>::limits()
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(ActixHost; not_applicable {
    drain_http2: "the adapter builds actix-web without `http2`, so the host serves HTTP/1.1 alone",
    drain_goaway: "the adapter builds actix-web without `http2`, so the host serves HTTP/1.1 alone",
});
