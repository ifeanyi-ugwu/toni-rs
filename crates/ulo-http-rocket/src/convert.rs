//! The conversion at rocket's edge, both directions. rocket 0.5 is on `http` 0.2 and carries its own
//! method, header and status types, so the head crosses as strings and bytes.

use std::io::{self, Cursor};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use http::StatusCode;
use http::header::{CONTENT_LENGTH, UPGRADE};
use http_body::Body as _;
use http_body_util::BodyExt;
use rocket::http::Status;
use tokio::io::{AsyncRead, ReadBuf};
use ulo_http::HttpBody;

use crate::upgrade::Handoff;

/// rocket's request head as the app's. rocket exposes no protocol version, so the head says
/// HTTP/1.1, which is what rocket serves without TLS.
pub(crate) fn head(request: &rocket::Request<'_>) -> Result<http::request::Parts, http::Error> {
    let mut builder = http::Request::builder()
        .method(request.method().as_str())
        .uri(request.uri().to_string())
        .version(http::Version::HTTP_11);
    for header in request.headers().iter() {
        builder = builder.header(header.name().as_str(), header.value());
    }
    Ok(builder.body(())?.into_parts().0)
}

/// The app's response as rocket's. A body of known length is collected and handed over sized, so
/// rocket writes its `Content-Length`; any other streams. A 101 carries `handoff` as the I/O
/// handler for the protocol its `Upgrade` header names, and rocket then writes the switch itself.
///
/// rocket writes `Content-Length` from the size of the body it is handed and keeps every header
/// set beside it, so the app's own `Content-Length` is not copied: hyper refuses a response
/// carrying two. For a `HEAD` request the app's `Content-Length` is the length of the body it
/// omitted, handed over as the size of an empty sized body, which rocket keeps when it strips
/// the body of a `HEAD` answer.
pub(crate) async fn response<'r>(response: ulo_http::Response, handoff: Option<Handoff>, head: bool) -> rocket::Response<'r> {
    let (parts, body) = response.into_parts();
    let mut builder = rocket::Response::build();
    builder.status(Status::new(parts.status.as_u16()));
    for (name, value) in &parts.headers {
        if name == CONTENT_LENGTH {
            continue;
        }
        builder.raw_header_adjoin(name.as_str().to_owned(), String::from_utf8_lossy(value.as_bytes()).into_owned());
    }
    if parts.status == StatusCode::SWITCHING_PROTOCOLS {
        let protocol = parts.headers.get(UPGRADE).and_then(|value| value.to_str().ok());
        if let (Some(handoff), Some(protocol)) = (handoff, protocol) {
            builder.upgrade(protocol.to_owned(), handoff);
        }
        return builder.finalize();
    }
    let omitted = parts.headers.get(CONTENT_LENGTH).and_then(|value| value.to_str().ok()?.parse::<usize>().ok());
    if let (true, Some(length)) = (head, omitted) {
        builder.sized_body(length, Cursor::new(Bytes::new()));
        return builder.finalize();
    }
    match body.size_hint().exact() {
        Some(0) => {}
        Some(_) => match body.collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                builder.sized_body(bytes.len(), Cursor::new(bytes));
            }
            Err(error) => {
                tracing::error!(%error, "the app's response body failed before rocket could send it; sending it empty");
            }
        },
        None => {
            builder.streamed_body(BodyReader { body, chunk: Bytes::new(), done: false });
        }
    }
    builder.finalize()
}

/// The app's body as the `AsyncRead` rocket streams, a read answering as soon as one frame has
/// arrived, so an SSE event is written when the app yields it.
struct BodyReader {
    body: HttpBody,
    chunk: Bytes,
    done: bool,
}

impl AsyncRead for BodyReader {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        while this.chunk.is_empty() {
            if this.done {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut this.body).poll_frame(cx)) {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        this.chunk = data;
                    }
                }
                Some(Err(error)) => {
                    this.done = true;
                    return Poll::Ready(Err(io::Error::other(error)));
                }
                None => {
                    this.done = true;
                    return Poll::Ready(Ok(()));
                }
            }
        }
        let n = this.chunk.len().min(buf.remaining());
        buf.put_slice(&this.chunk.split_to(n));
        Poll::Ready(Ok(()))
    }
}
