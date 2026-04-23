use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;

use crate::adapter::WsConnectionCallbacks;
use crate::http_helpers::HttpMethod;

use crate::adapter::request_handler::RequestHandler;

pub trait HttpAdapter: Send + Sync + 'static {
    /// Register one HTTP route with the adapter.
    ///
    /// Called at bootstrap for every route the framework discovers.  The
    /// adapter stores the (method, path, handler) triple and uses it in
    /// `route_handler` — either by feeding a `RouteTableBuilder` or its own
    /// native router.
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

    /// Seal the route configuration and return the adapter's routing handler.
    ///
    /// Called once after all `bind` and `bind_ws` calls, before `create`.
    /// The framework wraps the returned handler with the global middleware
    /// chain and passes the result back to `create` as the request entry
    /// point.
    fn route_handler(&mut self) -> Arc<dyn RequestHandler>;

    /// Start the server and return the accept-loop future.
    ///
    /// `handler` is the framework-provided entry point for every incoming
    /// HTTP request — it runs global middleware then delegates to the
    /// routing handler returned by `route_handler`.  The adapter must call
    /// `handler.handle(req)` for every request, including those that would
    /// produce a 404.
    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

    fn close(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
}

pub(crate) trait ErasedHttpAdapter: Send + Sync {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()>;
    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()>;
    fn route_handler(&mut self) -> Arc<dyn RequestHandler>;
    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        handler: Arc<dyn RequestHandler>,
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

    fn route_handler(&mut self) -> Arc<dyn RequestHandler> {
        HttpAdapter::route_handler(self)
    }

    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        handler: Arc<dyn RequestHandler>,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        HttpAdapter::create(self, port, hostname, handler)
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(HttpAdapter::close(self))
    }
}
