//! Puts the method path a call arrived on onto its request.
//!
//! `/package.Service/Method` exists on the wire and nowhere in the impl block
//! `#[grpc_methods]` reads: a trait's Rust name carries no package, and a
//! method's Rust name takes the route's casing by convention that
//! `tonic_build::manual` does not have to follow. tonic inserts its own
//! `GrpcMethod` extension on the client side only.
//!
//! Cheap by construction — the request is annotated and the inner future
//! returned as it is, so this adds no allocation per call.

use std::task::{Context, Poll};

use http::Request;
use toni::adapter::GrpcMethodPath;
use tonic::body::Body as TonicBody;
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub struct MethodPathLayer;

impl MethodPathLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for MethodPathLayer {
    type Service = MethodPathService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MethodPathService { inner }
    }
}

#[derive(Clone)]
pub struct MethodPathService<S> {
    inner: S,
}

impl<S> Service<Request<TonicBody>> for MethodPathService<S>
where
    S: Service<Request<TonicBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<TonicBody>) -> Self::Future {
        // gRPC paths are `/package.Service/Method`; the leading slash is
        // dropped so the value reads as the method path is written everywhere
        // else — in a proto, in a log line, in tonic's own span.
        let path = req.uri().path().trim_start_matches('/').to_string();
        req.extensions_mut().insert(GrpcMethodPath(path));
        self.inner.call(req)
    }
}
