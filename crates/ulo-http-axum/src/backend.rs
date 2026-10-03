use std::sync::Mutex;

use tokio::sync::watch;
use ulo::BoxError;
use ulo_http::{AppService, Backend, BackendLimits, HttpConfig};
use ulo_net::{BoundListener, TlsAcceptor};

use crate::listener::Listener;

/// The axum backend. `Default`, so `ulo_http_axum::Server::new(endpoint)` builds it.
#[derive(Default)]
pub struct Axum {
    /// Set by `bind`, taken by `serve`.
    pub(crate) bound: Mutex<Option<Bound>>,
    /// `drain` sends `true`: every listener's `axum::serve` starts its graceful shutdown, which
    /// sends GOAWAY on HTTP/2 and closes idle HTTP/1.1 keep-alive connections.
    pub(crate) draining: Option<watch::Sender<bool>>,
    /// `close` sends `true`: connections still open are dropped.
    pub(crate) closing: Option<watch::Sender<bool>>,
}

/// What `bind` prepared for `serve`.
pub(crate) struct Bound {
    pub(crate) listeners: Vec<Listener>,
    pub(crate) service: AppService,
}

impl Backend for Axum {
    const NAME: &'static str = "axum";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(
        &mut self,
        listeners: Vec<BoundListener>,
        tls: Option<TlsAcceptor>,
        svc: AppService,
        cfg: &HttpConfig,
    ) -> Result<(), BoxError> {
        let _ = (listeners, tls, svc, cfg);
        todo!("adopt each listener with `tokio::net::TcpListener::from_std`, wrap TLS, store `Bound`, create the watch channels")
    }

    async fn serve(&self) -> Result<(), BoxError> {
        todo!("`axum::serve` per listener with the fallback service, graceful on `draining`, aborted on `closing`; join them")
    }

    async fn drain(&self) {
        todo!("send `true` on `draining`")
    }

    async fn close(&self) -> Result<(), BoxError> {
        todo!("send `true` on `closing`; wait for the serve tasks to end")
    }
}
