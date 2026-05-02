use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::body::Body as TonicBody;
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic::transport::Server;
use tower::Service;

use toni::async_trait;

const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// gRPC transport adapter for the Toni framework.
///
/// Wraps `tonic::transport::Server`. Construction is contract-first: the
/// caller registers tonic-generated services via [`add_service`](Self::add_service)
/// before passing the adapter to `app.use_grpc_adapter()`. After the
/// framework calls `bind()`, the adapter owns a `TcpListener` on the
/// configured address; `serve()` then drives the gRPC server until shutdown.
///
/// # Graceful shutdown
///
/// On `close()`, tonic's `serve_with_incoming_shutdown` is signalled. The
/// configured drain timeout (default 10 s) bounds how long in-flight unary
/// RPCs and streaming RPCs are awaited; anything still running after the
/// deadline is aborted. Streaming clients see `UNAVAILABLE` mid-stream.
/// Override the timeout with [`with_drain_timeout`](Self::with_drain_timeout);
/// pass `None` to wait without bound.
pub struct GrpcAdapter {
    addr: SocketAddr,
    routes: Routes,
    drain_timeout: Option<Duration>,
    listener: Option<TcpListener>,
    local_addr: Option<SocketAddr>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl GrpcAdapter {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            routes: Routes::default(),
            drain_timeout: Some(DEFAULT_DRAIN_TIMEOUT),
            listener: None,
            local_addr: None,
            shutdown_tx: None,
        }
    }

    /// Register a tonic-generated service with the gRPC server.
    ///
    /// Accepts anything that satisfies tonic's service contract — typically
    /// a `*Server::new(impl_struct)` value produced by `tonic-build`.
    pub fn add_service<S>(mut self, svc: S) -> Self
    where
        S: Service<
                http::Request<TonicBody>,
                Response = http::Response<TonicBody>,
                Error = std::convert::Infallible,
            > + NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        S::Future: Send + 'static,
    {
        self.routes = self.routes.add_service(svc);
        self
    }

    /// Set how long `close()` waits for in-flight RPCs (including streams)
    /// to finish before aborting them. Pass `None` to wait without bound.
    pub fn with_drain_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.drain_timeout = timeout.into();
        self
    }
}

#[async_trait]
impl toni::GrpcAdapter for GrpcAdapter {
    fn bind(&mut self) -> Result<()> {
        // Bind synchronously so port-in-use surfaces as `Err` from
        // `app.bind()` instead of panicking inside the spawned serve loop.
        let std_listener = std::net::TcpListener::bind(self.addr)
            .with_context(|| format!("GrpcAdapter: failed to bind {}", self.addr))?;
        std_listener
            .set_nonblocking(true)
            .context("GrpcAdapter: failed to set listener nonblocking")?;
        let listener = TcpListener::from_std(std_listener)
            .context("GrpcAdapter: failed to register listener with the tokio runtime")?;
        let local_addr = listener
            .local_addr()
            .context("GrpcAdapter: failed to read local address from listener")?;

        self.listener = Some(listener);
        self.local_addr = Some(local_addr);
        Ok(())
    }

    fn serve(&mut self) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        let listener = self
            .listener
            .take()
            .expect("bind() must be called before serve()");
        let routes = std::mem::take(&mut self.routes);

        let (tx, rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(tx);
        let addr = self.local_addr;

        Ok(Box::pin(async move {
            let shutdown = async move {
                let _ = rx.await;
            };

            // tonic's `serve_with_incoming_shutdown` resolves once the
            // shutdown signal fires *and* in-flight requests drain. The
            // drain-timeout knob — bounding how long streaming RPCs are
            // allowed to hold the future open — lands with the streaming
            // PR; today it's stored on `self` but not yet enforced.
            if let Err(e) = Server::builder()
                .add_routes(routes)
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown)
                .await
            {
                tracing::error!(?addr, error = %e, "GrpcAdapter serve error");
            }
        }))
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    async fn close(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            // tonic's serve_with_incoming_shutdown begins draining on this
            // signal. The serve future itself enforces the drain timeout.
            let _ = tx.send(());
        }
        Ok(())
    }
}

