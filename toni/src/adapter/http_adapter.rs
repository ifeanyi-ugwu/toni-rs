use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::adapter::WsConnectionCallbacks;
use crate::adapter::adapter_context::AdapterContext;
use crate::adapter::lifecycle_handles::HttpLifecycleHandle;
use crate::adapter::request_handler::RequestHandler;
use crate::http_helpers::HttpMethod;

/// Implemented by every HTTP transport adapter (axum, actix, poem, rocket,
/// salvo). The framework calls [`bind`](Self::bind) and
/// [`bind_ws`](Self::bind_ws) during route resolution to register routes,
/// then calls [`into_lifecycle`](Self::into_lifecycle) once to consume the
/// adapter and produce a self-contained lifecycle handle.
///
/// Lifecycle methods (`listen` + `close`) used to live on this trait. They
/// were stripped: the framework's orchestrator never invoked them directly,
/// and keeping them on the public trait made every adapter crate carry
/// callback plumbing back from the lifecycle handle. The handle now owns
/// its own state and the trait surface is config-only.
#[async_trait]
pub trait HttpAdapter: Send + Sync + 'static {
    /// Register one HTTP route with the adapter.
    ///
    /// Called at bootstrap for every route the framework discovers. The
    /// adapter stores the (method, path, handler) triple and uses it when
    /// building its native router.
    fn bind(
        &mut self,
        method: HttpMethod,
        path: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<()>;

    /// Register a WebSocket upgrade path on the same port as HTTP.
    ///
    /// Default: returns an error — implement only if this adapter supports
    /// same-port WebSocket upgrades.
    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
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
    ///    in `bind` / `bind_ws`.
    /// 2. Binds the listener synchronously so port-in-use surfaces here
    ///    rather than inside the spawned serve task.
    /// 3. Captures its shutdown signal in a closure and hands the
    ///    `local_addr`, the serve future, and the closure to
    ///    [`HttpLifecycleHandle::new`].
    ///
    /// `ctx` carries the global middleware chain and other adapter-shared
    /// runtime context. The adapter is responsible for composing
    /// `ctx.global_chain` around its own routing handler so global
    /// middleware runs pre-routing.
    async fn into_lifecycle(
        self: Box<Self>,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<HttpLifecycleHandle>;
}
