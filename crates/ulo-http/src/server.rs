use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ulo::{BoundAddr, BoxError, DrainToken, Mounted, Transport};
use ulo_net::{Activation, ActivationError, BoundListener, Endpoint, EndpointSpec, ListenerName, Tls};
use ulo_transport::Admission;

use crate::backend::{Backend, BackendLimits, HttpConfig};
use crate::pre_dispatch::{PreDispatch, Stage};
use crate::router::Router;
use crate::router::pattern::Pattern;
use crate::service::{AppService, ServiceInner};
use crate::transport::Http;
use crate::upgrade::{UpgradeHandler, Upgrades};

/// The HTTP server over backend `B`, the core's `Server` for [`Http`]: handed to `app.bind(..)`,
/// it carries addresses, TLS and limits; everything a route needs it reads from the mounted
/// handlers and module metadata.
///
/// ```ignore
/// app.bind(ulo_http_axum::Server::new("0.0.0.0:8080").tls(Tls::from_pem_files("cert.pem", "key.pem")))
/// ```
///
/// `prepare` builds the route table and the pre-dispatch stage, parses every endpoint, loads TLS,
/// checks the inherited sockets and the backend's limits, and reports every failure at once;
/// `bind` binds every endpoint, all-or-nothing, and hands the listeners to the backend.
pub struct Server<B: Backend> {
    pub(crate) endpoints: Vec<EndpointSpec>,
    pub(crate) tls: Option<Tls>,
    pub(crate) config: HttpConfig,
    pub(crate) backend: B,
    /// Set by `prepare`, read by `bind`.
    pub(crate) prepared: Option<Prepared>,
    pub(crate) bound: Vec<BoundAddr>,
}

/// What `prepare` built for `bind`.
pub(crate) struct Prepared {
    pub(crate) endpoints: Vec<ulo_net::Endpoint>,
    pub(crate) tls: Option<ulo_net::TlsAcceptor>,
    pub(crate) service: AppService,
}

impl<B: Backend + Default> Server<B> {
    /// A server on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Server::with_backend(endpoint, B::default())
    }
}

impl<B: Backend> Server<B> {
    pub fn with_backend(endpoint: impl Into<EndpointSpec>, backend: B) -> Self {
        Server { endpoints: vec![endpoint.into()], tls: None, config: HttpConfig::default(), backend, prepared: None, bound: Vec::new() }
    }

    /// One more endpoint the same server listens on.
    pub fn endpoint(mut self, endpoint: impl Into<EndpointSpec>) -> Self {
        self.endpoints.push(endpoint.into());
        self
    }

    /// TLS on every endpoint, ALPN `h2` and `http/1.1`, loaded in `prepare`.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// The body limit of a route without `#[meta(BodyLimit(..))]`.
    pub fn body_limit(mut self, bytes: u64) -> Self {
        self.config.body_limit = bytes;
        self
    }

    pub fn max_inflight(mut self, requests: usize) -> Self {
        self.config.max_inflight = Some(requests);
        self
    }

    /// HTTP/2 `SETTINGS_MAX_CONCURRENT_STREAMS` per connection.
    pub fn max_concurrent_streams(mut self, streams: u32) -> Self {
        self.config.max_concurrent_streams = Some(streams);
        self
    }

    pub fn shed_retry_after(mut self, after: Duration) -> Self {
        self.config.shed_retry_after = after;
        self
    }

    /// The `WWW-Authenticate` challenge of a 401 whose error names none; `Bearer` (RFC 6750) unset.
    pub fn challenge(mut self, challenge: impl Into<Cow<'static, str>>) -> Self {
        self.config.challenge = challenge.into();
        self
    }

    /// Accept HTTP/2 without TLS, refused in `prepare` on a backend whose limits forbid it.
    pub fn h2c(mut self, enabled: bool) -> Self {
        self.config.h2c = enabled;
        self
    }
}

impl<B: Backend> Server<B> {
    /// Every endpoint resolved and checked against the backend's limits, an inherited one against
    /// the sockets this process inherited. Failures go to `failures`; the endpoints that resolved
    /// are returned.
    fn resolve_endpoints(&self, limits: BackendLimits, failures: &mut Vec<String>) -> Vec<Endpoint> {
        let mut endpoints: Vec<Endpoint> = Vec::new();
        for spec in &self.endpoints {
            let endpoint = match spec.resolve() {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    failures.push(error.to_string());
                    continue;
                }
            };
            match &endpoint {
                Endpoint::Addr(addr) => {
                    if addr.port() == 0 && !limits.port_zero {
                        failures.push(format!("{endpoint}: the {} backend declares `port_zero: false`", B::NAME));
                    }
                    // Two port-0 endpoints are two listeners, each on a port of the OS's choosing.
                    if addr.port() != 0 && endpoints.contains(&endpoint) {
                        failures.push(format!("{endpoint} is listed twice"));
                    }
                }
                Endpoint::Inherited(_) if !limits.inherited_sockets => {
                    failures.push(format!("{endpoint}: the {} backend declares `inherited_sockets: false`", B::NAME));
                }
                Endpoint::Inherited(_) => {}
            }
            endpoints.push(endpoint);
        }
        if limits.inherited_sockets {
            check_inherited(&endpoints, failures);
        }
        endpoints
    }
}

/// Each inherited name `endpoints` lists, against the sockets this process inherited. Every
/// listing takes the next descriptor answering to its name at `bind`, so a name listed more often
/// than descriptors answer to it is refused here, once per name. An index answers for one
/// descriptor at most, so an index listed twice is refused the same way.
fn check_inherited(endpoints: &[Endpoint], failures: &mut Vec<String>) {
    let mut names: Vec<(&ListenerName, usize)> = Vec::new();
    for endpoint in endpoints {
        if let Endpoint::Inherited(name) = endpoint {
            match names.iter_mut().find(|(seen, _)| *seen == name) {
                Some((_, listed)) => *listed += 1,
                None => names.push((name, 1)),
            }
        }
    }
    if names.is_empty() {
        return;
    }
    // Read only when an endpoint is inherited: a process spawned by a socket-activated one
    // inherits a `LISTEN_PID` that is not its own and would fail here though it binds addresses
    // only.
    let activation = match Activation::get() {
        Ok(activation) => activation,
        Err(error) => {
            failures.push(error.to_string());
            return;
        }
    };
    for (name, listed) in names {
        match activation.count(name) {
            0 => failures.push(ActivationError::Missing(name.clone()).to_string()),
            inherited if listed > inherited => {
                let sockets = if inherited == 1 { "socket answers" } else { "sockets answer" };
                failures.push(format!(
                    "{} is listed {listed} times, and {inherited} inherited {sockets} to it",
                    Endpoint::Inherited(name.clone())
                ));
            }
            _ => {}
        }
    }
}

/// The paths every registered `UpgradeHandler` takes, parsed; two that would match the same
/// requests are refused, as two gateways on one path are.
fn upgrade_paths(mounted: &Mounted<'_, Http>, failures: &mut Vec<String>) -> Vec<(Pattern, Arc<dyn UpgradeHandler>)> {
    let mut paths: Vec<(Pattern, Arc<dyn UpgradeHandler>)> = Vec::new();
    for (_, upgrades) in mounted.module_meta::<Upgrades>() {
        for handler in &upgrades.handlers {
            for path in handler.paths(mounted.app()) {
                let pattern = match Pattern::parse(&path) {
                    Ok(pattern) => pattern,
                    Err(error) => {
                        failures.push(format!("upgrade path: {error}"));
                        continue;
                    }
                };
                let clash = paths.iter().find(|(taken, _)| taken.conflicts(&pattern)).map(|(taken, _)| taken.to_string());
                match clash {
                    Some(taken) => failures.push(format!("two upgrade paths take the same requests: `{taken}` and `{pattern}`")),
                    None => paths.push((pattern, Arc::clone(handler))),
                }
            }
        }
    }
    paths
}

/// Every failure one `prepare` found, which `listen()` reports as one `StartupError::Configure`
/// entry for the HTTP transport.
#[derive(Debug)]
struct PrepareError {
    failures: Vec<String>,
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.failures.as_slice() {
            [] => f.write_str("the route table could not be built"),
            [failure] => f.write_str(failure),
            failures => {
                write!(f, "{} problems:", failures.len())?;
                for failure in failures {
                    write!(f, "\n  - {failure}")?;
                }
                Ok(())
            }
        }
    }
}

impl Error for PrepareError {}

impl<B: Backend> ulo::Server for Server<B> {
    type Transport = Http;

    async fn prepare(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let mut failures = Vec::new();
        let limits = B::limits();
        let stage = match Stage::build(mounted.module_meta::<PreDispatch>()) {
            Ok(stage) => stage,
            Err(errors) => {
                failures.extend(errors);
                Stage::empty()
            }
        };
        let router = match Router::build(mounted.handlers(), self.config.body_limit, &stage) {
            Ok(router) => Some(router),
            Err(errors) => {
                failures.extend(errors.into_iter().map(|error| error.message));
                None
            }
        };
        let upgrades = upgrade_paths(&mounted, &mut failures);
        if !upgrades.is_empty() && !limits.upgrades {
            failures.push(format!(
                "the {} backend declares `upgrades: false`, so it cannot hand a WebSocket upgrade to a gateway on its port; \
                 serve the gateways on a separate port",
                B::NAME
            ));
        }
        let endpoints = self.resolve_endpoints(limits, &mut failures);
        if self.config.h2c && !limits.h2c {
            failures.push(format!("the {} backend declares `h2c: false`: HTTP/2 without TLS is refused", B::NAME));
        }
        let tls = match &self.tls {
            None => None,
            Some(_) if !limits.tls => {
                failures.push(format!("the {} backend declares `tls: false`", B::NAME));
                None
            }
            Some(tls) => match tls.load(&[b"h2".as_slice(), b"http/1.1".as_slice()]) {
                Ok(acceptor) => Some(acceptor),
                Err(error) => {
                    failures.push(error.to_string());
                    None
                }
            },
        };
        let Some(router) = router.filter(|_| failures.is_empty()) else {
            return Err(Box::new(PrepareError { failures }));
        };
        let config = Arc::new(self.config.clone());
        let service = AppService::new(ServiceInner {
            app: mounted.app().clone(),
            router,
            stage,
            upgrades,
            admission: Admission::new(config.max_inflight).retry_after(config.shed_retry_after),
            config,
            timer: Arc::clone(mounted.timer()),
        });
        self.prepared = Some(Prepared { endpoints, tls, service });
        Ok(())
    }

    async fn bind(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let _ = mounted;
        let Some(Prepared { endpoints, tls, service }) = self.prepared.take() else {
            return Err(BoxError::from("the HTTP server was bound before it was prepared"));
        };
        let listeners = ulo_net::bind_all(&endpoints)?;
        let addrs: Vec<SocketAddr> = listeners.iter().map(BoundListener::local_addr).collect();
        let secure = tls.is_some();
        self.backend.bind(listeners, tls, service, &self.config).await?;
        self.bound = addrs.into_iter().map(|addr| BoundAddr::new(<Http as Transport>::KEY, addr).tls(secure)).collect();
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.backend.serve().await
    }

    async fn drain(&self, token: DrainToken) {
        let _ = token;
        self.backend.drain().await
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.backend.close().await
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.bound.clone()
    }
}
