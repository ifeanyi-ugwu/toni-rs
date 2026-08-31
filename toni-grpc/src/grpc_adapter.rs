use std::net::SocketAddr;
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

use toni::adapter::{GrpcServiceSource, ResolvedGrpcEnhancers};
use toni::async_trait;

use crate::drain_layer::DrainLayer;
use crate::method_path_layer::MethodPathLayer;
use crate::tracing_layer::TracingLayer;

const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the adapter gets its socket. Holds a `SocketAddr` rather than
/// reusing [`toni::BindTarget`], whose address arm is a hostname string:
/// routing a `SocketAddr` through one would re-resolve it and drop an IPv6
/// scope id along the way.
enum GrpcTarget {
    Addr(SocketAddr),
    Listener(std::net::TcpListener),
}

impl GrpcTarget {
    fn into_std_listener(self) -> std::io::Result<std::net::TcpListener> {
        match self {
            GrpcTarget::Addr(addr) => std::net::TcpListener::bind(addr),
            GrpcTarget::Listener(listener) => Ok(listener),
        }
    }
}

impl std::fmt::Display for GrpcTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcTarget::Addr(addr) => write!(f, "{addr}"),
            GrpcTarget::Listener(listener) => match listener.local_addr() {
                Ok(addr) => write!(f, "pre-bound listener on {addr}"),
                Err(_) => write!(f, "pre-bound listener"),
            },
        }
    }
}

/// Ceiling on how long the serve future is given after the drain deadline has
/// ended every reply. Connections with nothing left in flight close in
/// milliseconds; this bounds a peer that ignores `GOAWAY`.
///
/// The wait is the drain timeout or this, whichever is shorter, so a caller
/// asking for a fast shutdown gets one: the timeout they set is a statement of
/// how long shutdown may take, and a constant added after it would answer a
/// question they did not ask.
const HARD_STOP_CEILING: Duration = Duration::from_secs(2);

/// gRPC transport adapter for the Toni framework.
///
/// Wraps `tonic::transport::Server`. Construction is contract-first: the
/// caller registers tonic-generated services via [`add_service`](Self::add_service)
/// before passing the adapter to `app.use_grpc_adapter()`. The framework
/// then calls `register_services()` to merge its discovered services into
/// the same route set; `into_lifecycle()` acquires the listening socket —
/// binding the configured address, or adopting the one passed to
/// [`from_listener`](Self::from_listener) — and drives the gRPC server
/// until shutdown.
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
    target: Option<GrpcTarget>,
    routes_builder: RoutesBuilder,
    drain_timeout: Option<Duration>,
    /// Global concurrency cap across all connections. `None` = unbounded.
    /// Implemented via [`tower::limit::GlobalConcurrencyLimitLayer`] + tonic's
    /// load-shed flag so excess requests reject with `ResourceExhausted`
    /// instead of queueing.
    max_inflight: Option<usize>,
    /// Per-connection concurrency cap. `None` = unbounded. Implemented via
    /// tonic's built-in `concurrency_limit_per_connection` + load-shed; lets
    /// one slow client monopolise its own connection without starving
    /// others even when the global cap isn't hit.
    max_per_connection: Option<usize>,
    /// Two consumers subscribe: tonic's `serve_with_incoming_shutdown` (so
    /// it begins natural drain), and the drain-timeout guard (so the deadline
    /// timer starts only *after* shutdown is signalled, not at startup).
    shutdown_tx: Arc<watch::Sender<bool>>,
}

impl GrpcAdapter {
    /// Listen on `addr`. Port 0 asks the OS for a free port; read the
    /// assigned address back from `BoundAdapters::grpc`.
    pub fn new(addr: SocketAddr) -> Self {
        Self::with_target(GrpcTarget::Addr(addr))
    }

    /// Serve on a socket the caller already bound and put into listening
    /// state, instead of binding one.
    ///
    /// The socket outlives the process that hands it over, which is the point:
    /// a supervisor holding it across restarts (systemd socket activation,
    /// `toni dev --listen`) leaves requests queued in the accept backlog
    /// rather than refused. Pair with the `listenfd` crate to claim an
    /// inherited descriptor.
    pub fn from_listener(listener: std::net::TcpListener) -> Self {
        Self::with_target(GrpcTarget::Listener(listener))
    }

    fn with_target(target: GrpcTarget) -> Self {
        let (tx, _) = watch::channel(false);
        Self {
            target: Some(target),
            routes_builder: RoutesBuilder::default(),
            drain_timeout: Some(DEFAULT_DRAIN_TIMEOUT),
            max_inflight: None,
            max_per_connection: None,
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
    ///
    /// Aborting them ends their replies with `UNAVAILABLE` and leaves the
    /// connections to close, which is bounded by this same duration (capped at
    /// two seconds), so `close()` returns within twice what is set here.
    pub fn with_drain_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.drain_timeout = timeout.into();
        self
    }

    /// Bound concurrent in-flight handlers across the whole server. When
    /// the cap is reached, additional requests are rejected with
    /// `Status::resource_exhausted` rather than queued, so a misbehaving
    /// client can't pin server memory by spawning unlimited calls. Pass
    /// `None` to remove the cap (default).
    ///
    /// Sibling of [`with_max_per_connection`](Self::with_max_per_connection),
    /// which bounds *per-connection* concurrency for the same reason at
    /// a different granularity — the two stack.
    pub fn with_max_inflight(mut self, max: impl Into<Option<usize>>) -> Self {
        self.max_inflight = max.into();
        self
    }

    /// Bound concurrent in-flight handlers *per connection*. When a
    /// single client opens many parallel streams, this prevents that
    /// client from monopolising the server even when the global cap
    /// isn't hit. Pass `None` to remove the cap (default).
    ///
    /// Sibling of [`with_max_inflight`](Self::with_max_inflight). Reaching
    /// either limit surfaces as `Status::resource_exhausted`.
    pub fn with_max_per_connection(mut self, max: impl Into<Option<usize>>) -> Self {
        self.max_per_connection = max.into();
        self
    }
}

#[async_trait]
impl toni::GrpcAdapter for GrpcAdapter {
    fn register_services(
        &mut self,
        services: Vec<(Arc<dyn GrpcServiceSource>, Arc<ResolvedGrpcEnhancers>)>,
    ) -> Result<()> {
        // Framework-discovered services (`#[controller]` + `#[grpc_methods]`)
        // each know how to wrap themselves in their tonic `*Server` — hand
        // them the same `RoutesBuilder` already accumulating any
        // user-`add_service`'d entries, plus the resolved enhancer bundle so
        // the macro-generated wrapper can fold it into per-call dispatch.
        for (svc, enhancers) in &services {
            svc.register_with(
                &mut self.routes_builder as &mut dyn std::any::Any,
                enhancers.clone(),
            );
        }
        Ok(())
    }

    async fn into_lifecycle(mut self: Box<Self>) -> Result<toni::GrpcLifecycleHandle> {
        // Bind synchronously so port-in-use surfaces as `Err` from
        // `app.bind()` instead of panicking inside the spawned serve loop.
        let target = self
            .target
            .take()
            .context("GrpcAdapter: into_lifecycle() called more than once")?;
        let described = target.to_string();
        let std_listener = target
            .into_std_listener()
            .with_context(|| format!("GrpcAdapter: failed to listen on {described}"))?;
        std_listener
            .set_nonblocking(true)
            .context("GrpcAdapter: failed to set listener nonblocking")?;
        let listener = TcpListener::from_std(std_listener)
            .context("GrpcAdapter: failed to register listener with the tokio runtime")?;
        let local_addr = listener
            .local_addr()
            .context("GrpcAdapter: failed to read local address from listener")?;

        let routes: Routes = std::mem::take(&mut self.routes_builder).routes();
        let drain_timeout = self.drain_timeout;
        let addr = local_addr;
        let max_inflight = self.max_inflight;
        let max_per_connection = self.max_per_connection;

        let shutdown_tonic = self.shutdown_tx.subscribe();
        let shutdown_drain = self.shutdown_tx.subscribe();

        // Flipped when the drain deadline elapses, which ends every reply still
        // being served. See `drain_layer` for why the connections cannot be
        // reached directly.
        let (deadline_tx, deadline_rx) = tokio::sync::watch::channel(false);

        let serve = Box::pin(async move {
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

            // Backpressure stack:
            //   - `GlobalConcurrencyLimitLayer` caps in-flight requests
            //     across all connections. `tower::util::option_layer`
            //     keeps the builder's type constant whether or not the
            //     cap is set (would otherwise vary with each `.layer()`
            //     call, breaking the conditional builder).
            //   - tonic's `concurrency_limit_per_connection` adds a
            //     per-connection cap (returns `Self`, so chainable).
            //   - `load_shed(true)` flips at-cap `NotReady` into an
            //     immediate `ResourceExhausted` reject; without it,
            //     callers would queue indefinitely and defeat the
            //     OOM-protection point.
            let mut builder = Server::builder()
                .layer(TracingLayer::new())
                .layer(DrainLayer::new(deadline_rx))
                .layer(MethodPathLayer::new())
                .layer(tower::util::option_layer(
                    max_inflight.map(tower::limit::GlobalConcurrencyLimitLayer::new),
                ));
            if let Some(n) = max_per_connection {
                builder = builder.concurrency_limit_per_connection(n);
            }
            if max_inflight.is_some() || max_per_connection.is_some() {
                builder = builder.load_shed(true);
            }

            let server_fut = builder
                .add_routes(routes)
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown_for_tonic);

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
                    // Ending the replies is the abort. Each closes with
                    // `UNAVAILABLE`, its connection is left with nothing in
                    // flight, and tonic's graceful shutdown closes it — so the
                    // serve future is awaited from here rather than dropped,
                    // and shutdown reports complete once it is.
                    let _ = deadline_tx.send(true);
                    let hard_stop = drain_timeout
                        .map_or(HARD_STOP_CEILING, |d| d.min(HARD_STOP_CEILING));
                    if let Ok(Err(e)) =
                        tokio::time::timeout(hard_stop, &mut server_fut).await
                    {
                        tracing::error!(?addr, error = %e, "GrpcAdapter serve error");
                    }
                }
            }
        });

        let shutdown_tx = self.shutdown_tx.clone();
        Ok(toni::GrpcLifecycleHandle::new(
            Some(local_addr),
            serve,
            move || async move {
                // Idempotent — the watch coalesces repeat sends into the same value.
                let _ = shutdown_tx.send(true);
                Ok(())
            },
        ))
    }
}

/// `watch::Receiver::wait_for` returns a `Ref<'_, bool>` guard that's
/// `!Send`; holding it across an `.await` would force the whole serve
/// future to be `!Send`. Discarding the guard inside this helper keeps the
/// outer scope's send-ness.
async fn wait_for_shutdown(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}
