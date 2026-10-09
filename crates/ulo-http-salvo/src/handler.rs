//! The `salvo::Handler` over the app: builds the app's `Request` from salvo's, with the peer and the
//! upgrade future, and answers through `Service::respond`, which strips the prefix salvo leaves on
//! the path.

use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use hyper_util::rt::TokioIo;
use salvo::http::ResBody;
use ulo::BoxError;
use ulo_http::{ConnInfo, HttpBody, OnUpgrade, Request, Response, Upgraded};

use crate::{Handle, SalvoRequest};

/// The handler a salvo router mounts, from [`handler`].
#[derive(Clone)]
pub struct SalvoHandler {
    pub(crate) handle: Handle,
}

/// The handler over `handle`, for `Router::goal(..)`: `Router::with_path("api/{**rest}")` nested,
/// `Router::with_path("{**rest}")` as the catch-all.
pub fn handler(handle: &Handle) -> SalvoHandler {
    SalvoHandler { handle: handle.clone() }
}

#[salvo::async_trait]
impl salvo::Handler for SalvoHandler {
    async fn handle(
        &self,
        req: &mut salvo::Request,
        depot: &mut salvo::Depot,
        res: &mut salvo::Response,
        _ctrl: &mut salvo::FlowCtrl,
    ) {
        // Taken rather than copied: the upgrade future is the app's to drive once it answers 101.
        let upgrade = req.extensions_mut().remove::<hyper::upgrade::OnUpgrade>().map(|pending| {
            OnUpgrade::new(async move {
                let upgraded = pending.await?;
                Ok::<_, BoxError>(Upgraded::from_tokio(TokioIo::new(upgraded)))
            })
        });
        let body = HttpBody::new(req.take_body());
        let (mut head, ()) = http::Request::new(()).into_parts();
        head.method = req.method().clone();
        head.uri = req.uri().clone();
        head.version = req.version();
        head.headers = req.headers().clone();
        head.extensions = req.extensions().clone();
        let conn = ConnInfo::new(req.version());
        let conn = match req.remote_addr().clone().into_std() {
            Some(peer) => conn.peer(peer),
            None => conn,
        };
        let conn = match req.local_addr().clone().into_std() {
            Some(local) => conn.local(local),
            None => conn,
        };
        let app_req = Request { head, body, conn, upgrade };
        // `respond` reads the host's request only before it returns, so the borrow ends here and
        // the future awaited below holds none.
        let answer = {
            let host = SalvoRequest { request: req, depot };
            self.handle.service().respond(&host, app_req)
        };
        write(res, answer.await);
    }
}

/// The app's response into salvo's: status, headers over any a hoop set, extensions with the
/// app's `Routing` among them, and the body as it streams.
fn write(res: &mut salvo::Response, response: Response) {
    let (parts, body) = response.into_parts();
    res.status_code(parts.status);
    res.headers.extend(parts.headers);
    res.extensions.extend(parts.extensions);
    // Always a body, an empty one included: salvo hands an error status with no body to its
    // catcher, which would replace the app's problem document.
    res.body = ResBody::Boxed(Box::pin(SyncBody(Mutex::new(body))));
}

/// The app's body behind a mutex, which salvo's boxed body requires to be `Sync`. The body is only
/// ever polled through `&mut`, so the lock is taken uncontended, for the size hint alone.
struct SyncBody(Mutex<HttpBody>);

impl http_body::Body for SyncBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let body = self.get_mut().0.get_mut().unwrap_or_else(PoisonError::into_inner);
        Pin::new(body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).size_hint()
    }
}
