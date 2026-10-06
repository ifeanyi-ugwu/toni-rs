//! Runs a `ulo` application inside a poem server (transports DESIGN §3.8): a native embedding, the
//! adapter implementing `poem::Endpoint`, nested under a path or as the fallback.
//!
//! ```ignore
//! let server = ulo_http_poem::Embedded::new().nested_at("/api");
//! let embedded = server.handle();
//! let app = App::builder(AppModule).timer(ulo_tokio::Timer).wire()?.connect().await?.bind(server).listen().await?;
//!
//! let route = poem::Route::new().nest("/api", ulo_http_poem::endpoint(&embedded));
//! ulo_http_poem::run(app, &embedded, poem::Server::new(TcpListener::bind("0.0.0.0:3000")), route, ulo_tokio::shutdown_signal()).await?;
//! ```
//!
//! `Route::nest` strips the prefix, so the adapter declares `STRIPS_PREFIX: true`; mounting the
//! endpoint with `Route::at("/api/*path", ..)` instead leaves the prefix on the path and is not
//! supported. What it declares: the peer address from `remote_addr`; upgrades through
//! `take_upgrade`; no miss forwarding; host extensions from `Request::extensions`; no TLS info;
//! streamed bodies; a dropped body observed at the next failed write.
//!
//! Built-in forwards: `OriginalPath`, from `req.original_uri()`. The app's response, `Routing`
//! included, keeps its extensions in poem's response, where a middleware around the endpoint
//! reads it.

mod endpoint;
mod run;

use ulo_http::embed::{Disconnect, Embed, EmbedLimits, OriginalPath};

pub use endpoint::{PoemEndpoint, endpoint};
pub use run::run;

/// The poem host.
pub struct Poem;

impl Embed for Poem {
    const NAME: &'static str = "poem";

    const STRIPS_PREFIX: bool = true;

    type HostRequest<'r> = poem::Request;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE.forward_miss(false).tls_info(false).disconnect(Disconnect::AtNextWrite)
    }

    fn builtin_forwards(embedded: ulo_http::embed::Embedded<Self>) -> ulo_http::embed::Embedded<Self> {
        embedded.forward(|req: &poem::Request| Some(OriginalPath::from(req.original_uri())))
    }
}

/// The embedding server for poem, `ulo_http::embed::Embedded<Poem>`.
pub type Embedded = ulo_http::embed::Embedded<Poem>;

/// Its handle, `ulo_http::embed::Handle<Poem>`.
pub type Handle = ulo_http::embed::Handle<Poem>;
