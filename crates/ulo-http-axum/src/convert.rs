//! The conversion at axum's edge: an axum request into a `ulo_http::Request`, the response back.

use std::net::SocketAddr;

use axum::extract::Request as AxumRequest;
use axum::response::Response as AxumResponse;
use ulo_http::{Request, Response};

/// The request as `ulo-http` reads it: parts, body, the connection (`peer` from the listener's
/// connect info, `tls` from the handshake) and the upgrade future from `hyper::upgrade::on`.
pub(crate) fn request(req: AxumRequest, peer: Option<SocketAddr>, local: Option<SocketAddr>) -> Request {
    let _ = (req, peer, local);
    todo!("split the request; `HttpBody::new(body)`; `ConnInfo::new(version)`; `OnUpgrade::new` over `hyper::upgrade::on` mapped through `TokioIo` into `Upgraded::new`")
}

/// The response as axum writes it.
pub(crate) fn response(res: Response) -> AxumResponse {
    let _ = res;
    todo!("map the body into `axum::body::Body::new`")
}
