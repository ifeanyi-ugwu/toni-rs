use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;

use crate::adapter::WsConnectionCallbacks;
use crate::router::RequestDispatcher;

pub trait HttpAdapter: Send + Sync + 'static {
    /// Hand the framework's request dispatcher to the adapter.
    ///
    /// Called once, after all routes have been registered with the framework
    /// router, before [`create`] is called. The adapter must call
    /// `dispatcher.dispatch(req)` for every incoming HTTP request — including
    /// those that match no registered route — so that global middleware runs
    /// on all traffic.
    ///
    /// [`create`]: HttpAdapter::create
    fn set_dispatcher(&mut self, dispatcher: Arc<RequestDispatcher>);

    /// Register a WebSocket upgrade path on the same port as HTTP.
    ///
    /// **Default:** returns error — implement to support WebSocket upgrades on this adapter.
    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        let _ = (path, callbacks);
        Err(anyhow::anyhow!(
            "This HTTP adapter does not support WebSocket upgrades"
        ))
    }

    /// Seal configuration and return the running server future.
    ///
    /// Called once after `set_dispatcher` and all `bind_ws` calls. The returned
    /// future is the accept loop — the framework joins it alongside any WS/RPC
    /// server futures so no top-level spawn is needed in the adapter.
    fn create(
        &mut self,
        port: u16,
        hostname: &str,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

    fn close(&mut self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
}

pub(crate) trait ErasedHttpAdapter: Send + Sync {
    fn set_dispatcher(&mut self, dispatcher: Arc<RequestDispatcher>);
    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()>;
    fn create(
        &mut self,
        port: u16,
        hostname: &str,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;
    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
}

impl<A: HttpAdapter + 'static> ErasedHttpAdapter for A {
    fn set_dispatcher(&mut self, dispatcher: Arc<RequestDispatcher>) {
        HttpAdapter::set_dispatcher(self, dispatcher);
    }

    fn bind_ws(&mut self, path: &str, callbacks: Arc<WsConnectionCallbacks>) -> Result<()> {
        HttpAdapter::bind_ws(self, path, callbacks)
    }

    fn create(
        &mut self,
        port: u16,
        hostname: &str,
    ) -> Result<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        HttpAdapter::create(self, port, hostname)
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(HttpAdapter::close(self))
    }
}
