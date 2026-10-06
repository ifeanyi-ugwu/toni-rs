//! The `poem::Endpoint` over the app: builds the app's `Request` from poem's, with the peer and the
//! upgrade `take_upgrade` yields, and answers through `Service::respond`.

use std::io;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use ulo::BoxError;
use ulo_http::{ConnInfo, HttpBody, OnUpgrade, Request, Upgraded};

use crate::Handle;

/// The endpoint a poem route mounts, from [`endpoint`].
#[derive(Clone)]
pub struct PoemEndpoint {
    pub(crate) handle: Handle,
}

/// The endpoint over `handle`, for `Route::nest(..)`, which strips the prefix, or as the fallback.
pub fn endpoint(handle: &Handle) -> PoemEndpoint {
    PoemEndpoint { handle: handle.clone() }
}

impl poem::Endpoint for PoemEndpoint {
    type Output = poem::Response;

    async fn call(&self, mut req: poem::Request) -> poem::Result<Self::Output> {
        // `take_upgrade` answers `Err` for a request asking for no upgrade.
        let upgrade = req.take_upgrade().ok().map(|pending| {
            OnUpgrade::new(async move {
                let upgraded = pending.await.map_err(|error| BoxError::from(error.to_string()))?;
                Ok::<_, BoxError>(Upgraded::new(upgraded))
            })
        });
        let body: BoxBody<Bytes, io::Error> = req.take_body().into();
        let (mut head, ()) = http::Request::new(()).into_parts();
        head.method = req.method().clone();
        head.uri = req.uri().clone();
        head.version = req.version();
        head.headers = req.headers().clone();
        head.extensions = req.extensions().clone();
        let conn = ConnInfo::new(req.version());
        let conn = match req.remote_addr().as_socket_addr() {
            Some(peer) => conn.peer(*peer),
            None => conn,
        };
        let conn = match req.local_addr().as_socket_addr() {
            Some(local) => conn.local(*local),
            None => conn,
        };
        let app_req = Request { head, body: HttpBody::new(body), conn, upgrade };
        let answer = self.handle.service().respond(&req, app_req);
        let response = answer.await;
        Ok(poem::Response::from(response.map(|body| SyncBody(Mutex::new(body)))))
    }
}

/// The app's body behind a mutex, which poem's boxed body requires to be `Sync`, its error as
/// poem's `io::Error`. The body is only ever polled through `&mut`, so the lock is taken
/// uncontended, for the size hint alone.
struct SyncBody(Mutex<HttpBody>);

impl http_body::Body for SyncBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        let body = self.get_mut().0.get_mut().unwrap_or_else(PoisonError::into_inner);
        Pin::new(body).poll_frame(cx).map(|frame| frame.map(|frame| frame.map_err(io::Error::other)))
    }

    fn is_end_stream(&self) -> bool {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).size_hint()
    }
}
