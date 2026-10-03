//! Composition with tower layers inside the pre-dispatch stage (transports DESIGN §3.4).
//!
//! Layers are composed once, in `prepare`, and run inside the stage on every backend, actix and
//! rocket included, which are not tower-based. A layered service built once cannot capture a
//! request's state, so the request carries the rest of its chain in its extensions, and
//! [`Service`] takes it from there.

use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use ulo::{BoxError, BoxFuture};

use crate::body::HttpBody;

/// The service a pre-dispatch tower layer wraps: the rest of the stage, then routing or dispatch.
/// `.layer(L)` accepts any `tower::Layer<ulo_http::Service>`.
///
/// It accepts a request with any `Bytes` body, so a layer that wraps the request body, such as
/// `RequestBodyLimitLayer`, composes; the body is boxed back into an `HttpBody` before the stage
/// continues. It never fails: the rest of the chain always answers with a response, an error
/// rendered as one.
#[derive(Clone, Default)]
pub struct Service {
    _private: (),
}

impl<B> tower::Service<http::Request<B>> for Service
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    type Response = http::Response<HttpBody>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<http::Response<HttpBody>, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let _ = req;
        todo!("take the request's continuation from its extensions and run it with the body boxed")
    }
}

/// A layered service as the stage stores it: called with a request whose extensions carry its
/// continuation.
pub(crate) type LayeredService =
    Arc<dyn Fn(http::Request<HttpBody>) -> BoxFuture<'static, Result<http::Response<HttpBody>, BoxError>> + Send + Sync>;

/// A tower layer, erased.
pub(crate) trait ErasedLayer: Send + Sync + 'static {
    /// `inner` wrapped in this layer.
    fn layer(&self, inner: Service) -> LayeredService;
}

/// `L` as an `ErasedLayer`.
pub(crate) struct LayerOf<L>(pub(crate) L);

impl<L, S, B> ErasedLayer for LayerOf<L>
where
    L: tower::Layer<Service, Service = S> + Send + Sync + 'static,
    S: tower::Service<http::Request<HttpBody>, Response = http::Response<B>> + Clone + Send + Sync + 'static,
    S::Error: Into<BoxError>,
    S::Future: Send + 'static,
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    fn layer(&self, inner: Service) -> LayeredService {
        let _ = inner;
        todo!("`self.0.layer(inner)`, called per request on a clone after `poll_ready`, the response body boxed")
    }
}
