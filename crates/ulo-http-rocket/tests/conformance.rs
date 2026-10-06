//! The embedding conformance suite against a rocket host, the catch-all routes mounted at `PREFIX`
//! and at `/`. rocket's request store, the local cache, does not reach the app
//! (`host_extensions: false`), so a request fairing puts `HostValue` there and an
//! `Embedded::forward` copy carries it across; `RoutingFairing` writes the `Routing` header.

use rocket::fairing::AdHoc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_conformance::rocket_fairing::RoutingFairing;
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX};
use ulo_http_rocket::{Embedded, Rocket, routes};

struct RocketHost {
    base_url: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

impl Host for RocketHost {
    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        }
        .forward(|req: &rocket::Request<'_>| req.local_cache(|| None::<HostValue>).clone());
        let embedded = server.handle();
        let app = app.bind(server).listen().await.expect("the app listens inside rocket");
        let figment = rocket::Config::figment()
            .merge(("address", "127.0.0.1"))
            .merge(("port", 0))
            .merge(("log_level", "off"));
        // rocket binds inside `launch`, so the port it took is read once it has lifted off.
        let (bound, port) = oneshot::channel::<u16>();
        let bound = std::sync::Mutex::new(Some(bound));
        let host = rocket::custom(figment)
            .attach(AdHoc::on_request("host value", |req, _| {
                Box::pin(async move {
                    let value = req.headers().get_one(HOST_VALUE_HEADER).map(|value| HostValue(value.to_owned()));
                    req.local_cache(|| value);
                })
            }))
            .attach(RoutingFairing)
            .attach(AdHoc::on_liftoff("bound port", move |rocket| {
                let port = rocket.config().port;
                Box::pin(async move {
                    if let Some(bound) = bound.lock().ok().and_then(|mut bound| bound.take()) {
                        let _ = bound.send(port);
                    }
                })
            }))
            .mount(if mode == Mode::Nested { PREFIX } else { "/" }, routes(&embedded));
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_rocket::run(app, &embedded, host, signal).await;
        });
        let port = port.await.expect("rocket lifts off");
        RocketHost { base_url: format!("http://127.0.0.1:{port}"), stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Rocket as Embed>::limits()
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(RocketHost);
