//! `run`: the axum host under the app's one shutdown trigger.

use std::convert::Infallible;
use std::fmt::Debug;
use std::future::{Future, IntoFuture};

use axum::extract::Request;
use axum::response::Response;
use axum::serve::{IncomingStream, Listener};
use tower::Service;
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Attaches `with_graceful_shutdown(handle.stopping())` to `serve`, installs it with
/// `Handle::host`, and awaits `app.serve(signal)`. It takes `axum::serve::Serve` rather than a
/// `WithGracefulShutdown`, since a second shutdown signal would be a second owner of the trigger.
/// axum's drain has no bound of its own; the app's `close` bound contains it.
///
/// The bounds are the ones axum puts on awaiting a `WithGracefulShutdown`.
pub async fn run<L, M, S>(
    app: App<Bound>,
    handle: &Handle,
    serve: axum::serve::Serve<L, M, S>,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError>
where
    L: Listener,
    L::Addr: Debug,
    M: for<'a> Service<IncomingStream<'a, L>, Error = Infallible, Response = S> + Send + 'static,
    for<'a> <M as Service<IncomingStream<'a, L>>>::Future: Send,
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send,
{
    let host = serve.with_graceful_shutdown(handle.stopping()).into_future();
    if let Err(error) = handle.host(host) {
        // The app is bound and nothing will serve it: close it rather than leave it listening.
        let _ = app.handle().close(Signal::new("the axum host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
}
