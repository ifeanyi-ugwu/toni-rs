//! The payload pump: actix's `!Send` request payload forwarded chunk by chunk through a bounded
//! channel from the worker-local task, so the app reads it as a `Send` body. The response body
//! needs no pump: actix polls it on the worker, which a `Send` body allows, and drops it at the
//! next write that fails, which is where a disconnect is observed.

use std::io;
use std::pin::{Pin, pin};
use std::task::{Context, Poll, ready};

use actix_web::body::{BodySize, MessageBody};
use actix_web::dev::Payload;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use http_body::Body as _;
use tokio::sync::mpsc;
use ulo::BoxError;
use ulo_http::HttpBody;

/// The request's payload as the app's body. A request carrying none answers an empty body without
/// a pump.
///
/// The pump runs on the worker that owns the payload and holds one chunk of queue, which is how far
/// ahead of the app it reads. It stops at the payload's end, at its first error, which the body
/// yields, or when the app drops the body, whichever comes first.
pub(crate) fn request_body(payload: Payload) -> HttpBody {
    if matches!(payload, Payload::None) {
        return HttpBody::empty();
    }
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, BoxError>>(1);
    actix_web::rt::spawn(pump(payload, tx));
    HttpBody::stream(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

async fn pump(mut payload: Payload, tx: mpsc::Sender<Result<Bytes, BoxError>>) {
    loop {
        let next = {
            let abandoned = pin!(tx.closed());
            match select(abandoned, payload.next()).await {
                Either::Left(_) => return,
                Either::Right((next, _)) => next,
            }
        };
        let Some(chunk) = next else { return };
        let chunk = chunk.map_err(|error| BoxError::from(error.to_string()));
        let failed = chunk.is_err();
        if tx.send(chunk).await.is_err() || failed {
            return;
        }
    }
}

/// The app's response body as actix's: its exact length where the app's body knows it, so actix
/// writes `Content-Length`, and streamed otherwise. Trailers are dropped, since actix's body
/// carries none.
pub(crate) struct ResponseBody(pub(crate) HttpBody);

impl MessageBody for ResponseBody {
    type Error = io::Error;

    fn size(&self) -> BodySize {
        match self.0.size_hint().exact() {
            Some(length) => BodySize::Sized(length),
            None => BodySize::Stream,
        }
    }

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, io::Error>>> {
        let body = &mut self.get_mut().0;
        loop {
            match ready!(Pin::new(&mut *body).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) if !data.is_empty() => return Poll::Ready(Some(Ok(data))),
                    Ok(_) | Err(_) => continue,
                },
                Some(Err(error)) => return Poll::Ready(Some(Err(io::Error::other(error)))),
                None => return Poll::Ready(None),
            }
        }
    }
}
