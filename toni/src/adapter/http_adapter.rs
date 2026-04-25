use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;

use crate::adapter::WsConnectionCallbacks;
use crate::adapter::request_handler::RequestHandler;
use crate::adapter::server_handle::ServerHandle;
use crate::http_helpers::HttpMethod;

use crate::adapter::adapter_context::AdapterContext;

pub trait HttpAdapter: Send + Sync + 'static {
    /// Register one HTTP route with the adapter.
    ///
    /// Called at bootstrap for every route the framework discovers.  The
    /// adapter stores the (method, path, handler) triple and uses it when
    /// building its native router.
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()>;

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

    fn close(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
}

pub(crate) trait ErasedHttpAdapter: Send + Sync {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()>;
    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()>;
    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Pin<Box<dyn Future<Output = Result<ServerHandle>> + Send + 'static>>;
    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

impl<A: HttpAdapter + 'static> ErasedHttpAdapter for A {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()> {
        HttpAdapter::bind(self, method, path, handler)
    }

    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        HttpAdapter::bind_ws(self, path, callbacks)
    }

    fn listen(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Pin<Box<dyn Future<Output = Result<ServerHandle>> + Send + 'static>> {
        HttpAdapter::listen(self, port, hostname, ctx)
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(HttpAdapter::close(self))
    }
}
