//! The runtime half of a host on tokio.

use std::future::Future;
use std::io;
use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};
use ulo_listen_tokio::TokioListener;
use ulo_tokio::Tokio;

use crate::Harness;

/// Tokio as a host's [`Harness`]: each scenario on a multi-thread runtime of its own, the
/// reference host on `ulo-listen-tokio`'s listener, a client over tokio's `TcpStream`.
pub struct OnTokio;

impl Harness for OnTokio {
    type Runtime = Tokio;
    type Listener = TokioListener;
    type Stream = Compat<TcpStream>;

    fn runtime() -> Tokio {
        Tokio::current()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| crate::startup_failed!("a tokio runtime for the scenario: {error}"))
            .block_on(fut)
    }

    async fn connect(addr: SocketAddr) -> io::Result<Compat<TcpStream>> {
        TcpStream::connect(addr).await.map(TokioAsyncReadCompatExt::compat)
    }
}
