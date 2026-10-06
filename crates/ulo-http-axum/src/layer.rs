//! The layer ahead of the app's service that puts what axum keeps elsewhere into the request's
//! extensions: a `ConnInfo` from `ConnectInfo<SocketAddr>` and an `embed::OriginalPath` from
//! `OriginalUri`.

use std::net::SocketAddr;
use std::task::{Context, Poll};

use axum::extract::{ConnectInfo, OriginalUri};
use ulo_http::ConnInfo;
use ulo_http::embed::OriginalPath;

/// The layer, wrapped around the app's service where the host mounts it:
/// `router.nest_service("/api", HostLayer.layer(embedded.service()))`, or around
/// `.fallback_service(..)`'s argument.
///
/// It writes host extensions, which the axum embedding reads as it reads any other
/// (`host_extensions: true`): a `ConnInfo` carrying the peer address when the router is served
/// through `into_make_service_with_connect_info::<SocketAddr>()`, which `.peer_addr(true)` relies
/// on, and the path the client sent, before `nest_service` stripped the prefix, as
/// `OriginalPath`. A `ConnInfo` an earlier layer inserted is kept.
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

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        let version = req.version();
        let extensions = req.extensions_mut();
        if extensions.get::<ConnInfo>().is_none() {
            let peer = extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
            let conn = ConnInfo::new(version);
            extensions.insert(match peer {
                Some(peer) => conn.peer(peer),
                None => conn,
            });
        }
        let original = extensions.get::<OriginalUri>().map(|OriginalUri(uri)| OriginalPath::from(uri));
        if let Some(original) = original {
            extensions.insert(original);
        }
        self.inner.call(req)
    }
}
