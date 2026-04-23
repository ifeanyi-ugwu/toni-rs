use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;

use crate::adapter::WsConnectionCallbacks;
use crate::adapter::request_handler::RequestHandler;
use crate::http_helpers::HttpMethod;

use crate::adapter::adapter_context::AdapterContext;

pub trait HttpAdapter: Send + Sync + 'static {
    /// Register one HTTP route with the adapter.
    ///
    /// Called at bootstrap for every route the framework discovers.  The
    /// adapter stores the (method, path, handler) triple and uses it when
    /// building its router — either via `RouteTableBuilder` or its own
    /// native routing mechanism.
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

    /// Seal configuration and start the server.
    ///
    /// Called once after all `bind` and `bind_ws` calls.  `ctx` carries
    /// everything the framework provides at serve time — currently the global
    /// middleware chain (run before the adapter's routing on every request,
    /// including 404s) and future runtime context as the framework grows.
    ///
    /// The adapter is responsible for composing `ctx.global_chain` around its
    /// own routing handler so that global middleware runs pre-routing.
    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

    fn close(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
}

pub(crate) trait ErasedHttpAdapter: Send + Sync {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()>;
    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()>;
    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;
    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

impl<A: HttpAdapter + 'static> ErasedHttpAdapter for A {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()> {
        HttpAdapter::bind(self, method, path, handler)
    }

    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        HttpAdapter::bind_ws(self, path, callbacks)
    }

    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        HttpAdapter::create(self, port, hostname, ctx)
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(HttpAdapter::close(self))
    }
}
