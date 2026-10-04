//! The conversion at axum's edge: an axum request into a `ulo_http::Request`, the response back.

use std::net::SocketAddr;

use axum::extract::Request as AxumRequest;
use axum::response::Response as AxumResponse;
use hyper_util::rt::TokioIo;
use ulo::BoxError;
use ulo_http::{ConnInfo, HttpBody, OnUpgrade, Request, Response, TlsInfo, Upgraded};

/// The connection every request on it reports.
pub(crate) struct Conn {
    pub(crate) peer: SocketAddr,
    pub(crate) local: Option<SocketAddr>,
    pub(crate) tls: Option<TlsInfo>,
}

/// The request as `ulo-http` reads it: parts, body, the connection, and the upgrade future hyper
/// stores on a request that asks for one (an HTTP/1.1 `Upgrade`), `None` on any other.
pub(crate) fn request(req: AxumRequest, conn: &Conn) -> Request {
    let (mut head, body) = req.into_parts();
    let upgrade = head.extensions.remove::<hyper::upgrade::OnUpgrade>().map(|pending| {
        OnUpgrade::new(async move {
            let upgraded = pending.await?;
            Ok::<_, BoxError>(Upgraded::new(TokioIo::new(upgraded)))
        })
    });
    let mut info = ConnInfo::new(head.version).peer(conn.peer);
    if let Some(local) = conn.local {
        info = info.local(local);
    }
    if let Some(tls) = &conn.tls {
        info = info.tls(tls.clone());
    }
    Request { head, body: HttpBody::new(body), conn: info, upgrade }
}

/// The response as axum writes it.
pub(crate) fn response(res: Response) -> AxumResponse {
    res.map(axum::body::Body::new)
}
