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
//! so the adapter strips `.nested_at` itself. rocket's `Data` must be read inside the handler, so
//! the body is buffered under the embedding's own `body_limit`, a larger one answered with the same
//! 413 as everywhere else. `Routing` lives in the request's local cache, where a fairing's
//! `on_response` reads it. What it declares: the peer address from `remote` and `client_ip`;
//! upgrades through rocket's `IoHandler`; miss forwarding, `Outcome::Forward` with the request's
//! unread `Data`; no host extensions, a value in rocket's local cache crossing through a `forward`
//! copy, the adapter's own `OriginalPath` among them; no TLS info; the body buffered; a dropped
//! body observed at the next failed write.

mod convert;
mod handler;
mod run;
mod upgrade;

use ulo_http::embed::{Embed, EmbedLimits};

pub use handler::{RocketHandler, routes};
pub use run::run;

/// The rocket host.
pub struct Rocket;

impl Embed for Rocket {
    const NAME: &'static str = "rocket";

    type HostRequest<'r> = rocket::Request<'r>;

    fn limits() -> EmbedLimits {
        todo!()
    }
}

/// The embedding server for rocket, `ulo_http::embed::Embedded<Rocket>`, whose constructor
/// registers the adapter's built-in `forward` of `OriginalPath` from `req.uri().path()`.
pub type Embedded = ulo_http::embed::Embedded<Rocket>;

/// Its handle, `ulo_http::embed::Handle<Rocket>`.
pub type Handle = ulo_http::embed::Handle<Rocket>;
