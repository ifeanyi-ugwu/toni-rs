//! `run`: the poem host under the app's one shutdown trigger.

use std::future::Future;

use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Calls `server.run_with_graceful_shutdown(endpoint, handle.stopping(), Some(drain))`, the drain
/// read from `AppHandle::drain_timeout()`, installs it with `Handle::host`, and awaits
/// `app.serve(signal)`.
pub async fn run<L, A, E>(
    app: App<Bound>,
    handle: &Handle,
    server: poem::Server<L, A>,
    endpoint: E,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError> {
    let _ = (app, handle, server, endpoint, signal);
    todo!()
}
