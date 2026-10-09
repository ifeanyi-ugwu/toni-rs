//! What every socket-owning transport shares (transports DESIGN §2.7): where to listen
//! ([`Endpoint`]), sockets handed in by a supervisor ([`Activation`]), TLS loaded with rustls
//! ([`Tls`]), and binding with the address the OS chose ([`BoundListener`], [`BoundAddr`]).
//!
//! Nothing here belongs to a runtime: sockets are `std::net`'s, which each runtime crate adopts
//! into its reactor, and TLS ends at rustls's `ServerConfig`, which each runtime's TLS crate wraps
//! around its own streams: `tokio_rustls::TlsAcceptor::from(config)` on tokio,
//! `futures_rustls::TlsAcceptor::from(config)` over `futures-io`.
//!
//! Everything that can fail without a socket fails in a server's `prepare`, so a bad endpoint, a
//! missing inherited socket, a `LISTEN_PID` mismatch or a bad certificate is a
//! `StartupError::Configure`, reported before any port conflict.

mod activation;
mod endpoint;
mod listener;
mod tls;

pub use activation::{Activation, ActivationError};
pub use endpoint::{Endpoint, EndpointError, EndpointSpec, ListenerName};
pub use listener::{BindError, BoundListener, bind_all};
pub use tls::{Tls, TlsError};
pub use ulo::BoundAddr;
/// The rustls whose `ServerConfig` [`Tls::load`] answers.
pub use rustls;
