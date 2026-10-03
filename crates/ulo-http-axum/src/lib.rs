//! The axum backend for `ulo-http` (transports DESIGN §3.7): one `ulo_http::Backend`, serving
//! through `axum::serve` with a fallback service that converts each request into a
//! `ulo_http::Request` and calls the `AppService`. `ulo-http` owns routing, so axum's router holds
//! nothing but that fallback.
//!
//! ```ignore
//! let app = app.bind(ulo_http_axum::Server::new("0.0.0.0:8080")).listen().await?;
//! ```
//!
//! Documented limits: none. HTTP/1.1 and HTTP/2 (h2c included), TLS through `ulo_net::Tls`,
//! inherited sockets, port 0, and per-request upgrades through `hyper::upgrade::on`.

mod backend;
mod convert;
mod listener;

pub use backend::Axum;

/// `ulo_http::Server` over axum: `ulo_http_axum::Server::new("0.0.0.0:8080")`.
pub type Server = ulo_http::Server<Axum>;
