//! The UDP link for `ulo-rpc` (transports DESIGN §5.3): one frame per datagram, replies sent to
//! the sender's address. Unordered with no delivery guarantee; a payload of at most 65,507 bytes
//! less the envelope; unary calls and events only, a streamed shape refused at startup;
//! `Addressed`; a call sent as the server closes can be lost, its datagram still in the socket, and
//! its caller sees its own `Timeout` (`unconfirmed_drain`). No TLS: DTLS is not supported, so
//! `Udp::tls` does not exist.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_udp::Udp::new("0.0.0.0:7001")))
//! ```

mod link;

pub use link::Udp;
