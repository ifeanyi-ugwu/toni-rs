//! The conversion at hyper's edge: a hyper request into a `ulo_http::Request`, the response back.

use hyper::body::Incoming;
use ulo::BoxError;
use ulo_http::{ConnInfo, HttpBody, OnUpgrade, Request, Upgraded};
use ulo_hyper_serve::FuturesIo;

/// The request as `ulo-http` reads it: parts, body, the connection as the accept loop reported
/// it with the request's own HTTP version, and the upgrade future hyper stores on a request that
/// asks for one (an HTTP/1.1 `Upgrade`), `None` on any other.
pub(crate) fn request(req: http::Request<Incoming>, conn: &ConnInfo) -> Request {
    let (mut head, body) = req.into_parts();
    let upgrade = head.extensions.remove::<hyper::upgrade::OnUpgrade>().map(|pending| {
        OnUpgrade::new(async move {
            let upgraded = pending.await?;
            Ok::<_, BoxError>(Upgraded::from_futures(FuturesIo::new(upgraded)))
        })
    });
    let mut info = conn.clone();
    info.version = head.version;
    Request { head, body: HttpBody::new(body), conn: info, upgrade }
}
