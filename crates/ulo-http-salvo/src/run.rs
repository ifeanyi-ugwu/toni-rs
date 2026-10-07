//! `run`: the salvo host under the app's one shutdown trigger, and [`Closing`], the acceptor that
//! closes its listener when the drain begins.

use std::future::{Future, pending, poll_fn};
use std::io::Result as IoResult;
use std::pin::{Pin, pin};
use std::task::Poll;

use salvo::conn::{Accepted, Acceptor, Holding};
use salvo::fuse::ArcFuseFactory;
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};
use ulo_http::embed::{HostListener, ListenersClosed, Stopping};

use crate::Handle;

/// Serves `service` on `server` with `try_serve`. Once `handle.stopping()` resolves, the server's
/// [`Closing`] acceptor drops the acceptor it wraps, closing its listener, and reports that through
/// `handle`; then `stop_graceful(Some(drain))` is called on the server's handle, the drain read
/// from `AppHandle::drain_timeout()`. Installs that with `Handle::host`, and awaits
/// `app.serve(signal)`.
///
/// The server is the caller's, built over `Closing::new(&handle, acceptor)`, so its
/// `with_http_builder`, `http1_mut`, `http2_mut` and `fuse_factory` settings are kept. When no
/// `Closing` was built from `handle`, the server's was built from another embedding's handle, and
/// `run` closes the app and answers `Err` without serving. A second `Closing` built from `handle`
/// and kept outside the server holds the stop until it is dropped.
pub async fn run<A>(
    app: App<Bound>,
    handle: &Handle,
    server: salvo::Server<Closing<A>>,
    service: salvo::Service,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError>
where
    A: Acceptor + Send + 'static,
{
    let Some(closed) = handle.listeners_closed() else {
        let _ = app.handle().close(Signal::new("the salvo server's `Closing` is another embedding's")).await;
        return Err("the salvo server's `Closing` was built from another embedding's handle: build it with \
             `Closing::new(&handle, acceptor)` from the handle passed to `run`"
            .into());
    };
    let drain = app.handle().drain_timeout();
    let control = server.handle();
    let host = stop_after_close(server.try_serve(service), closed, move || control.stop_graceful(drain));
    if let Err(error) = handle.host(host) {
        // The app is bound and nothing will serve it: close it rather than leave it listening.
        let _ = app.handle().close(Signal::new("the salvo host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
}

/// The acceptor `run`'s server is built over: `acceptor` until `handle.stopping()` resolves, then
/// nothing, its listener closed and reported closed to `handle`.
///
/// salvo keeps its acceptor until every connection has closed, so a connection arriving during
/// the drain would otherwise be taken by the kernel and never served. Dropping the acceptor when
/// the drain begins makes such a connection refused, as on the other hosts. `run` tells salvo to
/// stop only once the acceptor is gone: salvo selects between accepting and its stop command in
/// random order, and a stop read first keeps the acceptor alive until the connections end.
///
/// ```ignore
/// let acceptor = salvo::conn::TcpListener::new("0.0.0.0:8080").bind().await;
/// let mut server = salvo::Server::new(ulo_http_salvo::Closing::new(&embedded, acceptor));
/// server.http1_mut().max_buf_size(64 * 1024);
/// ulo_http_salvo::run(app, &embedded, server, salvo::Service::new(router), ulo_tokio::shutdown_signal()).await?;
/// ```
pub struct Closing<A> {
    holdings: Vec<Holding>,
    /// Dropped together, the acceptor first, so the listener is closed before it is reported.
    inner: Option<(A, HostListener)>,
    stopping: Stopping,
}

impl<A: Acceptor> Closing<A> {
    /// Wraps `acceptor`, to be dropped when `handle`'s drain begins, and registers it with
    /// `handle`. `holdings()` answers what `acceptor` held at this call.
    pub fn new(handle: &Handle, acceptor: A) -> Self {
        Closing {
            holdings: acceptor.holdings().to_vec(),
            inner: Some((acceptor, handle.listener())),
            stopping: handle.stopping(),
        }
    }
}

impl<A: Acceptor + Send + 'static> Acceptor for Closing<A> {
    type Coupler = A::Coupler;
    type Stream = A::Stream;

    fn holdings(&self) -> &[Holding] {
        &self.holdings
    }

    async fn accept(&mut self, fuse_factory: Option<ArcFuseFactory>) -> IoResult<Accepted<Self::Coupler, Self::Stream>> {
        if let Some((inner, _)) = self.inner.as_mut() {
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
        pending().await
    }
}

/// Polls `serve` to its end, calling `stop` once `closed` resolves: once the server's `Closing` has
/// dropped its acceptor.
async fn stop_after_close<S: Future>(serve: S, mut closed: ListenersClosed, stop: impl FnOnce()) -> S::Output {
    let mut serve = pin!(serve);
    let mut stop = Some(stop);
    poll_fn(|cx| {
        if stop.is_some()
            && Pin::new(&mut closed).poll(cx).is_ready()
            && let Some(stop) = stop.take()
        {
            stop();
        }
        serve.as_mut().poll(cx)
    })
    .await
}
