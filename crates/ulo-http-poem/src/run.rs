//! `run`: the poem host under the app's one shutdown trigger.

use std::future::Future;

use poem::IntoEndpoint;
use poem::listener::{Acceptor, Listener};
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Calls `server.run_with_graceful_shutdown(endpoint, handle.stopping(), Some(drain))`, the drain
/// read from `AppHandle::drain_timeout()`, installs it with `Handle::host`, and awaits
/// `app.serve(signal)`.
///
/// The bounds are the ones poem puts on `run_with_graceful_shutdown`, with the endpoint `Send`
/// since the host's future is.
pub async fn run<L, A, E>(
    app: App<Bound>,
    handle: &Handle,
    server: poem::Server<L, A>,
    endpoint: E,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError>
where
    L: Listener + 'static,
    L::Acceptor: 'static,
    A: Acceptor + 'static,
    E: IntoEndpoint + Send + 'static,
    E::Endpoint: 'static,
{
    let drain = app.handle().drain_timeout();
    let host = server.run_with_graceful_shutdown(endpoint, handle.stopping(), Some(drain));
    if let Err(error) = handle.host(host) {
        // The app is bound and nothing will serve it: close it rather than leave it listening.
        let _ = app.handle().close(Signal::new("the poem host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
}
