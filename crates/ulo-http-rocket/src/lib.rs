//! Runs a `ulo` application inside a rocket server (transports DESIGN §3.8): a native embedding,
//! one catch-all `rocket::route::Handler` per method, mounted at a path.
//!
//! ```ignore
//! let server = ulo_http_rocket::Embedded::new().nested_at("/api");
//! let embedded = server.handle();
//! let app = App::builder(AppModule).timer(ulo_tokio::Timer).wire()?.connect().await?.bind(server).listen().await?;
//!
//! let rocket = rocket::build().mount("/api", ulo_http_rocket::routes(&embedded));
//! ulo_http_rocket::run(app, &embedded, rocket, ulo_tokio::shutdown_signal()).await?;
//! ```
//!
//! rocket 0.5 is on `http` 0.2, so the adapter converts at its edge. `mount` leaves the URI whole,
//! so the adapter declares `STRIPS_PREFIX: false` and the app strips `.nested_at` itself. rocket's
//! `Data` must be read inside the handler, so the body is buffered under the embedding's own
//! `body_limit` once the app first reads it, a larger one answered with the same 413 as everywhere
//! else; a body the app never reads stays unread, which is what lets a `Miss::Forward` miss go back
//! to rocket with its `Data`. rocket's `Response` has no extensions, so `Routing` lives in the
//! request's local cache as an `Option<Routing>`, where a fairing's `on_response` reads it with
//! `req.local_cache(|| None::<Routing>)`; `None` there means the app did not answer.
//!
//! What it declares: the peer address from `remote`; upgrades through rocket's `IoHandler`; miss
//! forwarding, `Outcome::Forward` with the request's unread `Data`; no host extensions, a value in
//! rocket's local cache crossing through a `forward` copy; no TLS info; the body buffered; a
//! dropped body observed at the next failed write.
//!
//! Built-in forwards: `OriginalPath`, from `req.uri().path()`.

mod convert;
mod handler;
mod run;
mod upgrade;

use ulo_http::HttpConfig;
use ulo_http::embed::{Disconnect, Embed, EmbedLimits, OriginalPath, RequestBody};

pub use handler::{RocketHandler, routes};
pub use run::run;

/// The rocket host.
pub struct Rocket;

impl Embed for Rocket {
    const NAME: &'static str = "rocket";

    const STRIPS_PREFIX: bool = false;

    type HostRequest<'r> = rocket::Request<'r>;

    /// `request_body` reports `Buffered` at the default `body_limit`, 2 MiB: the declaration is
    /// per host, and an embedding's own `.body_limit(..)` is the cap its requests are buffered
    /// under.
    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
            .host_extensions(false)
            .tls_info(false)
            .request_body(RequestBody::Buffered(HttpConfig::default().body_limit))
            .disconnect(Disconnect::AtNextWrite)
    }

    fn builtin_forwards(embedded: ulo_http::embed::Embedded<Self>) -> ulo_http::embed::Embedded<Self> {
        embedded.forward(|req: &rocket::Request<'_>| Some(OriginalPath::new(req.uri().path().as_str())))
    }
}

/// The embedding server for rocket, `ulo_http::embed::Embedded<Rocket>`, whose constructor
/// registers the adapter's built-in `forward` of `OriginalPath` from `req.uri().path()` through
/// `Embed::builtin_forwards`.
pub type Embedded = ulo_http::embed::Embedded<Rocket>;

/// Its handle, `ulo_http::embed::Handle<Rocket>`.
pub type Handle = ulo_http::embed::Handle<Rocket>;
