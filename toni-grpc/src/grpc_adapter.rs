use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::body::Body as TonicBody;
use tonic::server::NamedService;
use tonic::service::{Routes, RoutesBuilder};
use tonic::transport::Server;
use tower::Service;

use toni::async_trait;
use toni::adapter::GrpcServiceTrait;

use crate::tracing_layer::TracingLayer;

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
    routes_builder: RoutesBuilder,
    drain_timeout: Option<Duration>,
    listener: Option<TcpListener>,
    local_addr: Option<SocketAddr>,
    /// Two consumers subscribe: tonic's `serve_with_incoming_shutdown` (so
    /// it begins natural drain), and the drain-timeout guard (so the deadline
    /// timer starts only *after* shutdown is signalled, not at startup).
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl GrpcAdapter {
    pub fn new(addr: SocketAddr) -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            addr,
            routes_builder: RoutesBuilder::default(),
            drain_timeout: Some(DEFAULT_DRAIN_TIMEOUT),
            listener: None,
            local_addr: None,
            shutdown_tx: Arc::new(tx),
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
        self.routes_builder.add_service(svc);
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
    fn bind(&mut self, services: Vec<Arc<Box<dyn GrpcServiceTrait>>>) -> Result<()> {
        // Framework-discovered services (`#[grpc_service]` + `#[grpc_methods]`)
        // each know how to wrap themselves in their tonic `*Server` — hand
        // them the same `RoutesBuilder` already accumulating any
        // user-`add_service`'d entries and let macro-generated code add
        // itself.
        for svc in &services {
            svc.register_with(&mut self.routes_builder as &mut dyn std::any::Any);
        }

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
        let routes: Routes = std::mem::take(&mut self.routes_builder).routes();
        let drain_timeout = self.drain_timeout;
        let addr = self.local_addr;

        let shutdown_tonic = self.shutdown_tx.subscribe();
        let shutdown_drain = self.shutdown_tx.subscribe();

        Ok(Box::pin(async move {
            // Tonic's shutdown signal: fires when the watch flips to true.
            // From there tonic begins natural drain — completes once every
            // in-flight RPC finishes (or the deadline below abort-races it).
            let shutdown_for_tonic = wait_for_shutdown(shutdown_tonic);

            // Drain deadline: the timer starts *only* after shutdown is
            // signalled. Without this gate, a `tokio::time::timeout` over
            // the whole serve future would cap total uptime, not drain time.
            let drain_guard = async move {
                wait_for_shutdown(shutdown_drain).await;
                match drain_timeout {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            };

            let server_fut = Server::builder()
                .layer(TracingLayer::new())
                .add_routes(routes)
                .serve_with_incoming_shutdown(
                    TcpListenerStream::new(listener),
                    shutdown_for_tonic,
                );

            tokio::pin!(server_fut);
            tokio::pin!(drain_guard);

            tokio::select! {
                res = &mut server_fut => {
                    if let Err(e) = res {
                        tracing::error!(?addr, error = %e, "GrpcAdapter serve error");
                    }
                }
                _ = &mut drain_guard => {
                    tracing::warn!(
                        ?addr,
                        timeout_ms = drain_timeout.map(|d| d.as_millis() as u64),
                        "GrpcAdapter drain timed out; in-flight streams will see UNAVAILABLE",
                    );
                    // Dropping `server_fut` by leaving the select arm is the
                    // abort: hyper closes connections, and any task still
                    // executing a streaming handler is cancelled when its
                    // task handle inside tonic gets dropped.
                }
            }
        }))
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    async fn close(&mut self) -> Result<()> {
        // Idempotent — the watch coalesces repeat sends into the same value.
        let _ = self.shutdown_tx.send(true);
        Ok(())
    }
}

/// `watch::Receiver::wait_for` returns a `Ref<'_, bool>` guard that's
/// `!Send`; holding it across an `.await` would force the whole serve
/// future to be `!Send`. Discarding the guard inside this helper keeps the
/// outer scope's send-ness.
async fn wait_for_shutdown(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

