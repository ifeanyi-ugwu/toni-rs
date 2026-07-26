use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::adapter::WsConnectionCallbacks;
use crate::adapter::adapter_context::AdapterContext;
use crate::adapter::lifecycle_handles::HttpLifecycleHandle;
use crate::adapter::request_handler::RequestHandler;
use crate::http_helpers::HttpMethod;

/// Implemented by every HTTP transport adapter (axum, actix, poem, rocket,
/// salvo). The framework calls [`register_route`](Self::register_route) and
/// [`register_ws_route`](Self::register_ws_route) during route resolution to register routes,
/// then calls [`into_lifecycle`](Self::into_lifecycle) once to consume the
/// adapter and produce a self-contained lifecycle handle.
///
/// Lifecycle methods (`listen` + `close`) used to live on this trait. They
/// were stripped: the framework's orchestrator never invoked them directly,
/// and keeping them on the public trait made every adapter crate carry
/// callback plumbing back from the lifecycle handle. The handle now owns
/// its own state and the trait surface is config-only.
///
/// # Bounded in-flight on HTTP
///
/// Notable absence vs. [`RpcAdapter`](crate::adapter::RpcAdapter) and
/// [`GrpcAdapter`](crate::adapter::GrpcAdapter): no `with_max_inflight`.
/// HTTP adapters wrap five different framework crates (axum, actix, poem,
/// rocket, salvo) each with their own middleware model, and the framework
/// already owns a cross-adapter abstraction that solves this without
/// touching the trait surface: the global middleware chain in
/// [`AdapterContext`](crate::adapter::AdapterContext) runs pre-routing on
/// every HTTP adapter.
///
/// Users wanting bounded concurrent in-flight on HTTP can:
///
/// 1. Add a semaphore-backed `Middleware` to the global chain via
///    `app.use_global_middleware(...)` — works uniformly on every HTTP
///    adapter without per-adapter wiring.
/// 2. Reach for the framework-native answer when an adapter exposes it
///    directly: a `tower::limit::GlobalConcurrencyLimitLayer` on axum/poem,
///    actix-web middleware, a Rocket fairing, etc.
/// 3. Place a reverse proxy (nginx, HAProxy) in front and let it handle
///    rate-limiting at the network layer.
///
/// Folding this knob into the trait would mean five framework-specific
/// integrations for one feature that the existing middleware path solves
/// once — out of scope for the adapter trait.
#[async_trait]
pub trait HttpAdapter: Send + Sync + 'static {
    /// Register one HTTP route with the adapter.
    ///
    /// Called at bootstrap for every route the framework discovers. The
    /// adapter stores the (method, path, handler) triple and uses it when
    /// building its native router.
    fn register_route(
        &mut self,
        method: HttpMethod,
        path: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<()>;

    /// Register a WebSocket upgrade path on the same port as HTTP.
    ///
    /// Default: returns an error — implement only if this adapter supports
    /// same-port WebSocket upgrades.
    fn register_ws_route(
        &mut self,
        path: &str,
        callbacks: Arc<WsConnectionCallbacks>,
    ) -> Result<()> {
        let _ = (path, callbacks);
        Err(anyhow::anyhow!(
            "This HTTP adapter does not support WebSocket upgrades"
        ))
    }

    /// Consume the adapter, bind the listening socket, and return a fully
    /// self-contained [`HttpLifecycleHandle`] the orchestrator can drive.
    ///
    /// The implementation typically:
    /// 1. Builds its framework-native router from the routes accumulated
    ///    in `register_route` / `register_ws_route`.
    /// 2. Binds the listener synchronously so port-in-use surfaces here
    ///    rather than inside the spawned serve task.
    /// 3. Captures its shutdown signal in a closure and hands the
    ///    `local_addr`, the serve future, and the closure to
    ///    [`HttpLifecycleHandle::new`].
    ///
    /// `ctx` carries the global middleware chain and other adapter-shared
    /// runtime context. The adapter must anchor [`AdapterContext::execute`]
    /// at its outermost point, wrapping the entire native router — not
    /// inside matched-route handlers. The chain then observes every inbound
    /// request (matched, unknown path, method mismatch, WebSocket upgrade),
    /// may short-circuit with a response, and the request it forwards is
    /// the one the router matches on. `global_chain_conformance.rs` in the
    /// integration suite pins these behaviors per adapter.
    async fn into_lifecycle(
        self: Box<Self>,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle>;
}
