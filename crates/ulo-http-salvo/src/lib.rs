//! Runs a `ulo` application inside a salvo server (transports DESIGN §3.8): a native embedding,
//! the adapter implementing `salvo::Handler`, nested under a path or as the catch-all.
//!
//! ```ignore
//! let server = ulo_http_salvo::Embedded::new().nested_at("/api");
//! let embedded = server.handle();
//! let app = App::builder(AppModule).timer(ulo_tokio::Timer).wire()?.connect().await?.bind(server).listen().await?;
//!
//! let router = salvo::Router::new().push(salvo::Router::with_path("api/{**rest}").goal(ulo_http_salvo::handler(&embedded)));
//! let acceptor = salvo::conn::TcpListener::new("0.0.0.0:8080").bind().await;
//! let server = salvo::Server::new(ulo_http_salvo::Closing::new(&embedded, acceptor));
//! ulo_http_salvo::run(app, &embedded, server, salvo::Service::new(router), ulo_tokio::shutdown_signal()).await?;
//! ```
//!
//! The server is built over [`Closing`], which drops the acceptor it wraps when the drain begins
//! and so closes its listener; `run`'s parameter type, `salvo::Server<Closing<A>>`, requires it.
//! The rest of the server is configured as usual, `with_http_builder`, `http1_mut`, `http2_mut`
//! and `fuse_factory` included.
//!
//! salvo hands its handler the full path, so the adapter declares `STRIPS_PREFIX: false` and the
//! app strips `.nested_at` itself; a request it receives outside the prefix is answered as the
//! app's 404 and logged at `warn` once. What it declares: the peer address from
//! `Request::remote_addr`; upgrades through `OnUpgrade` left in the extensions; no miss
//! forwarding; host extensions copied from `Request::extensions`, a value a middleware put in the
//! `Depot` crossing through a `forward` copy over [`SalvoRequest`]; no TLS info; streamed bodies; a
//! dropped body observed at the disconnect.
//!
//! Built-in forwards: `OriginalPath`, from `req.uri().path()`. The app's response, `Routing`
//! included, goes into salvo's response extensions, where a hoop after the handler reads it.

mod handler;
mod run;

use ulo_http::embed::{Embed, EmbedLimits, OriginalPath};

pub use handler::{SalvoHandler, handler};
pub use run::{Closing, run};

/// The salvo host.
pub struct Salvo;

impl Embed for Salvo {
    const NAME: &'static str = "salvo";

    const STRIPS_PREFIX: bool = false;

    type HostRequest<'r> = SalvoRequest<'r>;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE.forward_miss(false).tls_info(false)
    }

    fn builtin_forwards(embedded: ulo_http::embed::Embedded<Self>) -> ulo_http::embed::Embedded<Self> {
        embedded.forward(|req: &SalvoRequest<'_>| Some(OriginalPath::from(req.request.uri())))
    }
}

/// The salvo request as a `forward` copy reads it: the request and the `Depot` beside it, where
/// salvo middleware conventionally stores what it learned.
#[derive(Clone, Copy)]
pub struct SalvoRequest<'r> {
    pub request: &'r salvo::Request,
    pub depot: &'r salvo::Depot,
}

/// The embedding server for salvo, `ulo_http::embed::Embedded<Salvo>`.
pub type Embedded = ulo_http::embed::Embedded<Salvo>;

/// Its handle, `ulo_http::embed::Handle<Salvo>`.
pub type Handle = ulo_http::embed::Handle<Salvo>;
