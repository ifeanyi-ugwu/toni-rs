//! Ends in-flight replies when the drain deadline elapses.
//!
//! tonic spawns a detached task per connection (`serve_connection`, which
//! `tokio::spawn`s), and each holds a receiver whose drop is how the server
//! learns the connection finished. Dropping the serve future therefore stops
//! the acceptor and leaves established connections serving — a reply that never
//! ends keeps a connection open past a shutdown the application has already
//! been told completed.
//!
//! Nothing in tonic hands back a handle to those tasks. What the framework does
//! own is the body each call answers with, so the deadline ends the bodies: the
//! reply closes with `UNAVAILABLE`, its stream drops (firing the execution's
//! cancellation token through `ScopedGrpcStream`), the connection has no
//! in-flight calls left, and tonic's own graceful shutdown closes it.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::{HeaderMap, HeaderValue, Request, Response};
use http_body::{Body as HttpBody, Frame, SizeHint};
use tokio::sync::watch;
use tonic::body::Body as TonicBody;
use tower::{Layer, Service};

/// gRPC signals a mid-stream end through trailers, not through the frames.
fn unavailable_trailers() -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert(
        "grpc-status",
        HeaderValue::from_static("14"), // UNAVAILABLE
    );
    trailers.insert(
        "grpc-message",
        HeaderValue::from_static("server shutting down"),
    );
    trailers
}

#[derive(Clone)]
pub struct DrainLayer {
    deadline: watch::Receiver<bool>,
}

impl DrainLayer {
    pub fn new(deadline: watch::Receiver<bool>) -> Self {
        Self { deadline }
    }
}

impl<S> Layer<S> for DrainLayer {
    type Service = DrainService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DrainService {
            inner,
            deadline: self.deadline.clone(),
        }
    }
}

#[derive(Clone)]
pub struct DrainService<S> {
    inner: S,
    deadline: watch::Receiver<bool>,
}

impl<S, ResBody> Service<Request<TonicBody>> for DrainService<S>
where
    S: Service<Request<TonicBody>, Response = Response<ResBody>> + Send + 'static,
    S::Future: Send + 'static,
    ResBody: HttpBody + Send + 'static,
{
    type Response = Response<DrainBody<ResBody>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<TonicBody>) -> Self::Future {
        let fut = self.inner.call(req);
        let deadline = self.deadline.clone();
        Box::pin(async move {
            let response = fut.await?;
            Ok(response.map(|body| DrainBody::new(body, deadline)))
        })
    }
}

/// Delegates to the reply body until the deadline flips, then closes the call.
pub struct DrainBody<B> {
    inner: Pin<Box<B>>,
    /// Resolves when the deadline elapses. Polled alongside the body so a reply
    /// parked waiting for its next item is woken rather than waited on.
    elapsed: Pin<Box<dyn Future<Output = ()> + Send>>,
    ended: bool,
}

impl<B> DrainBody<B> {
    fn new(inner: B, mut deadline: watch::Receiver<bool>) -> Self {
        Self {
            inner: Box::pin(inner),
            elapsed: Box::pin(async move {
                while !*deadline.borrow_and_update() {
                    if deadline.changed().await.is_err() {
                        // The adapter is gone, which outlives every reply.
                        std::future::pending::<()>().await;
                    }
                }
            }),
            ended: false,
        }
    }
}

impl<B: HttpBody + Send> HttpBody for DrainBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        if this.elapsed.as_mut().poll(cx).is_ready() {
            this.ended = true;
            return Poll::Ready(Some(Ok(Frame::trailers(unavailable_trailers()))));
        }
        this.inner.as_mut().poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.ended || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
