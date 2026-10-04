//! Running the application inside another framework's server (transports DESIGN §3.8): the host
//! owns the sockets, and the app runs as a service the host mounts, nested under a path or as its
//! fallback.
//!
//! ```ignore
//! let server = ulo_http_axum::Embedded::new().nested_at("/api").body_limit(4 * MB);
//! let embedded = server.handle();
//! let app = App::builder(AppModule).timer(ulo_tokio::Timer).wire()?.connect().await?.bind(server).listen().await?;
//! let router = axum::Router::new().nest_service("/api", embedded.service());
//! ```
//!
//! [`Embedded`] is a `Server` binding nothing: `listen()` still prepares it, so the route table,
//! CORS and paths are checked, and the adapter's [`EmbedLimits`] with them. Routing, extraction,
//! pre-dispatch, dispatch and error rendering run inside the app, so its responses are the ones a
//! backend gives. The host's server settings stay the host's: `Embedded` has no `tls`,
//! `endpoint`, `h2c` or `max_concurrent_streams`.
//!
//! Information crosses into the app through the request's `http::Extensions`, read by
//! [`Host<T>`](crate::Host) or copied into the execution by `PreDispatch::adopt`, and back out
//! through [`Routing`](crate::Routing) in the response's extensions.
//!
//! Shutdown has one owner, the app's `serve(signal)`. The host's server future goes into the
//! handle's slot ([`Handle::host`]) and `Embedded::serve` polls it, so no task is spawned. When the
//! app stops accepting, `Embedded::drain` resolves [`Handle::stopping`], which the host's graceful
//! shutdown awaits, and returns at once: the host's lingering connections are `close`'s business,
//! since an embedding cannot cut them. `Embedded::close` waits for the host's future to end,
//! bounded by the core's close bound. A host future that ends before the shutdown began is an
//! error, and the app shuts down naming the transport.

use std::borrow::Cow;
use std::convert::Infallible;
use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bytes::Bytes;
use hyper_util::rt::TokioIo;
use ulo::{AppHandle, Bound, BoxError, BoxFuture, DrainToken, Mounted, Phase};

use crate::__private::HttpHandler;
use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::render;
use crate::request::{ConnInfo, OnUpgrade, Request, Upgraded};
use crate::response::Response;
use crate::router::pattern::Pattern;
use crate::server::{PrepareError, prepare_app};
use crate::service::AppService;
use crate::transport::{ClientAddr, Http};

/// One host framework the app can run inside: what an adapter crate implements, as a backend
/// crate implements [`Backend`](crate::Backend).
pub trait Embed: Send + Sync + 'static {
    /// The host's name, as a `Configure` error naming a limit prints it.
    const NAME: &'static str;

    /// What the host cannot do, checked in `prepare`: asking for it is a
    /// `StartupError::Configure` naming the limit.
    fn limits() -> EmbedLimits;
}

/// What a host cannot do, as its adapter declares it. Separate from
/// [`BackendLimits`](crate::BackendLimits): a host owns its sockets, so ports, TLS and h2c are not
/// the app's to limit.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbedLimits {
    /// The host supplies the peer address with every request. `false` refuses a handler reading
    /// `Dep<ClientAddr>` without `Option`, directly or through an execution-scoped service it
    /// reaches, unless `Embedded::peer_addr(true)` says this host provides it.
    pub peer_addr: bool,
    /// The host hands over a per-request upgraded I/O; `false` refuses a gateway on the HTTP
    /// transport.
    pub upgrades: bool,
    /// The host can route a request the app missed again; `false` refuses `Miss::Forward`.
    pub forward_miss: bool,
    /// The host's request extensions reach the app; `false` refuses a handler reading `Host<T>`.
    pub host_extensions: bool,
    /// The host reports the TLS it terminated. Nothing is refused: `ConnInfo::tls` is `None`.
    pub tls_info: bool,
    /// How the host hands over a request body.
    pub request_body: RequestBody,
    /// When the host drops an abandoned response body, which is when the execution observes
    /// `CancelReason::Disconnected`.
    pub disconnect: Disconnect,
}

impl EmbedLimits {
    /// A host that can do everything.
    pub const NONE: EmbedLimits = EmbedLimits {
        peer_addr: true,
        upgrades: true,
        forward_miss: true,
        host_extensions: true,
        tls_info: true,
        request_body: RequestBody::Streamed,
        disconnect: Disconnect::AtClose,
    };

    pub const fn peer_addr(self, supported: bool) -> Self {
        EmbedLimits { peer_addr: supported, ..self }
    }

    pub const fn upgrades(self, supported: bool) -> Self {
        EmbedLimits { upgrades: supported, ..self }
    }

    pub const fn forward_miss(self, supported: bool) -> Self {
        EmbedLimits { forward_miss: supported, ..self }
    }

    pub const fn host_extensions(self, supported: bool) -> Self {
        EmbedLimits { host_extensions: supported, ..self }
    }

    pub const fn tls_info(self, supported: bool) -> Self {
        EmbedLimits { tls_info: supported, ..self }
    }

    pub const fn request_body(self, body: RequestBody) -> Self {
        EmbedLimits { request_body: body, ..self }
    }

    pub const fn disconnect(self, disconnect: Disconnect) -> Self {
        EmbedLimits { disconnect, ..self }
    }
}

/// How a host hands over a request body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestBody {
    /// As it arrives.
    Streamed,
    /// Read whole before the app sees it, refused over this many bytes by the host.
    Buffered(u64),
}

/// When a host drops a response body the peer abandoned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disconnect {
    /// When the connection closes.
    AtClose,
    /// At the next write that fails, so an idle stream is not cancelled until it has something
    /// to send.
    AtNextWrite,
}

/// What the app does with a request no route matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Miss {
    /// The app answers 404 through its global error handlers, as on a backend.
    #[default]
    Final,
    /// The host routes the request again, where it can (`EmbedLimits::forward_miss`): a request
    /// whose body nothing has polled and whose path no pre-dispatch entry rewrote is answered 404
    /// with [`Forwardable`] in its extensions, skipping the error handlers, for the adapter to
    /// hand back. Any other miss is the app's own 404, logged at `warn`. A 405 is never
    /// forwarded: a route of the app matches the path.
    Forward,
}

/// In the extensions of a 404 the app answered under `Miss::Forward` for a request the host can
/// route again: its body unread and its path unchanged.
#[derive(Clone, Copy, Debug)]
pub struct Forwardable {
    pub(crate) _private: (),
}

/// The HTTP transport's `Server` for an app running inside host `A`: it binds nothing, and the
/// host reaches the app through [`handle`](Self::handle)'s [`Service`].
///
/// Its settings are the app's: the five below, where the app is mounted, whether the host
/// supplies the peer address, and what a miss does. Each adapter crate exports an alias,
/// `ulo_http_axum::Embedded`.
pub struct Embedded<A: Embed> {
    config: HttpConfig,
    nested_at: Option<Cow<'static, str>>,
    peer_addr: bool,
    on_miss: Miss,
    shared: Arc<Shared>,
    /// Set by `prepare`, installed by `bind`.
    prepared: Option<AppService>,
    _host: PhantomData<fn() -> A>,
}

impl<A: Embed> Default for Embedded<A> {
    fn default() -> Self {
        Embedded::new()
    }
}

impl<A: Embed> Embedded<A> {
    pub fn new() -> Self {
        let config = HttpConfig::default();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::Unbound(Arc::new(config.clone()))),
            host: Mutex::new(HostSlot::Empty(None)),
            stopping: Notice::default(),
            finished: Notice::default(),
            app: OnceLock::new(),
            upgrades: A::limits().upgrades,
        });
        Embedded { config, nested_at: None, peer_addr: false, on_miss: Miss::Final, shared, prepared: None, _host: PhantomData }
    }

    /// The cheap-clone handle the host mounts, usable before `listen()`: a request it receives
    /// before the app is bound is answered 503. Building the host's router before `listen()` and
    /// serving it only after is the host's responsibility.
    pub fn handle(&self) -> Handle {
        Handle { shared: Arc::clone(&self.shared) }
    }

    /// The body limit of a route without `#[meta(BodyLimit(..))]`.
    pub fn body_limit(self, bytes: u64) -> Self {
        self.configure(|config| config.body_limit = bytes)
    }

    pub fn max_inflight(self, requests: usize) -> Self {
        self.configure(|config| config.max_inflight = Some(requests))
    }

    /// `Retry-After` on a load-shedding refusal, a request refused during the drain, and one
    /// refused before `listen()`.
    pub fn shed_retry_after(self, after: Duration) -> Self {
        self.configure(|config| config.shed_retry_after = after)
    }

    /// The `WWW-Authenticate` challenge of a 401 whose error names none; `Bearer` unset.
    pub fn challenge(self, challenge: impl Into<Cow<'static, str>>) -> Self {
        let challenge = challenge.into();
        self.configure(move |config| config.challenge = challenge)
    }

    /// How long the error handlers may take with a route timeout's `Timeout`; see
    /// [`Server::timeout_grace`](crate::Server::timeout_grace).
    pub fn timeout_grace(self, grace: Bound) -> Self {
        self.configure(|config| config.timeout_grace = grace)
    }

    /// The path the host nests the app under, which it strips before the app sees a request:
    /// `Router::nest_service("/api", ..)` takes `.nested_at("/api")`. The app routes on the
    /// stripped path, records the full route in its span and in `Routing::Matched`, and hands
    /// handlers the prefix through `HttpCx::mount_prefix()`. Unset for a fallback.
    pub fn nested_at(mut self, prefix: impl Into<Cow<'static, str>>) -> Self {
        self.nested_at = Some(prefix.into());
        self
    }

    /// Whether the host supplies the peer address on a host whose adapter declares
    /// `peer_addr: false`: axum does when served through
    /// `into_make_service_with_connect_info::<SocketAddr>()`.
    pub fn peer_addr(mut self, provided: bool) -> Self {
        self.peer_addr = provided;
        self
    }

    /// What the app does with a request no route matches; `Miss::Final` unset.
    pub fn on_miss(mut self, miss: Miss) -> Self {
        self.on_miss = miss;
        self
    }

    fn configure(mut self, set: impl FnOnce(&mut HttpConfig)) -> Self {
        set(&mut self.config);
        *self.shared.state() = State::Unbound(Arc::new(self.config.clone()));
        self
    }

    /// The prefix with a leading `/` and no trailing one, empty for `/` or none; a prefix that is
    /// not a plain path is a failure.
    fn mount(&self, failures: &mut Vec<String>) -> String {
        let Some(prefix) = self.nested_at.as_deref() else { return String::new() };
        let joined = Pattern::join(prefix, "");
        if joined == "/" {
            return String::new();
        }
        match Pattern::parse(&joined) {
            Ok(pattern) if pattern.param_names().next().is_none() => joined,
            Ok(_) => {
                failures.push(format!("`.nested_at(\"{prefix}\")`: a mount prefix is a plain path, without parameters"));
                String::new()
            }
            Err(error) => {
                failures.push(format!("`.nested_at(\"{prefix}\")`: {error}"));
                String::new()
            }
        }
    }

    /// The limits the app needs against what host `A` declares.
    fn check_limits(&self, mounted: &Mounted<'_, Http>, upgrading: bool, failures: &mut Vec<String>) {
        let limits = A::limits();
        let host = A::NAME;
        if upgrading && !limits.upgrades {
            failures.push(format!(
                "the {host} embedding declares `upgrades: false`, so a gateway on the HTTP transport cannot take an upgrade \
                 through this host; serve the gateways on a separate port"
            ));
        }
        if !limits.peer_addr && !self.peer_addr {
            for reader in mounted.handlers_reading::<ClientAddr>() {
                failures.push(format!(
                    "{} reads `ClientAddr`, and the {host} embedding supplies no peer address: read `Option<Dep<ClientAddr>>`, \
                     or call `.peer_addr(true)` when the host provides it",
                    reader.path().join(" → ")
                ));
            }
        }
        if !limits.host_extensions {
            for handler in mounted.handlers() {
                let Some(http) = handler.handler::<HttpHandler>() else { continue };
                for read in &http.host_reads {
                    failures.push(format!(
                        "`{}::{}` reads `Host<{}>`, and the {host} embedding declares `host_extensions: false`: \
                         the host's request extensions do not reach the app",
                        handler.controller(),
                        handler.name(),
                        read.type_name
                    ));
                }
            }
        }
        if self.on_miss == Miss::Forward && !limits.forward_miss {
            failures.push(format!(
                "`.on_miss(Miss::Forward)`: the {host} embedding declares `forward_miss: false`, so the host cannot route a miss again"
            ));
        }
    }
}

impl<A: Embed> ulo::Server for Embedded<A> {
    type Transport = Http;

    /// The route table and stage as a backend's server builds them, then the host's limits.
    async fn prepare(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let mut failures = Vec::new();
        let mount = self.mount(&mut failures);
        let (service, upgrading) = prepare_app(&mounted, &self.config, &mount, self.on_miss == Miss::Forward, &mut failures);
        self.check_limits(&mounted, upgrading, &mut failures);
        let Some(service) = service.filter(|_| failures.is_empty()) else {
            return Err(Box::new(PrepareError { failures }));
        };
        let _ = self.shared.app.set(mounted.app().clone());
        self.prepared = Some(service);
        Ok(())
    }

    /// Installs the service the handle's [`Service`] calls; no socket is acquired.
    async fn bind(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let _ = mounted;
        let Some(service) = self.prepared.take() else {
            return Err(BoxError::from("the embedded HTTP server was bound before it was prepared"));
        };
        *self.shared.state() = State::Bound(service);
        Ok(())
    }

    /// Polls the host's server future from the handle's slot, waiting for [`Handle::host`] when
    /// none is there yet. The host future's error is this transport's error; a host future that
    /// ends before the shutdown began is one too, since nothing then serves the app.
    async fn serve(&self) -> Result<(), BoxError> {
        let shared = &*self.shared;
        let _finished = FireOnDrop(&shared.finished);
        let mut host: Option<BoxFuture<'static, Result<(), BoxError>>> = None;
        let ended = poll_fn(|cx| {
            let fut = match &mut host {
                Some(fut) => fut,
                None => match shared.take_host(cx.waker()) {
                    Some(fut) => host.insert(fut),
                    None => return Poll::Pending,
                },
            };
            fut.as_mut().poll(cx)
        })
        .await;
        let early = shared.app.get().is_some_and(|app| app.phase() == Phase::Running);
        match ended {
            Err(error) => Err(format!("the {} host server failed: {error}", A::NAME).into()),
            Ok(()) if early => Err(format!("the {} host server stopped before the shutdown began", A::NAME).into()),
            Ok(()) => Ok(()),
        }
    }

    /// Resolves [`Handle::stopping`], to which the host's graceful shutdown is wired, and returns.
    /// The core has already fired `draining()`, and from then on the app answers a new request 503
    /// with `Connection: close`.
    async fn drain(&self, token: DrainToken) {
        let _ = token;
        self.shared.stopping.fire();
    }

    /// Waits for the host's server future to end when `serve` is polling it, bounded by what the
    /// core allows `close`; a host future installed and never polled is dropped. A request the
    /// handle receives afterwards is answered 503 as during the drain.
    async fn close(&self) -> Result<(), BoxError> {
        let shared = &*self.shared;
        shared.stopping.fire();
        let polled = {
            let mut slot = shared.host();
            match std::mem::replace(&mut *slot, HostSlot::Released) {
                HostSlot::Taken => {
                    *slot = HostSlot::Taken;
                    true
                }
                HostSlot::Empty(_) | HostSlot::Installed(_) | HostSlot::Released => false,
            }
        };
        if polled {
            poll_fn(|cx| shared.finished.poll_fired(cx)).await;
        }
        let config = shared.state().config();
        *shared.state() = State::Closed(config);
        Ok(())
    }
}

/// The state the server and every handle share.
struct Shared {
    state: Mutex<State>,
    host: Mutex<HostSlot>,
    /// Fired by `drain`, and by `close` for an app closed without one.
    stopping: Notice,
    /// Fired when `serve` ends, with or without the host future.
    finished: Notice,
    app: OnceLock<AppHandle>,
    /// `A::limits().upgrades`: whether a hyper upgrade future in the request is taken.
    upgrades: bool,
}

enum State {
    /// Before `bind`: requests answer 503 "not yet listening" under these settings.
    Unbound(Arc<HttpConfig>),
    Bound(AppService),
    /// After `close`: requests answer 503 as during the drain.
    Closed(Arc<HttpConfig>),
}

impl State {
    fn config(&self) -> Arc<HttpConfig> {
        match self {
            State::Unbound(config) | State::Closed(config) => Arc::clone(config),
            State::Bound(service) => Arc::clone(&service.inner.config),
        }
    }
}

enum HostSlot {
    /// Nothing installed; `serve`'s waker, once it waits.
    Empty(Option<Waker>),
    Installed(BoxFuture<'static, Result<(), BoxError>>),
    /// `serve` holds the future.
    Taken,
    /// `close` ran first; an installation now is dropped.
    Released,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn host(&self) -> MutexGuard<'_, HostSlot> {
        self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The installed host future, or `None` with `waker` registered for the installation.
    fn take_host(&self, waker: &Waker) -> Option<BoxFuture<'static, Result<(), BoxError>>> {
        let mut slot = self.host();
        match std::mem::replace(&mut *slot, HostSlot::Taken) {
            HostSlot::Installed(fut) => Some(fut),
            HostSlot::Empty(_) => {
                *slot = HostSlot::Empty(Some(waker.clone()));
                None
            }
            other => {
                *slot = other;
                None
            }
        }
    }
}

/// The handle to an [`Embedded`] server: cheap to clone, usable before `listen()`.
#[derive(Clone)]
pub struct Handle {
    shared: Arc<Shared>,
}

impl Handle {
    /// The tower service the host mounts: `Router::nest_service("/api", embedded.service())`.
    pub fn service(&self) -> Service {
        Service { shared: Arc::clone(&self.shared) }
    }

    /// Hands the app the host's server future, which `Embedded::serve` polls: no task is spawned.
    /// Wire the host's graceful shutdown to [`stopping`](Self::stopping) first. Installed after
    /// `close`, or a second time, the future is dropped and the call answers `Err`.
    pub fn host<F, E>(&self, server: F) -> Result<(), BoxError>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Into<BoxError>,
    {
        let fut: BoxFuture<'static, Result<(), BoxError>> = Box::pin(async move { server.await.map_err(Into::into) });
        let waker = {
            let mut slot = self.shared.host();
            match &mut *slot {
                HostSlot::Empty(waker) => {
                    let waker = waker.take();
                    *slot = HostSlot::Installed(fut);
                    waker
                }
                HostSlot::Installed(_) | HostSlot::Taken => return Err("a host server future is already installed".into()),
                HostSlot::Released => return Err("the embedded server is closed".into()),
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Resolves when the app stops accepting, the moment `Embedded::drain` runs, or at `close` for
    /// an app closed without a drain: the host's graceful-shutdown signal, as in axum's
    /// `with_graceful_shutdown(embedded.stopping())`.
    pub fn stopping(&self) -> Stopping {
        Stopping { shared: Arc::clone(&self.shared) }
    }

    /// The app's handle once it has prepared; `None` before. Its `drain_timeout()` is the window a
    /// host's own drain clock takes.
    pub fn app(&self) -> Option<AppHandle> {
        self.shared.app.get().cloned()
    }
}

/// What [`Handle::stopping`] returns.
pub struct Stopping {
    shared: Arc<Shared>,
}

impl Future for Stopping {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.shared.stopping.poll_fired(cx)
    }
}

/// The app as a tower `Service` over `http::Request<B>`, for a host that mounts tower services,
/// and through [`respond`](Self::respond) for an adapter that builds the app's request itself.
///
/// Each request becomes the app's [`Request`]: its parts as they arrive, extensions included, its
/// body as an `HttpBody`, the connection from a `ConnInfo` in its extensions (an adapter's layer
/// puts the host's connection there; without one only the HTTP version is known), and hyper's
/// upgrade future when the request carries one and the host declares `upgrades`. The response is the app's, problem
/// details for an error, with [`Routing`](crate::Routing) in its extensions; the service never
/// fails.
#[derive(Clone)]
pub struct Service {
    shared: Arc<Shared>,
}

impl Service {
    /// Answers one request the adapter built: 503 "not yet listening" before `listen()`, 503 with
    /// `Connection: close` after `close`, and the app's answer between.
    pub fn respond(&self, req: Request) -> impl Future<Output = Response> + Send + use<> {
        let refusal = match &*self.shared.state() {
            State::Bound(service) => Ok(service.call(req)),
            State::Unbound(config) => Err(render::not_listening(config)),
            State::Closed(config) => Err(render::draining(config)),
        };
        async move {
            match refusal {
                Ok(answer) => answer.await,
                Err(refused) => refused,
            }
        }
    }

    fn convert<B>(&self, req: http::Request<B>) -> Request
    where
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let (mut head, body) = req.into_parts();
        let pending = head.extensions.remove::<hyper::upgrade::OnUpgrade>();
        let upgrade = pending.filter(|_| self.shared.upgrades).map(|pending| {
            OnUpgrade::new(async move {
                let upgraded = pending.await?;
                Ok::<_, BoxError>(Upgraded::new(TokioIo::new(upgraded)))
            })
        });
        let conn = head.extensions.get::<ConnInfo>().cloned().unwrap_or_else(|| ConnInfo::new(head.version));
        Request { head, body: HttpBody::new(body), conn, upgrade }
    }
}

impl<B> tower::Service<http::Request<B>> for Service
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let answer = self.respond(self.convert(req));
        Box::pin(async move { Ok::<_, Infallible>(answer.await) })
    }
}

/// Fires once and stays fired; every listener before or after the fire resolves.
#[derive(Default)]
struct Notice {
    fired: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl Notice {
    fn fire(&self) {
        if self.fired.swap(true, Ordering::AcqRel) {
            return;
        }
        let wakers = std::mem::take(&mut *self.wakers.lock().unwrap_or_else(PoisonError::into_inner));
        for waker in wakers {
            waker.wake();
        }
    }

    fn poll_fired(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.fired.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        let mut wakers = self.wakers.lock().unwrap_or_else(PoisonError::into_inner);
        // `fire` sets the flag before taking the list, so a check under the lock sees either the
        // flag or a list this waker joins before it is taken.
        if self.fired.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
            wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// Fires its notice however the future holding it ends, dropped included.
struct FireOnDrop<'a>(&'a Notice);

impl Drop for FireOnDrop<'_> {
    fn drop(&mut self) {
        self.0.fire();
    }
}
