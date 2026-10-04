//! The accept loop every socket-owning `ulo` server shares (transports DESIGN §3.7): the HTTP
//! backend over hyper, the gRPC server and the standalone WebSocket server.
//!
//! [`Serve`] takes the listeners `ulo-net` bound and an optional TLS acceptor, accepts on every
//! listener, performs each TLS handshake under [`ServeConfig::handshake_timeout`], and spawns one
//! task per accepted connection into a `JoinSet`, handing the connection to a closure the
//! consumer supplies. The closure is where the three servers differ: it drives hyper's `auto`,
//! `http2` or `http1` builder over [`Accepted::io`], and starts the connection's graceful shutdown
//! when [`Accepted::draining`] resolves.
//!
//! ```ignore
//! let serve = Serve::new(listeners, tls, &ServeConfig { handshake_timeout: Some(Duration::from_secs(30)) })?;
//! serve.run(move |accepted: Accepted| {
//!     let builder = builder.clone();
//!     async move { /* drive `builder` over `TokioIo::new(accepted.io)` until it ends */ }
//! }).await?;
//! ```
//!
//! [`Serve::drain`] stops accepting, drops the listeners and resolves every connection's
//! `draining`, then waits for the connection tasks; [`Serve::close`] aborts whatever is left. A
//! failed or timed-out handshake is logged at `debug` with the peer and the connection dropped;
//! the accept loop is unaffected.

mod handshake;
mod listener;
mod serve;

pub use listener::Io;
pub use serve::{Accepted, Draining, Serve, ServeConfig};
