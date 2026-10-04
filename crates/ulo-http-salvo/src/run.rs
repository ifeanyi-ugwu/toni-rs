//! `run`: the salvo host under the app's one shutdown trigger.

use std::future::Future;

use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Calls `server.serve_with_graceful_shutdown(service, handle.stopping(), Some(drain))`, the drain
/// read from `AppHandle::drain_timeout()`, installs it with `Handle::host`, and awaits
/// `app.serve(signal)`.
pub async fn run<A>(
    app: App<Bound>,
    handle: &Handle,
    server: salvo::Server<A>,
    service: salvo::Service,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError> {
    let _ = (app, handle, server, service, signal);
    todo!()
}
