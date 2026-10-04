//! `run`: the rocket host under the app's one shutdown trigger.

use std::future::Future;

use rocket::Build;
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Sets `shutdown.ctrlc = false` and `shutdown.grace`, the drain window from
/// `AppHandle::drain_timeout()` rounded up to the second, before `ignite()`; takes the `Shutdown`
/// handle, installs a future that notifies it on `handle.stopping()` then awaits `launch()`, and
/// awaits `app.serve(signal)`.
pub async fn run(
    app: App<Bound>,
    handle: &Handle,
    rocket: rocket::Rocket<Build>,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError> {
    let _ = (app, handle, rocket, signal);
    todo!()
}
