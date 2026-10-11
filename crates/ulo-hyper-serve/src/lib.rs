//! The accept loop every socket-owning `ulo` server over hyper shares (transports DESIGN §3.7):
//! the HTTP backend, the gRPC server and the standalone WebSocket server.
//!
//! [`Serve`] adopts the listeners `ulo-net` bound through a [`Listener`], the plug point a runtime
//! crate implements with its own sockets and TLS stack: `ulo-listen-tokio` and `ulo-listen-smol`.
//! It accepts on every listener, finishes each connection's handshake under
//! [`ServeConfig::handshake_timeout`], and spawns one task per accepted connection through the
//! app's `Runtime`, handing the connection to a closure the consumer supplies. The closure is where
//! the servers differ: it drives hyper's `auto`, `http2` or `http1` builder over [`Accepted::io`],
//! and starts the connection's graceful shutdown when [`Accepted::draining`] resolves.
//!
//! ```ignore
//! let serve = Serve::<ulo_listen_tokio::TokioListener>::new(listeners, tls, &config, Arc::clone(mounted.runtime()))?;
//! serve.run(move |accepted: Accepted<_>| {
//!     let builder = builder.clone();
//!     async move { /* drive `builder` over `accepted.io` until it ends */ }
//! }).await?;
//! ```
//!
//! [`Serve::drain`] stops accepting, drops the listeners and resolves every connection's
//! `draining`, then waits for the connection tasks; [`Serve::close`] aborts whatever is left. A
//! failed or timed-out handshake is logged at `debug` with the peer and the connection dropped;
//! the accept loop is unaffected. [`ReadCount`], handed in through [`ServeConfig::read_count`],
//! counts the connections the server has read from.
//!
//! What hyper needs beyond a connection's I/O comes from the app's runtime too: [`RuntimeExecutor`]
//! spawns HTTP/2's stream tasks and [`RuntimeTimer`] times the HTTP/1.1 header read. [`FuturesIo`]
//! carries a `futures-io` stream into hyper's I/O traits and hyper's own I/O, an upgraded
//! connection, out to `futures-io`'s.
//!
//! Nothing here starts or names a runtime. hyper itself depends on tokio with its `sync` feature,
//! for a channel inside its upgrade future, so tokio's synchronisation primitives are in every
//! hyper server's tree, this crate's included; its executor, reactor and timers are not.

mod listener;
mod rt;
mod serve;

pub use listener::{Incoming, Io, Listener, ReadCount, tls_info};
pub use rt::{FuturesIo, RuntimeExecutor, RuntimeTimer};
pub use serve::{Accepted, Draining, Serve, ServeConfig};
