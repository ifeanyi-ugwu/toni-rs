//! The embedding conformance suite against a salvo host, nested under `PREFIX` with
//! `api/{**rest}` and as the catch-all `{**rest}`.

use salvo::conn::tcp::TcpAcceptor;
use salvo::http::HeaderValue;
use salvo::{Depot, FlowCtrl, Request, Response, Router};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_conformance::{HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, routing_label};
use ulo_http_salvo::{Embedded, Salvo, handler};

struct SalvoHost {
    base_url: String,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

/// The host's hoop, around the app's handler: the header's value into the request's extensions,
/// and the app's `Routing` onto the response.
struct HostHoop;

#[salvo::async_trait]
impl salvo::Handler for HostHoop {
    async fn handle(&self, req: &mut Request, depot: &mut Depot, res: &mut Response, ctrl: &mut FlowCtrl) {
        let value = req.headers().get(HOST_VALUE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
        if let Some(value) = value {
            req.extensions_mut().insert(HostValue(value));
        }
        ctrl.call_next(req, depot, res).await;
        if let Some(label) = res.extensions.get::<Routing>().map(routing_label) {
            res.headers.insert(ROUTING_HEADER, HeaderValue::from_str(&label).expect("a label is a header value"));
        }
    }
}

impl Host for SalvoHost {
    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        };
        let embedded = server.handle();
        let app = app.bind(server).listen().await.expect("the app listens inside salvo");
        let path = match mode {
            Mode::Nested => format!("{}/{{**rest}}", PREFIX.trim_start_matches('/')),
            Mode::Fallback => "{**rest}".to_owned(),
        };
        let router = Router::new().hoop(HostHoop).push(Router::with_path(path).goal(handler(&embedded)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("the listener's address");
        let acceptor = TcpAcceptor::try_from(listener).expect("salvo adopts the listener");
        let host = salvo::Server::new(acceptor);
        let service = salvo::Service::new(router);
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_salvo::run(app, &embedded, host, service, signal).await;
        });
        SalvoHost { base_url: format!("http://{addr}"), stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Salvo as Embed>::limits()
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(SalvoHost);
