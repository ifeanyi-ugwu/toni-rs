//! `run`: the salvo host under the app's one shutdown trigger.

use std::future::{Future, poll_fn};
use std::pin::pin;

use salvo::conn::Acceptor;
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Serves `service` with `server.try_serve`, calling `stop_graceful(Some(drain))` on the server's
/// handle once `handle.stopping()` resolves, the drain read from `AppHandle::drain_timeout()`;
/// installs that with `Handle::host`, and awaits `app.serve(signal)`.
pub async fn run<A>(
    app: App<Bound>,
    handle: &Handle,
    server: salvo::Server<A>,
    service: salvo::Service,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError>
where
    A: Acceptor + Send + 'static,
{
    let drain = app.handle().drain_timeout();
    let control = server.handle();
    let stopping = handle.stopping();
    let host = alongside(server.try_serve(service), async move {
        stopping.await;
        control.stop_graceful(drain);
    });
    if let Err(error) = handle.host(host) {
        // The app is bound and nothing will serve it: close it rather than leave it listening.
        let _ = app.handle().close(Signal::new("the salvo host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
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
