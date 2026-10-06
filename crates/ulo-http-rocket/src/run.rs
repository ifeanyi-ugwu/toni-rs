//! `run`: the rocket host under the app's one shutdown trigger.

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::time::Duration;

use rocket::Build;
use ulo::app::Bound;
use ulo::{App, BoxError, BoxFuture, Shutdown, Signal};

use crate::Handle;

/// Sets `shutdown.ctrlc = false`, no shutdown signals, and `shutdown.grace`, the drain window from
/// `AppHandle::drain_timeout()` rounded up to the second, before `ignite()`; takes the `Shutdown`
/// handle from the ignited rocket, installs a future that notifies it on `handle.stopping()` then
/// awaits `launch()`, and awaits `app.serve(signal)`.
///
/// A rocket that fails to ignite is installed as a host future failing at once, so the app shuts
/// down naming the error rather than staying bound with nothing serving it.
pub async fn run(
    app: App<Bound>,
    handle: &Handle,
    rocket: rocket::Rocket<Build>,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError> {
    let grace = whole_seconds(app.handle().drain_timeout());
    let figment = rocket
        .figment()
        .clone()
        .merge(("shutdown.ctrlc", false))
        .merge(("shutdown.signals", Vec::<String>::new()))
        .merge(("shutdown.grace", grace));
    let host: BoxFuture<'static, Result<(), BoxError>> = match rocket.configure(figment).ignite().await {
        Ok(ignited) => {
            let shutdown = ignited.shutdown();
            let stopping = handle.stopping();
            Box::pin(async move {
                let launched = alongside(ignited.launch(), async move {
                    stopping.await;
                    shutdown.notify();
                })
                .await;
                // `to_string` marks rocket's error handled, which its `Drop` otherwise panics on.
                launched.map(drop).map_err(|error| BoxError::from(error.to_string()))
            })
        }
        Err(error) => {
            let message = error.to_string();
            Box::pin(async move { Err(BoxError::from(message)) })
        }
    };
    if let Err(error) = handle.host(host) {
        // The app is bound and nothing will serve it: close it rather than leave it listening.
        let _ = app.handle().close(Signal::new("the rocket host server was refused")).await;
        return Err(error);
    }
    Ok(app.serve(signal).await?)
}

/// `window` in whole seconds, rounded up: rocket's clock is the app's plus under a second.
fn whole_seconds(window: Duration) -> u32 {
    let seconds = window.as_secs().saturating_add(u64::from(window.subsec_nanos() > 0));
    u32::try_from(seconds).unwrap_or(u32::MAX)
}

/// Polls `serve` to its end, running `beside` with it until `beside` completes.
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
