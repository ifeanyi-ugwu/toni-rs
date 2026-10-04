//! `run`: the axum host under the app's one shutdown trigger.

use std::future::Future;

use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Attaches `with_graceful_shutdown(handle.stopping())` to `serve`, installs it with
/// `Handle::host`, and awaits `app.serve(signal)`. It takes `axum::serve::Serve` rather than a
/// `WithGracefulShutdown`, since a second shutdown signal would be a second owner of the trigger.
/// axum's drain has no bound of its own; the app's `close` bound contains it.
pub async fn run<L, M, S>(
    app: App<Bound>,
    handle: &Handle,
    serve: axum::serve::Serve<L, M, S>,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError> {
    let _ = (app, handle, serve, signal);
    todo!()
}
