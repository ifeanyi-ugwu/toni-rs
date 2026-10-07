//! The actix-web host both conformance targets run. actix's request store does not reach the app
//! (`host_extensions: false`), so the host's middleware puts `HostValue` there and an
//! `Embedded::forward` copy carries it across.

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

/// The actix-web host. `H2C` listens with `listen_auto_h2c`, which serves HTTP/2 without TLS
/// beside HTTP/1.1 and exists only where actix-web's `http2` feature is on; otherwise `listen`,
/// which serves HTTP/1.1 alone.
pub struct ActixHost<const H2C: bool> {
    base_url: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl<const H2C: bool> Host for ActixHost<H2C> {
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
        .workers(1);
        #[cfg(feature = "conformance-http2")]
        let host = if H2C { host.listen_auto_h2c(listener) } else { host.listen(listener) };
        #[cfg(not(feature = "conformance-http2"))]
        let host = {
            const { assert!(!H2C, "an h2c host needs actix-web's `http2`: `--features conformance-http2`") };
            host.listen(listener)
        };
        let host = host.expect("actix-web adopts the listener");
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
