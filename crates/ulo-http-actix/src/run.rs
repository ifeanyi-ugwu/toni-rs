//! `run`: the actix-web host under the app's one shutdown trigger.

use std::fmt;
use std::future::Future;

use actix_http::Request;
use actix_http::body::MessageBody;
use actix_service::{IntoServiceFactory, ServiceFactory};
use actix_web::HttpServer;
use actix_web::dev::{AppConfig, Response};
use ulo::app::Bound;
use ulo::{App, BoxError, Shutdown, Signal};

use crate::Handle;

/// Calls `server.disable_signals().shutdown_timeout(secs).run()`, the drain window from
/// `AppHandle::drain_timeout()` rounded up to the second, installs a future that awaits
/// `handle.stopping()`, calls `ServerHandle::stop(true)`, then awaits the server, and awaits
/// `app.serve(signal)`.
pub async fn run<F, I, S, B>(
    app: App<Bound>,
    handle: &Handle,
    server: HttpServer<F, I, S, B>,
    signal: impl Future<Output = Signal> + Send,
) -> Result<Shutdown, BoxError>
where
    F: Fn() -> I + Send + Clone + 'static,
    I: IntoServiceFactory<S, Request>,
    S: ServiceFactory<Request, Config = AppConfig>,
    S::Error: Into<actix_web::Error>,
    S::InitError: fmt::Debug,
    S::Response: Into<Response<B>>,
    B: MessageBody,
{
    let _ = (app, handle, server, signal);
    todo!()
}
