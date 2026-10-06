//! `run`: the salvo host under the app's one shutdown trigger.

use std::future::{Future, pending, poll_fn};
use std::io::Result as IoResult;
use std::pin::{Pin, pin};
use std::task::Poll;

use salvo::conn::{Accepted, Acceptor, Holding};
use salvo::fuse::ArcFuseFactory;
use tokio::sync::oneshot;
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};
use ulo_http::embed::Stopping;

use crate::Handle;

/// Builds `salvo::Server::new` over `acceptor` and serves `service` with `try_serve`. Once
/// `handle.stopping()` resolves, the acceptor is dropped, closing its listener, and then
/// `stop_graceful(Some(drain))` is called on the server's handle, the drain read from
/// `AppHandle::drain_timeout()`; installs that with `Handle::host`, and awaits `app.serve(signal)`.
///
/// salvo keeps its acceptor until every connection has closed, so a connection arriving during
/// the drain would otherwise be accepted by the kernel and never served. Dropping it first makes
/// such a connection refused, as on the other hosts.
pub async fn run<A>(
    app: App<Bound>,
    handle: &Handle,
    acceptor: A,
    service: salvo::Service,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError>
where
    A: Acceptor + Send + 'static,
{
    let drain = app.handle().drain_timeout();
    let (closed, listener_closed) = oneshot::channel();
    let server = salvo::Server::new(Closing {
        holdings: acceptor.holdings().to_vec(),
        inner: Some(acceptor),
        stopping: handle.stopping(),
        closed: Some(closed),
    });
    let control = server.handle();
    let host = alongside(server.try_serve(service), async move {
        // The listener closes before the stop command, which ends salvo's accept loop: a stop read
        // first would leave the acceptor alive inside `try_serve` until its connections end.
        let _ = listener_closed.await;
        control.stop_graceful(drain);
    });
    if let Err(error) = handle.host(host) {
        // The app is bound and nothing will serve it: close it rather than leave it listening.
        let _ = app.handle().close(Signal::new("the salvo host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
}

/// The caller's acceptor until `stopping` resolves; then nothing, its listener closed.
struct Closing<A> {
    holdings: Vec<Holding>,
    inner: Option<A>,
    stopping: Stopping,
    /// Fired once `inner` is dropped.
    closed: Option<oneshot::Sender<()>>,
}

impl<A: Acceptor + Send + 'static> Acceptor for Closing<A> {
    type Coupler = A::Coupler;
    type Stream = A::Stream;

    fn holdings(&self) -> &[Holding] {
        &self.holdings
    }

    async fn accept(&mut self, fuse_factory: Option<ArcFuseFactory>) -> IoResult<Accepted<Self::Coupler, Self::Stream>> {
        if let Some(inner) = self.inner.as_mut() {
            let stopping = &mut self.stopping;
            let mut accept = pin!(inner.accept(fuse_factory));
            let accepted = poll_fn(|cx| {
                if Pin::new(&mut *stopping).poll(cx).is_ready() {
                    return Poll::Ready(None);
                }
                accept.as_mut().poll(cx).map(Some)
            })
            .await;
            if let Some(accepted) = accepted {
                return accepted;
            }
        }
        self.inner = None;
        if let Some(closed) = self.closed.take() {
            let _ = closed.send(());
        }
        pending().await
    }
}

/// Polls `serve` to its end, running `beside` with it until `beside` completes. salvo's stop
/// command is read inside `serve`'s loop, so the server is polled throughout.
async fn alongside<S: Future, W: Future<Output = ()>>(serve: S, beside: W) -> S::Output {
    let mut serve = pin!(serve);
    let mut beside = pin!(beside);
    let mut watching = true;
    poll_fn(|cx| {
        if watching && beside.as_mut().poll(cx).is_ready() {
            watching = false;
        }
        serve.as_mut().poll(cx)
    })
    .await
}
