//! The embedding conformance suite against a salvo host, nested under `PREFIX` with
//! `api/{**rest}` and as the catch-all `{**rest}`.

use std::io::Result as IoResult;
use std::time::Duration;

use salvo::conn::tcp::{TcpAcceptor, TcpCoupler};
use salvo::conn::{Accepted, Acceptor, Holding};
use salvo::fuse::ArcFuseFactory;
use salvo::http::HeaderValue;
use salvo::{Depot, FlowCtrl, Request, Response, Router};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use ulo::app::Connected;
use ulo::{App, Signal};
use ulo_http::Routing;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http_conformance::{
    Counted, HOST_VALUE_HEADER, Host, HostValue, Mode, PREFIX, ROUTING_HEADER, ReadCount, report, routing_label, startup_failed,
};
use ulo_http_salvo::{Closing, Embedded, Salvo, handler};

struct SalvoHost {
    base_url: String,
    read_count: ReadCount,
    stop: oneshot::Sender<()>,
    serving: JoinHandle<()>,
}

/// salvo's acceptor, each connection it accepts counted at its first read.
struct Counting {
    tcp: TcpAcceptor,
    read_count: ReadCount,
}

type TcpStream = <TcpAcceptor as Acceptor>::Stream;

impl Acceptor for Counting {
    type Coupler = TcpCoupler<Counted<TcpStream>>;
    type Stream = Counted<TcpStream>;

    fn holdings(&self) -> &[Holding] {
        self.tcp.holdings()
    }

    async fn accept(&mut self, fuse_factory: Option<ArcFuseFactory>) -> IoResult<Accepted<Self::Coupler, Self::Stream>> {
        let Accepted { stream, fusewire, local_addr, remote_addr, http_scheme, .. } = self.tcp.accept(fuse_factory).await?;
        let stream = self.read_count.wrap(stream);
        Ok(Accepted { coupler: TcpCoupler::new(), stream, fusewire, local_addr, remote_addr, http_scheme })
    }
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
    type Harness = ulo_http_conformance::OnTokio;

    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let server = match mode {
            Mode::Nested => Embedded::new().nested_at(PREFIX),
            Mode::Fallback => Embedded::new(),
        };
        let embedded = server.handle();
        let app = app.bind(server).listen().await.unwrap_or_else(|error| startup_failed!("the app did not listen inside salvo: {}", report(&error)));
        let path = match mode {
            Mode::Nested => format!("{}/{{**rest}}", PREFIX.trim_start_matches('/')),
            Mode::Fallback => "{**rest}".to_owned(),
        };
        let router = Router::new().hoop(HostHoop).push(Router::with_path(path).goal(handler(&embedded)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await
            .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
        let addr = listener.local_addr().unwrap_or_else(|error| startup_failed!("the host's listener has no address: {}", report(&error)));
        let acceptor = TcpAcceptor::try_from(listener)
            .unwrap_or_else(|error| startup_failed!("salvo did not adopt the listener: {}", report(&error)));
        let read_count = ReadCount::default();
        let acceptor = Counting { tcp: acceptor, read_count: read_count.clone() };
        let server = salvo::Server::new(Closing::new(&embedded, acceptor));
        let service = salvo::Service::new(router);
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(async move {
            let signal = async move {
                let _ = stopped.await;
                Signal::new("suite")
            };
            let _ = ulo_http_salvo::run(app, &embedded, server, service, signal).await;
        });
        SalvoHost { base_url: format!("http://{addr}"), read_count, stop, serving }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        <Salvo as Embed>::limits()
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.read_count.get())
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.serving.await;
    }
}

ulo_http_conformance::http_conformance_suite!(SalvoHost);

/// A server built over a `Closing` from another embedding's handle would keep its listener open
/// through this embedding's drain; `run` refuses it.
#[tokio::test]
async fn run_refuses_a_closing_from_another_handle() {
    let server = Embedded::new();
    let embedded = server.handle();
    let other = Embedded::new().handle();
    let app = ulo_http_conformance::app_for(<Salvo as Embed>::limits(), ulo_tokio::Tokio::current()).await;
    let app = app.bind(server).listen().await.unwrap_or_else(|error| startup_failed!("the app did not listen inside salvo: {}", report(&error)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await
        .unwrap_or_else(|error| startup_failed!("the host did not bind a port: {}", report(&error)));
    let acceptor = TcpAcceptor::try_from(listener).unwrap_or_else(|error| startup_failed!("salvo did not adopt the listener: {}", report(&error)));
    let server = salvo::Server::new(Closing::new(&other, acceptor));
    let service = salvo::Service::new(Router::new());
    let run = ulo_http_salvo::run(app, &embedded, server, service, std::future::pending());
    let served = tokio::time::timeout(Duration::from_secs(5), run).await.expect("`run` answers rather than serving");
    let error = served.expect_err("`run` refuses a `Closing` built from another handle").to_string();
    assert!(error.contains("another embedding's handle"), "the refusal names the mismatch, got: {error}");
}
