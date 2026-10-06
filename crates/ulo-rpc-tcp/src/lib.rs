//! The TCP link for `ulo-rpc` (transports DESIGN §5.3): calls multiplexed by `id` over one
//! connection, each frame a 4-byte big-endian length then the frame, bounded by `max_frame`, so an
//! oversized prefix closes the connection before the body is read. Ordered per connection; every
//! call shape; `Addressed`; TLS through `ulo_net::Tls` with no ALPN; `goaway` on every connection
//! at the drain.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")))
//! ```

mod link;

pub use link::Tcp;
