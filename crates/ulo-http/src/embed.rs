//! Running the application inside another framework's server (transports DESIGN §3.8): the host
//! owns the sockets, and the app runs as a service the host mounts, nested under a path or as its
//! fallback.
//!
//! ```ignore
//! let server = ulo_http_axum::Embedded::new().nested_at("/api").body_limit(4 * MB);
//! let embedded = server.handle();
//! let app = App::builder(AppModule).runtime(ulo_tokio::Tokio::current()).wire()?.connect().await?.bind(server).listen().await?;
//! let router = axum::Router::new().nest_service("/api", embedded.service());
//! ```
//!
//! [`Embedded`] is a `Server` binding nothing: `listen()` still prepares it, so the route table,
//! CORS and paths are checked, and the adapter's [`EmbedLimits`] with them. Routing, extraction,
//! pre-dispatch, dispatch and error rendering run inside the app, so its responses are the ones a
//! backend gives. The host's server settings stay the host's: `Embedded` has no `tls`,
//! `endpoint`, `h2c`, `max_concurrent_streams`, `header_timeout` or `handshake_timeout`.
//!
//! Information crosses into the app through the request's `http::Extensions`, read by
//! [`Host<T>`](crate::Host) or copied into the execution by `PreDispatch::adopt`, and back out
//! through [`Routing`](crate::Routing) in the response's extensions. An adapter puts the host's
//! connection there as a `ConnInfo`, and the path the client sent, before the host stripped the
//! mount prefix, as an [`OriginalPath`]. On a host whose own request store does not reach the
//! app, [`Embedded::forward`] registers a copy from the host's request into those extensions.
//!
//! Shutdown has one owner, the app's `serve(signal)`. The host's server future goes into the
//! handle's slot ([`Handle::host`]) and `Embedded::serve` polls it, so no task is spawned. When the
//! app stops accepting, `Embedded::drain` resolves [`Handle::stopping`], to which an adapter's
//! `run` wires the host's graceful shutdown, and returns at once: the host's lingering connections
//! are `close`'s business, since an embedding cannot cut them. `Embedded::close` waits for the
//! host's future to end, bounded by the core's close bound. A host future that ends before the shutdown began is an
//! error, and the app shuts down naming the transport. A host that keeps its listener until its
//! connections end registers it with [`Handle::register_listener`], and its adapter's `run` stops the host
//! once [`Handle::listeners_closed`] resolves.

use std::borrow::Cow;
use std::convert::Infallible;
use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, Location, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bytes::Bytes;
use ulo::{AppHandle, Bound, BoxError, BoxFuture, DrainToken, Mounted, Phase, TypeName};
use ulo_transport::Count;
use ulo_transport::prepare::{Failure, Failures};

use crate::__private::HttpHandler;
use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::pre_dispatch::{Entry, PreDispatch, Step, Supply};
use crate::render;
use crate::request::{ConnInfo, OnUpgrade, Request};
use crate::response::Response;
use crate::router::pattern::Pattern;
use crate::server::{PreparedApp, prepare_app, upgrades_bound, upgrades_close, upgrades_drain};
use crate::service::AppService;
use crate::transport::{ClientAddr, Http};
use crate::upgrade::UpgradeHandler;

/// One host framework the app can run inside: what an adapter crate implements, as a backend
/// crate implements [`Backend`](crate::Backend).
pub trait Embed: Send + Sync + 'static {
    /// The host's name, as a `Configure` error naming a limit prints it.
    const NAME: &'static str;

    /// The request the host's handlers receive, which each copy [`Embedded::forward`] registers
    /// reads, borrowed for one call: rocket's `Request<'r>`, which its handler holds for the
    /// lifetime `'r`. A request that borrows nothing ignores the lifetime, as actix's
    /// `type HostRequest<'r> = HttpRequest;` does. A host that mounts tower services names
    /// `http::request::Parts`, the head of the `http::Request` it hands over, and only then is
    /// [`Service`] a `tower::Service`.
    type HostRequest<'r>;

    /// What the host cannot do, checked in `prepare`: asking for it is a
    /// `StartupError::Configure` naming the limit.
    fn limits() -> EmbedLimits;

    /// Whether the host removes the mount prefix before its handler sees the path, as axum's
    /// `nest_service` and poem's `nest` do. A host that hands over the full path declares `false`,
    /// and [`Service::respond`] strips the `.nested_at` prefix itself, answering a path outside it
    /// as the app's `NoRoute` 404.
    const STRIPS_PREFIX: bool = true;

    /// The adapter's built-in values, as `forward` registrations every app gets: an
    /// [`OriginalPath`] read from the host's request, for one. [`Embedded::new`] passes the fresh
    /// server through it.
    fn builtin_forwards(embedded: Embedded<Self>) -> Embedded<Self>
    where
        Self: Sized,
    {
        embedded
    }

    /// Takes the host's upgrade future out of a request's extensions, for [`Service`]'s
    /// `tower::Service` impl, which hands it to the app as [`Request::upgrade`] when the host
    /// declares `upgrades`. A hyper-based host stores hyper's `OnUpgrade` there, and its adapter
    /// converts it, as axum's does. `None`, the default, takes nothing; an adapter that builds the
    /// app's request itself for [`Service::respond`] sets `upgrade` there instead.
    fn take_upgrade(extensions: &mut http::Extensions) -> Option<OnUpgrade> {
        let _ = extensions;
        None
    }
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
    /// The host's request extensions reach the app; `false` refuses a handler reading `Host<T>`
    /// and a pre-dispatch `adopt::<T>()` for a `T` that nothing supplies before it is read
    /// (`Embedded::forward`, `PreDispatch::supplies`).
    pub host_extensions: bool,
    /// The host reports the TLS it terminated. Nothing is refused: `ConnInfo::tls` is `None`.
    pub tls_info: bool,
    /// How the host hands over a request body.
    pub request_body: RequestBody,
    /// When the host drops an abandoned response body, which is when the execution observes
    /// `CancelReason::Disconnected`.
    pub disconnect: Disconnect,
    /// Whether a request whose head is still arriving when the host's graceful stop begins reaches
    /// the app. Nothing is refused: a client caught mid-request at the moment of shutdown gets the
    /// app's 503 with `Connection: close` on a host declaring `Served`, and a closed connection
    /// with no answer on one declaring `Closed`.
    pub drain_pending: DrainPending,
    /// How long the host's graceful stop lasts when a response in flight as it begins is abandoned
    /// by the client. Nothing is refused: on a host declaring `Window`, a client that leaves
    /// mid-response during shutdown makes the app's `close` take the whole drain window.
    pub drain_abandoned: DrainAbandoned,
    /// What the host's graceful stop does with an HTTP/2 connection. Nothing is refused: on a
    /// host declaring `Reset`, an HTTP/2 client is sent no GOAWAY, keeps sending on its
    /// connection, and gets the app's 503s until the host resets the connection at its stop
    /// deadline; the app's `close` waits for that deadline while any HTTP/2 connection is open.
    /// A host that serves no HTTP/2 declares what it would do were it enabled.
    pub drain_http2: DrainHttp2,
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
        drain_pending: DrainPending::Served,
        drain_abandoned: DrainAbandoned::Released,
        drain_http2: DrainHttp2::GoAway,
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

    pub const fn drain_pending(self, drain_pending: DrainPending) -> Self {
        EmbedLimits { drain_pending, ..self }
    }

    pub const fn drain_abandoned(self, drain_abandoned: DrainAbandoned) -> Self {
        EmbedLimits { drain_abandoned, ..self }
    }

    pub const fn drain_http2(self, drain_http2: DrainHttp2) -> Self {
        EmbedLimits { drain_http2, ..self }
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

/// What a host does with a request whose head is still arriving when its graceful stop begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainPending {
    /// The request reaches the app, which is draining and answers it 503 with
    /// `Connection: close`.
    Served,
    /// The host closes the connection before the request reaches the app, so the client gets no
    /// answer.
    Closed,
}

/// How long a host's graceful stop lasts when a response in flight as it begins is abandoned by the
/// client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainAbandoned {
    /// Until the host drops the abandoned response, at the moment its `disconnect` declares.
    Released,
    /// The whole drain window, however soon the host drops the abandoned response.
    Window,
}

/// What a host's graceful stop does with an HTTP/2 connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainHttp2 {
    /// The host sends GOAWAY with `NO_ERROR` when its stop begins and closes the connection once
    /// its streams end, so a client opens a new connection for its next request.
    GoAway,
    /// The host sends no GOAWAY. The connection stays open, carrying requests to the draining app,
    /// until the host's stop deadline, when it is reset.
    Reset,
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

/// The path the client sent, before the host stripped the mount prefix, which an adapter puts in
/// the request's extensions where its host keeps it (axum's `OriginalUri`). The request span's
/// `url.path` records it; without one, the path the app received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginalPath {
    path: String,
}

impl OriginalPath {
    pub fn new(path: impl Into<String>) -> Self {
        OriginalPath { path: path.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.path
    }
}

impl From<&http::Uri> for OriginalPath {
    /// The URI's path, its query left out.
    fn from(uri: &http::Uri) -> Self {
        OriginalPath::new(uri.path())
    }
}

/// The HTTP transport's `Server` for an app running inside host `A`: it binds nothing, and the
/// host reaches the app through [`handle`](Self::handle)'s [`Service`].
///
/// Its settings are the app's: the five below, where the app is mounted, whether the host
/// supplies the peer address, which host values are copied in, and what a miss does. Each adapter
/// crate exports an alias, `ulo_http_axum::Embedded`.
pub struct Embedded<A: Embed> {
    config: HttpConfig,
    nested_at: Option<Cow<'static, str>>,
    peer_addr: bool,
    /// What `forward` registered, until `prepare` hands it to `copies`.
    forwarded: Vec<HostCopy<A>>,
    /// The copies every handle's [`Service`] runs, set once `prepare` has checked the app.
    copies: Arc<OnceLock<Box<[HostCopy<A>]>>>,
    on_miss: Miss,
    shared: Arc<Shared>,
    /// Set by `prepare`, installed by `bind`.
    prepared: Option<AppService>,
    /// Every registered upgrade handler, once each, set by `prepare`: a host declaring `upgrades`
    /// hands connections to them, and `drain` and `close` reach those connections through them as
    /// `Server<B>`'s do (X21).
    upgrades: Vec<Arc<dyn UpgradeHandler>>,
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
            prefix: OnceLock::new(),
            outside_warned: AtomicBool::new(false),
            listeners: Mutex::new(Listeners::default()),
        });
        A::builtin_forwards(Embedded {
            config,
            nested_at: None,
            peer_addr: false,
            forwarded: Vec::new(),
            copies: Arc::new(OnceLock::new()),
            on_miss: Miss::Final,
            shared,
            prepared: None,
            upgrades: Vec::new(),
            _host: PhantomData,
        })
    }

    /// The cheap-clone handle the host mounts, usable before `listen()`: a request it receives
    /// before the app is bound is answered 503. Building the host's router before `listen()` and
    /// serving it only after is the host's responsibility.
    pub fn handle(&self) -> Handle<A> {
        Handle { shared: Arc::clone(&self.shared), copies: Arc::clone(&self.copies) }
    }

    /// The body limit of a route without `#[meta(BodyLimit(..))]`.
    pub fn body_limit(self, bytes: u64) -> Self {
        self.configure(|config| config.body_limit = bytes)
    }

    /// The app's in-flight bound; see [`Server::max_inflight`](crate::Server::max_inflight).
    pub fn max_inflight(self, requests: Count) -> Self {
        self.configure(|config| config.max_inflight = requests)
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

    /// Copies a value from the host's own request into the app's request extensions, for a host
    /// whose request store does not reach the app (`host_extensions: false`): actix's
    /// `req.extensions().get::<CurrentUser>().cloned()`. The copy runs on every request before
    /// the app sees it, and its `T` is then supplied everywhere: a handler reading `Host<T>` and
    /// a pre-dispatch `adopt::<T>()` are accepted. `None` inserts nothing, and a copy that panics
    /// inserts nothing and is logged at `error`.
    pub fn forward<T: Clone + Send + Sync + 'static>(
        mut self,
        copy: impl for<'r> Fn(&A::HostRequest<'r>) -> Option<T> + Send + Sync + 'static,
    ) -> Self {
        self.forwarded.push(HostCopy {
            ty: TypeName::of::<T>(),
            run: Box::new(move |host, extensions| {
                if let Some(value) = copy(host) {
                    extensions.insert(value);
                }
            }),
        });
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
    fn mount(&self, failures: &mut Failures) -> String {
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
    fn check_limits(&self, mounted: &Mounted<'_, Http>, service: Option<&AppService>, upgrading: bool, failures: &mut Failures) {
        let limits = A::limits();
        let host = A::NAME;
        if upgrading && !limits.upgrades {
            failures.push(format!(
                "the {host} embedding declares `upgrades: false`, so a gateway on the HTTP transport cannot take an upgrade \
                 through this host; serve the gateways on a separate port"
            ));
        }
        if !limits.peer_addr && !self.peer_addr {
            let input = TypeName::of::<ClientAddr>();
            for reader in mounted.handlers_reading::<ClientAddr>() {
                let controller = reader.handler().controller().key().type_name();
                let handler = reader.handler().name();
                let transport = reader.transport();
                let steps = reader.steps().to_vec();
                failures.push(Failure::naming(vec![controller, transport, input], move |names| {
                    let head = format!("{}::{handler} ({})", names.of(controller), names.of(transport));
                    let path: Vec<String> = std::iter::once(head).chain(steps.iter().cloned()).collect();
                    let input = names.of(input);
                    format!(
                        "{} reads `{input}`, and the {host} embedding supplies no peer address: read `Option<Dep<{input}>>`, \
                         or call `.peer_addr(true)` when the host provides it",
                        path.join(" → ")
                    )
                }));
            }
        }
        if !limits.host_extensions {
            self.check_host_values(mounted, service, failures);
        }
        if self.on_miss == Miss::Forward && !limits.forward_miss {
            failures.push(format!(
                "`.on_miss(Miss::Forward)`: the {host} embedding declares `forward_miss: false`, so the host cannot route a miss again"
            ));
        }
    }

    /// Under `host_extensions: false`: every `Host<T>` a handler reads and every pre-dispatch
    /// `adopt::<T>()`, each against the supplies that reach it. A `forward` copy reaches
    /// everything, being in place before any entry runs. A `PreDispatch::supplies` after an
    /// unscoped entry reaches every handler and the `adopt` entries after it in stage order,
    /// whatever its `exclude`, which a later rewrite can make miss or match; one after a scoped
    /// entry, the handlers of the routes that entry's stage covers. `service` is
    /// `None` when the stage or the route table failed, and the handlers are then not checked,
    /// since which routes a scoped entry covers is unknown.
    fn check_host_values(&self, mounted: &Mounted<'_, Http>, service: Option<&AppService>, failures: &mut Failures) {
        let host = A::NAME;
        let metas = mounted.module_meta::<PreDispatch>();
        let forwarded = |ty: TypeName| self.forwarded.iter().any(|copy| copy.ty == ty);
        // The unscoped entries in the order the unscoped sub-step runs them, modules in collection
        // order: each supply and each `adopt` with its entry's position in that order.
        let mut unscoped: Vec<(usize, &Supply)> = Vec::new();
        let mut adopts: Vec<(usize, &Entry, TypeName)> = Vec::new();
        let mut scoped: Vec<(&Arc<PreDispatch>, usize, &Supply)> = Vec::new();
        let mut position = 0;
        for (_, meta) in &metas {
            for (index, entry) in meta.entries.iter().enumerate() {
                if entry.scoped {
                    scoped.extend(entry.supplies.iter().map(|supply| (meta, index, supply)));
                    continue;
                }
                if let Step::Adopt(_, ty) = &entry.step {
                    adopts.push((position, entry, *ty));
                }
                unscoped.extend(entry.supplies.iter().map(|supply| (position, supply)));
                position += 1;
            }
        }
        let routes = service.map_or(&[][..], |service| &service.inner.router.routes[..]);
        for (method, target) in routes.iter().flat_map(|route| &route.methods) {
            let Some(http) = target.handler.handler::<HttpHandler>() else { continue };
            for read in &http.host_reads {
                let ty = read.ty;
                if forwarded(ty) || unscoped.iter().any(|(_, supply)| supply.ty == ty) {
                    continue;
                }
                let of_type = scoped.iter().filter(|(_, _, supply)| supply.ty == ty);
                let covers = |meta: &Arc<PreDispatch>, index: usize| {
                    target.stage.steps.iter().any(|(_, held, at)| Arc::ptr_eq(held, meta) && *at == index)
                };
                if of_type.clone().any(|(meta, index, _)| covers(meta, *index)) {
                    continue;
                }
                let elsewhere: Vec<Site> = of_type.map(|(_, _, supply)| supply.location).collect();
                let controller = target.handler.controller().key().type_name();
                let handler = target.handler.name();
                let (method, pattern) = (method.clone(), Arc::clone(&target.pattern));
                failures.push(Failure::naming(vec![controller, ty], move |names| {
                    let name = names.of(ty);
                    let why = if elsewhere.is_empty() {
                        unsupplied(&name)
                    } else {
                        let entries = if elsewhere.len() == 1 { "a scoped entry whose scope does" } else { "scoped entries whose scopes do" };
                        format!("{} {entries} not cover `{pattern}`, so it does not reach this route", declared(&name, &elsewhere))
                    };
                    format!(
                        "`{}::{handler}` on `{method} {pattern}` reads `Host<{name}>`, and the {host} embedding declares \
                         `host_extensions: false`: {why}",
                        names.of(controller),
                    )
                }));
            }
        }
        for (at, entry, ty) in adopts {
            if forwarded(ty) || unscoped.iter().any(|(position, supply)| *position < at && supply.ty == ty) {
                continue;
            }
            let later: Vec<Site> = unscoped.iter().filter(|(_, supply)| supply.ty == ty).map(|(_, supply)| supply.location).collect();
            let routed: Vec<Site> = scoped.iter().filter(|(_, _, supply)| supply.ty == ty).map(|(_, _, supply)| supply.location).collect();
            let location = entry.location;
            failures.push(Failure::naming(vec![ty], move |names| {
                let name = names.of(ty);
                let mut why = Vec::new();
                if !later.is_empty() {
                    why.push(format!(
                        "{} an entry that runs after this `adopt`, which then finds nothing; write the `adopt` after it",
                        declared(&name, &later)
                    ));
                }
                if !routed.is_empty() {
                    why.push(format!(
                        "{} a scoped entry, which runs after routing and so after every `adopt`",
                        declared(&name, &routed)
                    ));
                }
                if why.is_empty() {
                    why.push(unsupplied(&name));
                }
                format!(
                    "`adopt::<{name}>()` at {location} copies a host value, and the {host} embedding declares \
                     `host_extensions: false`: {}",
                    why.join("; ")
                )
            }));
        }
    }
}

/// Where a `supplies::<T>()` was written.
type Site = &'static Location<'static>;

/// Why nothing reaches a read of the host value `name`, when no `supplies` of it exists.
fn unsupplied(name: &str) -> String {
    format!(
        "nothing supplies it. A value the host keeps is copied in with `Embedded::forward::<{name}>(..)`, and a \
         pre-dispatch entry inserting it is declared with `.supplies::<{name}>()` after it"
    )
}

/// "the `.supplies::<T>()` at {sites} follows", agreeing with how many there are.
fn declared(name: &str, sites: &[Site]) -> String {
    let at: Vec<String> = sites.iter().map(|site| site.to_string()).collect();
    let verb = if sites.len() == 1 { "follows" } else { "follow" };
    format!("the `.supplies::<{name}>()` at {} {verb}", at.join(", "))
}

impl<A: Embed> ulo::Server for Embedded<A> {
    type Transport = Http;

    /// The route table and stage as a backend's server builds them, then the host's limits.
    async fn prepare(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        let mount = self.mount(&mut failures);
        let PreparedApp { service, upgrading, handlers } =
            prepare_app(&mounted, &self.config, &mount, self.on_miss == Miss::Forward, &mut failures);
        self.check_limits(&mounted, service.as_ref(), upgrading, &mut failures);
        let Some(service) = service.filter(|_| failures.is_empty()) else {
            return Err(Box::new(failures.into_error()));
        };
        let _ = self.copies.set(std::mem::take(&mut self.forwarded).into_boxed_slice());
        let _ = self.shared.app.set(mounted.app().clone());
        let _ = self.shared.prefix.set(mount);
        self.prepared = Some(service);
        self.upgrades = handlers;
        Ok(())
    }

    /// Installs the service the handle's [`Service`] calls; no socket is acquired. The upgrade
    /// handlers' `bound` runs here, as it does after a backend binds.
    async fn bind(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let Some(service) = self.prepared.take() else {
            return Err(BoxError::from("the embedded HTTP server was bound before it was prepared"));
        };
        *self.shared.state() = State::Bound(service);
        upgrades_bound(&self.upgrades, mounted.app());
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

    /// Resolves [`Handle::stopping`], to which the host's graceful shutdown is wired, and runs every
    /// upgrade handler's drain, which closes the upgraded connections the host no longer sees. The
    /// core has already fired `draining()`, and from then on the app answers a new request 503
    /// with `Connection: close`.
    async fn drain(&self, token: DrainToken) {
        self.shared.stopping.fire();
        upgrades_drain(&self.upgrades, &token).await;
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
        let host_ended = async {
            if polled {
                poll_fn(|cx| shared.finished.poll_fired(cx)).await;
            }
        };
        futures_util::future::join(host_ended, upgrades_close(&self.upgrades)).await;
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
    /// `A::limits().upgrades`: whether the upgrade future `Embed::take_upgrade` finds is handed on.
    upgrades: bool,
    /// The normalized `.nested_at` prefix, set at `prepare`; read only when the host leaves the
    /// prefix on the path (`Embed::STRIPS_PREFIX == false`).
    prefix: OnceLock<String>,
    /// A request outside the prefix is logged once, not per request.
    outside_warned: AtomicBool,
    /// The listeners registered through [`Handle::register_listener`].
    listeners: Mutex<Listeners>,
}

/// What [`Handle::register_listener`] registered, and who waits for those still open.
#[derive(Default)]
struct Listeners {
    registered: usize,
    open: usize,
    waiting: Vec<Waker>,
}

/// Marks a request whose path lies outside the mount prefix, which routing answers as a miss.
#[derive(Clone, Copy)]
pub(crate) struct OutsidePrefix;

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

    fn listeners(&self) -> MutexGuard<'_, Listeners> {
        self.listeners.lock().unwrap_or_else(PoisonError::into_inner)
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

/// One `Embedded::forward` registration: the type it supplies, and the copy into a request's
/// extensions.
struct HostCopy<A: Embed> {
    ty: TypeName,
    run: Box<dyn for<'r> Fn(&A::HostRequest<'r>, &mut http::Extensions) + Send + Sync>,
}

/// The handle to an [`Embedded`] server for host `A`: cheap to clone, usable before `listen()`.
pub struct Handle<A: Embed> {
    shared: Arc<Shared>,
    copies: Arc<OnceLock<Box<[HostCopy<A>]>>>,
}

impl<A: Embed> Clone for Handle<A> {
    fn clone(&self) -> Self {
        Handle { shared: Arc::clone(&self.shared), copies: Arc::clone(&self.copies) }
    }
}

impl<A: Embed> Handle<A> {
    /// The service the host mounts, `Router::nest_service("/api", embedded.service())` on a host
    /// mounting tower services; an adapter building the app's request itself calls
    /// [`Service::respond`].
    pub fn service(&self) -> Service<A> {
        Service { shared: Arc::clone(&self.shared), copies: Arc::clone(&self.copies) }
    }

    /// The request body limit the embedding was configured with, under which an adapter that
    /// buffers the body reads it.
    pub fn body_limit(&self) -> u64 {
        self.shared.state().config().body_limit
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
    ///
    /// This is the signal an adapter's `run` wires. It resolves at the same moment as the core's
    /// `AppHandle::draining()`, and is available before `listen()` returns, while the app has no
    /// `AppHandle` yet: the host's server and its shutdown are built from the handle alone.
    pub fn stopping(&self) -> Stopping {
        Stopping { shared: Arc::clone(&self.shared) }
    }

    /// The app's handle once it has prepared; `None` before. Its `drain_timeout()` is the window a
    /// host's own drain clock takes.
    pub fn app(&self) -> Option<AppHandle> {
        self.shared.app.get().cloned()
    }

    /// Registers a listener the host's server holds and the adapter closes once
    /// [`stopping`](Self::stopping) resolves, for a host that keeps its listener until its
    /// connections end. Dropping the returned [`HostListener`] reports the listener closed, so the
    /// adapter drops it with the listener.
    pub fn register_listener(&self) -> HostListener {
        let mut listeners = self.shared.listeners();
        listeners.registered += 1;
        listeners.open += 1;
        HostListener { shared: Arc::clone(&self.shared) }
    }

    /// Resolves once every listener registered through [`register_listener`](Self::register_listener) has closed;
    /// `None` when none was ever registered on this handle. An adapter's `run` awaits it before
    /// telling the host to stop, so the stop cannot race the listener's close.
    pub fn listeners_closed(&self) -> Option<ListenersClosed> {
        let registered = self.shared.listeners().registered;
        (registered > 0).then(|| ListenersClosed { shared: Arc::clone(&self.shared) })
    }
}

/// A listener registered through [`Handle::register_listener`]; reports it closed when dropped.
pub struct HostListener {
    shared: Arc<Shared>,
}

impl Drop for HostListener {
    fn drop(&mut self) {
        let waiting = {
            let mut listeners = self.shared.listeners();
            listeners.open -= 1;
            if listeners.open > 0 {
                return;
            }
            std::mem::take(&mut listeners.waiting)
        };
        for waker in waiting {
            waker.wake();
        }
    }
}

/// What [`Handle::listeners_closed`] returns.
pub struct ListenersClosed {
    shared: Arc<Shared>,
}

impl Future for ListenersClosed {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut listeners = self.shared.listeners();
        if listeners.open == 0 {
            return Poll::Ready(());
        }
        if !listeners.waiting.iter().any(|waker| waker.will_wake(cx.waker())) {
            listeners.waiting.push(cx.waker().clone());
        }
        Poll::Pending
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

/// The app as a service of host `A`: through [`respond`](Self::respond) for an adapter that
/// builds the app's request itself, and as a tower `Service` over `http::Request<B>` for a host
/// that mounts tower services, whose `Embed::HostRequest<'r>` is `http::request::Parts` for
/// every `'r`.
///
/// Each tower request becomes the app's [`Request`]: its parts as they arrive, extensions
/// included, with what each [`Embedded::forward`] copy read off them added, its body as an
/// `HttpBody`, the connection from a `ConnInfo` in its extensions (an adapter's layer puts the
/// host's connection there; without one only the HTTP version is known), and the upgrade future
/// [`Embed::take_upgrade`] finds when the host declares `upgrades`. The response is the
/// app's, problem details for an error, with [`Routing`](crate::Routing) in its extensions; the
/// service never fails.
pub struct Service<A: Embed> {
    shared: Arc<Shared>,
    copies: Arc<OnceLock<Box<[HostCopy<A>]>>>,
}

impl<A: Embed> Clone for Service<A> {
    fn clone(&self) -> Self {
        Service { shared: Arc::clone(&self.shared), copies: Arc::clone(&self.copies) }
    }
}

impl<A: Embed> Service<A> {
    /// Answers one request the adapter built from `host`, the host's own request: each
    /// [`Embedded::forward`] copy runs on `host` first, inserting what it finds into `req`'s
    /// extensions. Then 503 "not yet listening" before `listen()`, 503 with `Connection: close`
    /// after `close`, and the app's answer between.
    pub fn respond(&self, host: &A::HostRequest<'_>, mut req: Request) -> impl Future<Output = Response> + Send + use<A> {
        self.copy(host, &mut req.head.extensions);
        if !A::STRIPS_PREFIX {
            self.strip_prefix(&mut req);
        }
        self.answer(req)
    }

    /// Removes the mount prefix from a path the host handed over whole, keeping the query. A
    /// path outside the prefix is marked, so routing answers it as the app's `NoRoute` 404 rather
    /// than matching an in-app route by accident.
    fn strip_prefix(&self, req: &mut Request) {
        let Some(prefix) = self.shared.prefix.get().filter(|prefix| !prefix.is_empty()) else { return };
        let path = req.head.uri.path().to_owned();
        let rest = match path.strip_prefix(prefix.as_str()) {
            Some("") => "/",
            Some(rest) if rest.starts_with('/') => rest,
            _ => {
                if !self.shared.outside_warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        host = A::NAME,
                        path = %path,
                        prefix = %prefix,
                        "a request outside the embedding's `.nested_at` prefix is answered as the app's 404"
                    );
                }
                req.head.extensions.insert(OutsidePrefix);
                return;
            }
        };
        let rewritten = match req.head.uri.query() {
            Some(query) => format!("{rest}?{query}"),
            None => rest.to_owned(),
        };
        let mut parts = req.head.uri.clone().into_parts();
        parts.path_and_query = rewritten.parse().ok();
        if let Ok(uri) = http::Uri::from_parts(parts) {
            req.head.uri = uri;
        }
    }

    /// Runs every `forward` copy on `host` into `extensions`. A copy that panics inserts nothing:
    /// the handler reading its value then answers `HostMissing`, as for a value the host left out.
    fn copy(&self, host: &A::HostRequest<'_>, extensions: &mut http::Extensions) {
        for copy in self.copies.get().into_iter().flatten() {
            if catch_unwind(AssertUnwindSafe(|| (copy.run)(host, extensions))).is_err() {
                tracing::error!(host = A::NAME, r#type = %format_args!("{:#}", copy.ty), "an `Embedded::forward` copy panicked; it inserts nothing");
            }
        }
    }

    fn answer(&self, req: Request) -> impl Future<Output = Response> + Send + use<A> {
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

    fn convert<B>(&self, mut head: http::request::Parts, body: B) -> Request
    where
        B: http_body::Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        let upgrade = A::take_upgrade(&mut head.extensions).filter(|_| self.shared.upgrades);
        let conn = head.extensions.get::<ConnInfo>().cloned().unwrap_or_else(|| ConnInfo::new(head.version));
        Request { head, body: HttpBody::new(body), conn, upgrade }
    }
}

impl<A, B> tower::Service<http::Request<B>> for Service<A>
where
    A: for<'r> Embed<HostRequest<'r> = http::request::Parts>,
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
        let (mut head, body) = req.into_parts();
        let mut copied = http::Extensions::new();
        self.copy(&head, &mut copied);
        head.extensions.extend(copied);
        let answer = self.answer(self.convert(head, body));
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
