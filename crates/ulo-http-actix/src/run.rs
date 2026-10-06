//! `run`: the actix-web host under the app's one shutdown trigger.

use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::time::Duration;

use actix_http::Request;
use actix_http::body::MessageBody;
use actix_service::{IntoServiceFactory, Service, ServiceFactory};
use actix_web::HttpServer;
use actix_web::dev::{AppConfig, Response};
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Calls `server.disable_signals().shutdown_timeout(secs).run()`, the drain window from
/// `AppHandle::drain_timeout()` rounded up to the second, and installs a future that awaits
/// `handle.stopping()`, calls `ServerHandle::stop(true)`, then awaits the server; answers a future
/// that awaits `app.serve(signal)`.
///
/// The builder work happens before `run` returns, since actix's `HttpServer` is `!Send` and the
/// running server it yields is not, so the answered future is `Send` and can be spawned. Like
/// actix itself, `run` must be called inside a tokio runtime.
///
/// The bounds are the ones actix-web puts on `disable_signals` and `shutdown_timeout`, which
/// `run` calls; they are `HttpServer::new`'s, so a server built by `HttpServer::new` meets them.
pub fn run<F, I, S, B, Sig>(
    app: App<Bound>,
    handle: &Handle,
    server: HttpServer<F, I, S, B>,
    signal: Sig,
) -> impl Future<Output = Result<Shutdown, BoxError>> + Send + use<F, I, S, B, Sig>
where
    F: Fn() -> I + Send + Clone + 'static,
    I: IntoServiceFactory<S, Request>,
    S: ServiceFactory<Request, Config = AppConfig> + 'static,
    S::Error: Into<actix_web::Error> + 'static,
    S::InitError: fmt::Debug,
    S::Response: Into<Response<B>> + 'static,
    <S::Service as Service<Request>>::Future: 'static,
    S::Service: 'static,
    B: MessageBody + 'static,
    Sig: Future<Output = Signal> + Send,
{
    let secs = whole_seconds(app.handle().drain_timeout());
    let server = server.disable_signals().shutdown_timeout(secs).run();
    let control = server.handle();
    let stopping = handle.stopping();
    // The server's command loop runs inside its future, so the future is polled while the stop is
    // sent and until it ends.
    let host = alongside(server, async move {
        stopping.await;
        control.stop(true).await;
    });
    let installed = handle.host(host);
    async move {
        if let Err(error) = installed {
            // The app is bound and nothing will serve it: close it rather than leave it listening.
            let _ = app.handle().close(Signal::new("the actix host server was refused")).await;
            return Err(error);
        }
        Ok(app.serve(signal).await?)
    }
}

/// `window` in whole seconds, rounded up: actix's clock is the app's plus under a second.
fn whole_seconds(window: Duration) -> u64 {
    window.as_secs().saturating_add(u64::from(window.subsec_nanos() > 0))
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
