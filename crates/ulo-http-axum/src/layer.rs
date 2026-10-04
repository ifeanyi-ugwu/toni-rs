//! The layer ahead of the app's service that puts what axum keeps elsewhere into the request's
//! extensions: a `ConnInfo` from `ConnectInfo<SocketAddr>` and an `embed::OriginalPath` from
//! `OriginalUri`. It also checks the host's `NestedPath` against `.nested_at` and logs a mismatch
//! at `warn` once.

use std::task::{Context, Poll};

/// The layer: `router.layer(ulo_http_axum::HostLayer)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostLayer;

impl<S> tower::Layer<S> for HostLayer {
    type Service = HostService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HostService { inner }
    }
}

/// The service [`HostLayer`] wraps around the app's.
#[derive(Clone, Debug)]
pub struct HostService<S> {
    pub(crate) inner: S,
}

impl<S, B> tower::Service<http::Request<B>> for HostService<S>
where
    S: tower::Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let _ = req;
        todo!()
    }
}
