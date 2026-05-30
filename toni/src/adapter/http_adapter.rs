use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;

use crate::adapter::WsConnectionCallbacks;
use crate::adapter::request_handler::RequestHandler;
use crate::adapter::server_handle::ServerHandle;
use crate::http_helpers::HttpMethod;

use crate::adapter::adapter_context::AdapterContext;

#[async_trait]
pub trait HttpAdapter: Send + Sync + 'static {
    /// Register one HTTP route with the adapter.
    ///
    /// Called at bootstrap for every route the framework discovers.  The
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

    /// Bind the listening socket and return a handle to the running server.
    ///
    /// Called once after all `bind` and `bind_ws` calls. The returned future
    /// resolves once the socket is bound — `handle.local_addr` reflects the
    /// actual bound address (useful when `port` is 0). Awaiting `handle.serve`
    /// runs the accept loop.
    ///
    /// `ctx` carries the global middleware chain and future runtime context.
    /// The adapter is responsible for composing `ctx.global_chain` around its
    /// own routing handler so that global middleware runs pre-routing.
    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Pin<Box<dyn Future<Output = Result<ServerHandle>> + Send + 'static>>;

    /// Trigger adapter shutdown. Default: no-op. Adapters that hold
    /// background resources (sockets, channels, in-flight streams)
    /// override to release them.
    ///
    /// `async_trait`'d so the trait stays object-safe — that's what lets
    /// `ToniApplication` store the adapter as `Box<dyn HttpAdapter>`
    /// without a parallel erased shim.
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}
