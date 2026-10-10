//! The actix-web host both conformance targets run. actix's request store does not reach the app
//! (`host_extensions: false`), so the host's middleware puts `HostValue` there and an
//! `Embedded::forward` copy carries it across.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, report, startup_failed, routing_label};

/// The actix-web host. `H2C` listens with `listen_auto_h2c`, which serves HTTP/2 without TLS
/// beside HTTP/1.1 and exists only where actix-web's `http2` feature is on; otherwise `listen`,
/// which serves HTTP/1.1 alone.
pub struct ActixHost<const H2C: bool> {
    base_url: String,
    /// The connections actix has handed to its worker, counted by `on_connect` as each starts,
    /// before anything is read: actix takes a listener and gives no hold on a connection's stream.
    /// `drain_http1` asks no more of a host declaring `DrainPending::Closed`, as actix does.
    started: Arc<AtomicUsize>,
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
        let app = app.bind(server).listen().await
            .unwrap_or_else(|error| startup_failed!("the app did not listen inside actix-web: {}", report(&error)));
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        let started = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&started);
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
        .on_connect(move |_, _| {
            counted.fetch_add(1, Ordering::AcqRel);
        })
        .workers(1);
        #[cfg(feature = "conformance-http2")]
        let host = if H2C { host.listen_auto_h2c(listener) } else { host.listen(listener) };
        #[cfg(not(feature = "conformance-http2"))]
        let host = {
            const { assert!(!H2C, "an h2c host needs actix-web's `http2`: `--features conformance-http2`") };
            host.listen(listener)
        };
        let host = host.unwrap_or_else(|error| startup_failed!("actix-web did not adopt the listener: {}", report(&error)));
        let (stop, stopped) = oneshot::channel::<()>();
        let signal = async move {
            let _ = stopped.await;
            Signal::new("suite")
        };
        let running = ulo_http_actix::run(app, &embedded, host, signal);
        let serving = tokio::spawn(async move {
            let _ = running.await;
        });
        ActixHost { base_url: format!("http://{addr}"), started, stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Actix as Embed>::limits()
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.started.load(Ordering::Acquire))
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}
