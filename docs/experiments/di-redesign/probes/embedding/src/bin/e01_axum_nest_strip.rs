//! What a tower service nested in an axum router receives: the stripped URI, `OriginalUri` and
//! `NestedPath` in the request's extensions; what a fallback service receives: the full URI and
//! `OriginalUri`, no `NestedPath`. Run, not only compiled: `Router` answers `oneshot` with no socket.

use std::convert::Infallible;

use axum::Router;
use axum::body::Body;
use axum::extract::{NestedPath, OriginalUri, Request};
use axum::response::Response;
use tower::ServiceExt;
use tower::service_fn;

async fn describe(req: Request) -> Result<Response, Infallible> {
    let original = req.extensions().get::<OriginalUri>().map(|o| o.0.to_string());
    let nested = req.extensions().get::<NestedPath>().map(|n| n.as_str().to_owned());
    let connect = req.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().is_some();
    let text = format!("uri={} original={original:?} nested={nested:?} connect_info={connect}", req.uri());
    Ok(Response::new(Body::from(text)))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let app = Router::new()
        .nest_service("/api", service_fn(describe))
        .fallback_service(service_fn(describe));
    for path in ["/api/users/1?x=1", "/api", "/other/path"] {
        let req = Request::builder().uri(path).body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let body = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
        println!("{path} -> {}", String::from_utf8_lossy(&body));
    }
}
