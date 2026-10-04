//! The hyper backend for `ulo-http` (transports DESIGN §3.7), the reference server and the
//! default: one `ulo_http::Backend` that owns the listeners. Each connection is served by hyper's
//! connection builders, configured from the server's settings; each request is converted into a
//! `ulo_http::Request` and answered by the `AppService`. `ulo-http` owns routing.
//!
//! ```ignore
//! let app = app.bind(ulo_http_hyper::Server::new("0.0.0.0:8080")).listen().await?;
//! ```
//!
//! Documented limits: none. HTTP/1.1 and HTTP/2 (h2c when the server enables it), TLS through
//! `ulo_net::Tls`, inherited sockets, port 0, and per-request upgrades through hyper's upgrade
//! future.
//!
//! An application running inside another framework's server binds `ulo_http::embed::Embedded`
//! through that framework's adapter crate instead.

mod backend;
mod convert;
mod listener;

pub use backend::Hyper;

/// `ulo_http::Server` over hyper: `ulo_http_hyper::Server::new("0.0.0.0:8080")`.
pub type Server = ulo_http::Server<Hyper>;
