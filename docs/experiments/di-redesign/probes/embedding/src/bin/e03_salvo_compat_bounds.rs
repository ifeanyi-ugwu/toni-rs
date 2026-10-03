//! The same tower shape passes salvo's `TowerServiceCompat::compat` bounds (with `ReqBody` as the
//! request body) and mounts on a `Router` path with a rest segment. Compile-level: salvo's bridge
//! is `salvo::prelude`-reachable only through `salvo_extra`, re-exported as `salvo::tower_compat`.

use std::convert::Infallible;
use std::task::{Context, Poll};

use futures_util::future::BoxFuture;
use salvo::http::ReqBody;
use salvo::prelude::*;
use salvo::prelude::TowerServiceCompat;

#[derive(Clone)]
struct Describe;

impl tower::Service<hyper::Request<ReqBody>> for Describe {
    type Response = hyper::Response<http_body_util::Full<bytes::Bytes>>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: hyper::Request<ReqBody>) -> Self::Future {
        let upgrade = req.extensions().get::<hyper::upgrade::OnUpgrade>().is_some();
        let text = format!("uri={} on_upgrade_ext={upgrade}", req.uri());
        Box::pin(async move { Ok(hyper::Response::new(http_body_util::Full::new(bytes::Bytes::from(text)))) })
    }
}

fn main() {
    let _router = Router::new().push(Router::with_path("api/{**rest}").goal(Describe.compat()));
    println!("salvo tower compat bounds hold for a ulo-shaped service");
}
