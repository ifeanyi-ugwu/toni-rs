//! `tower::Layer` that wraps every gRPC call in an `rpc.request` span.
//!
//! Mirrors the per-request span shape installed on the TCP and UDP RPC
//! adapters so an operator's tracing subscriber sees the same field set
//! across every transport: `transport`, `pattern`, `id`, `peer`. For gRPC,
//! `pattern` is the proto method path (e.g. `"toni_test.orders.Orders/Create"`)
//! and `id` is the `grpc-trace-bin` / `x-request-id` header when present
//! (tonic doesn't surface a built-in correlation id, so any header the
//! caller chose to send is the best we have).
//!
//! Installed automatically by `GrpcAdapter::serve` via `Server::builder().layer(…)`.
//! Not configurable for v1 — spans are emitted unconditionally and surface
//! only when the application installs a tracing subscriber.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::Request;
use tonic::body::Body as TonicBody;
use tower::{Layer, Service};
use tracing::Instrument;

#[derive(Clone, Default)]
pub struct TracingLayer;

impl TracingLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for TracingLayer {
    type Service = TracingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TracingService { inner }
    }
}

#[derive(Clone)]
pub struct TracingService<S> {
    inner: S,
}

impl<S, ResBody> Service<Request<TonicBody>> for TracingService<S>
where
    S: Service<Request<TonicBody>, Response = http::Response<ResBody>> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = http::Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<TonicBody>) -> Self::Future {
        // gRPC method paths are `/package.Service/Method`. Splitting on `/`
        // and rejoining gives a more readable `package.Service/Method` —
        // matches how tonic logs the path everywhere else.
        let pattern = req.uri().path().trim_start_matches('/').to_string();

        // Best-effort correlation id — gRPC has no native request id, so
        // we look for common metadata keys callers use.
        let id = req
            .headers()
            .get("x-request-id")
            .or_else(|| req.headers().get("grpc-trace-id"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Peer address is set as a request extension by tonic's
        // `ConnectInfoLayer`; absent on direct unix-socket transports etc.
        let peer = req
            .extensions()
            .get::<tonic::transport::server::TcpConnectInfo>()
            .and_then(|ci| ci.remote_addr())
            .map(|a| a.to_string());

        let span = tracing::info_span!(
            "rpc.request",
            transport = "grpc",
            pattern = %pattern,
            id = ?id,
            peer = peer.as_deref().unwrap_or(""),
        );

        let fut = self.inner.call(req);
        Box::pin(fut.instrument(span))
    }
}
