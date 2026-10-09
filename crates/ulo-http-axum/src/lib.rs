//! Runs a `ulo` application inside an axum server (transports DESIGN §3.8): the one tower
//! embedding. The host owns the sockets, its own middleware wraps the app, and the app answers
//! through `embedded.service()`, nested or as the fallback.
//!
//! ```ignore
//! use tower::Layer;
//!
//! let server = ulo_http_axum::Embedded::new().nested_at("/api").peer_addr(true);
//! let embedded = server.handle();
//! let app = App::builder(AppModule).timer(ulo_tokio::Timer).wire()?.connect().await?.bind(server).listen().await?;
//!
//! let router = axum::Router::new()
//!     .nest_service("/api", ulo_http_axum::HostLayer.layer(embedded.service()))
//!     .into_make_service_with_connect_info::<SocketAddr>();
//! ulo_http_axum::run(app, &embedded, axum::serve(listener, router), ulo_tokio::shutdown_signal()).await?;
//! ```
//!
//! `nest_service` strips the prefix before the app sees the path. [`HostLayer`], wrapped around
//! the app's service, supplies the peer address and the original path, which axum keeps in
//! `ConnectInfo` and `OriginalUri`.
//!
//! What it declares: the peer address only through `ConnectInfo`, so `peer_addr` is `false` unless
//! `.peer_addr(true)`; upgrades through hyper's `OnUpgrade`; no miss forwarding; host extensions
//! read from `http::Extensions`; no TLS info; streamed bodies; a dropped body observed at the
//! disconnect.

mod layer;
mod run;

use hyper_util::rt::TokioIo;
use ulo::BoxError;
use ulo_http::embed::{Embed, EmbedLimits};
use ulo_http::{OnUpgrade, Upgraded};

pub use layer::{HostLayer, HostService};
pub use run::run;

/// The axum host.
pub struct Axum;

impl Embed for Axum {
    const NAME: &'static str = "axum";

    const STRIPS_PREFIX: bool = true;

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE.peer_addr(false).forward_miss(false).tls_info(false)
    }

    /// Hyper's `OnUpgrade`, which axum's server leaves in a request's extensions.
    fn take_upgrade(extensions: &mut http::Extensions) -> Option<OnUpgrade> {
        let pending = extensions.remove::<hyper::upgrade::OnUpgrade>()?;
        Some(OnUpgrade::new(async move {
            let upgraded = pending.await?;
            Ok::<_, BoxError>(Upgraded::from_tokio(TokioIo::new(upgraded)))
        }))
    }
}

/// The embedding server for axum, `ulo_http::embed::Embedded<Axum>`.
pub type Embedded = ulo_http::embed::Embedded<Axum>;

/// Its handle, `ulo_http::embed::Handle<Axum>`.
pub type Handle = ulo_http::embed::Handle<Axum>;

/// The tower service the host mounts, `ulo_http::embed::Service<Axum>`.
pub type Service = ulo_http::embed::Service<Axum>;
