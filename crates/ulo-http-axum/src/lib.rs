//! The axum backend for `ulo-http` (transports DESIGN §3.7): one `ulo_http::Backend`. Each
//! connection is served by the hyper connection builders `axum::serve` uses, configured from the
//! server's settings, which `axum::serve` takes none of; each request becomes an axum request,
//! converted into a `ulo_http::Request` and answered by the `AppService`. `ulo-http` owns routing,
//! so no axum router is involved.
//!
//! ```ignore
//! let app = app.bind(ulo_http_axum::Server::new("0.0.0.0:8080")).listen().await?;
//! ```
//!
//! Documented limits: none. HTTP/1.1 and HTTP/2 (h2c when the server enables it), TLS through
//! `ulo_net::Tls`, inherited sockets, port 0, and per-request upgrades through hyper's upgrade
//! future.

mod backend;
mod convert;
mod listener;

pub use backend::Axum;

/// `ulo_http::Server` over axum: `ulo_http_axum::Server::new("0.0.0.0:8080")`.
pub type Server = ulo_http::Server<Axum>;
