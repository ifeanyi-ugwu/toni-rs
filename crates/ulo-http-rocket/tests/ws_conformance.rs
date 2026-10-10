//! The WebSocket conformance suite against `WsModule`'s hand-off inside a rocket host, the app's
//! catch-all routes mounted at `/`: each scenario on a multi-thread tokio runtime of its own, a
//! client reaching rocket over TCP. rocket declares `upgrades`, and its adapter sets the upgrade on
//! the request it builds for the app.

use std::future::Future;
use std::io;
use std::sync::Mutex;

use rocket::fairing::AdHoc;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use ulo::app::{Bound, Connected};
use ulo::{App, BoundAddr, Signal};
use ulo_http::Upgraded;
use ulo_http::embed::Embed;
use ulo_http_rocket::{Embedded, Handle, Rocket, routes};
use ulo_tokio::Tokio;
use ulo_ws::Port;
use ulo_ws_conformance::{Host, Serving, report, startup_failed, ws_conformance_suite};

struct RocketHost {
    embedded: Handle,
}

impl Host for RocketHost {
    type Runtime = Tokio;
    type Stream = Upgraded;

    const PORT: Port = Port::Http;

    fn upgrades() -> bool {
        <Rocket as Embed>::limits().upgrades
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
        (app.bind(server), RocketHost { embedded })
    }

    /// rocket 0.5 binds and accepts inside `launch`, and neither takes a listener nor reports a
    /// connection.
    fn connections_read(&self) -> Option<usize> {
        None
    }

    async fn serve(&self, app: App<Bound>) -> Serving {
        let figment = rocket::Config::figment().merge(("address", "127.0.0.1")).merge(("port", 0)).merge(("log_level", "off"));
        // rocket binds inside `launch`, so the port it took is read once it has lifted off.
        let (bound, port) = oneshot::channel::<u16>();
        let bound = Mutex::new(Some(bound));
        let host = rocket::custom(figment)
            .attach(AdHoc::on_liftoff("bound port", move |rocket| {
                let port = rocket.config().port;
                Box::pin(async move {
                    if let Some(bound) = bound.lock().ok().and_then(|mut bound| bound.take()) {
                        let _ = bound.send(port);
                    }
                })
            }))
            .mount("/", routes(&self.embedded));
        let embedded = self.embedded.clone();
        let serving = tokio::spawn(async move { ulo_http_rocket::run(app, &embedded, host, std::future::pending::<Signal>()).await });
        // A launch that fails never lifts off and drops the sender; `run` answers why.
        let Ok(port) = port.await else {
            match serving.await {
                Ok(Err(error)) => startup_failed!("rocket did not lift off: {}", report(&*error)),
                Ok(Ok(shutdown)) => startup_failed!("rocket did not lift off; the app shut down on: {}", shutdown.signal),
                Err(error) => startup_failed!("rocket did not lift off, and its task failed: {}", report(&error)),
            }
        };
        Serving {
            addresses: vec![BoundAddr::new("http", ([127, 0, 0, 1], port).into())],
            until_closed: Box::pin(async move {
                let _ = serving.await;
            }),
        }
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Upgraded> {
        TcpStream::connect(addresses[0].addr).await.map(Upgraded::from_tokio)
    }
}

ws_conformance_suite!(RocketHost; not_applicable {
    handshake_refuses_http_1_0: "F382: rocket 0.5 keeps no protocol version on its `Request`, so the adapter hands the app every request as HTTP/1.1 and an HTTP/1.0 upgrade switches protocols",
    handshake_refuses_during_the_drain: "F378: rocket 0.5 binds and accepts inside `launch`, takes no listener and reports no connection, so the suite cannot show an upgrade request in progress when the drain begins",
});
