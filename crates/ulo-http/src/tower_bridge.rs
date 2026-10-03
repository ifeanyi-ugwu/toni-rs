//! Composition with tower layers inside the pre-dispatch stage (transports DESIGN §3.4).
//!
//! Layers are composed once, in `prepare`, and run inside the stage on every backend, actix and
//! rocket included, which are not tower-based. A layered service built once cannot capture a
//! request's state, so the request carries the rest of its chain in its extensions, and
//! [`Service`] takes it from there.

use std::convert::Infallible;
use std::future::poll_fn;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use http::StatusCode;
use ulo::{BoxError, BoxFuture};

use crate::body::HttpBody;
use crate::response::Response;

/// The service a pre-dispatch tower layer wraps: the rest of the stage, then routing or dispatch.
/// `.layer(L)` accepts any `tower::Layer<ulo_http::Service>`.
///
/// It accepts a request with any `Bytes` body, so a layer that wraps the request body, such as
/// `RequestBodyLimitLayer`, composes; the body is boxed back into an `HttpBody` before the stage
/// continues. It never fails: the rest of the chain always answers with a response, an error
/// rendered as one.
///
/// The rest of the chain runs once per request. A layer that calls its inner service a second
/// time for one request, a retry for example, or that builds a new request without the original's
/// extensions, gets a 500 response from it.
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
        let (mut head, body) = req.into_parts();
        let rest = head.extensions.remove::<Continuation>().and_then(|continuation| continuation.take());
        let req = http::Request::from_parts(head, HttpBody::new(body));
        Box::pin(async move {
            match rest {
                Some(rest) => Ok::<_, Infallible>(rest(req).await),
                None => {
                    tracing::error!(
                        "a pre-dispatch tower layer called its inner service without the request's continuation: \
                         a second call for one request, or a request built without the original's extensions"
                    );
                    let mut response = Response::new(HttpBody::empty());
                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    Ok(response)
                }
            }
        })
    }
}

/// What runs after a layer: the rest of the pre-dispatch chain, then routing or dispatch.
type Rest = Box<dyn FnOnce(http::Request<HttpBody>) -> BoxFuture<'static, Response> + Send>;

/// The request's continuation, as it travels through a layer in the request's extensions.
///
/// `http::Extensions` holds only `Clone` values, and a layer may clone a request's extensions, so
/// every clone shares the one slot and the first [`Service`] call to reach it takes it.
#[derive(Clone)]
pub(crate) struct Continuation {
    slot: Arc<Mutex<Option<Rest>>>,
}

impl Continuation {
    pub(crate) fn new(rest: impl FnOnce(http::Request<HttpBody>) -> BoxFuture<'static, Response> + Send + 'static) -> Self {
        Continuation { slot: Arc::new(Mutex::new(Some(Box::new(rest)))) }
    }

    fn take(&self) -> Option<Rest> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).take()
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
        let service = self.0.layer(inner);
        // `tower::Service::call` takes `&mut self` and readiness belongs to one caller, so each
        // request drives its own clone, as tower's `oneshot` does.
        Arc::new(move |req: http::Request<HttpBody>| -> BoxFuture<'static, Result<http::Response<HttpBody>, BoxError>> {
            let mut service = service.clone();
            Box::pin(async move {
                poll_fn(|cx| <S as tower::Service<http::Request<HttpBody>>>::poll_ready(&mut service, cx))
                    .await
                    .map_err(Into::<BoxError>::into)?;
                let response = <S as tower::Service<http::Request<HttpBody>>>::call(&mut service, req)
                    .await
                    .map_err(Into::<BoxError>::into)?;
                Ok::<_, BoxError>(response.map(HttpBody::new))
            })
        })
    }
}
