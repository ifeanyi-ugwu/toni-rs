//! Client modules (transports DESIGN §6.3): a tonic-generated client bound under its own type, its
//! channel connected lazily, so `connect` does no network I/O.
//!
//! ```ignore
//! imports = [GrpcClientModule::<pb::UserServiceClient<Channel>>::for_root(
//!     GrpcEndpoint::new("https://users:443").tls(ClientTls::system_roots()),
//! ).keyed::<UsersApi>()]
//! ```

use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::time::Duration;

use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use ulo::{BoxError, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity};

/// A tonic client built over a channel. `ulo-build` writes the impl for every client it
/// generates, `UserServiceClient::new(channel)`.
pub trait GrpcClient: Send + Sync + 'static {
    fn from_channel(channel: Channel) -> Self;
}

/// Binds client `C`, exported, usually keyed so two endpoints of one service coexist:
/// `Dep<pb::UserServiceClient<Channel>, UsersApi>`.
///
/// The URI is parsed when the app wires, a bad one reported with the wiring errors. The TLS roots
/// are loaded and the channel built when the app connects; the channel opens its connection on
/// the first call.
pub struct GrpcClientModule<C: GrpcClient> {
    pub(crate) endpoint: GrpcEndpoint,
    pub(crate) _client: PhantomData<fn() -> C>,
}

impl<C: GrpcClient> GrpcClientModule<C> {
    pub fn for_root(endpoint: GrpcEndpoint) -> Self {
        GrpcClientModule { endpoint, _client: PhantomData }
    }
}

impl<C: GrpcClient> Module for GrpcClientModule<C> {
    fn identity(&self) -> ModuleIdentity {
        let key = ClientKey::<C> { uri: self.endpoint.uri.clone(), tls: self.endpoint.tls.clone(), _client: PhantomData };
        ModuleIdentity::of_value(&key).label("GrpcClientModule")
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.try_value(ClientEndpoint::<C>::parse(&self.endpoint));
        m.try_singleton(|endpoint: Dep<ClientEndpoint<C>>| async move { endpoint.connect() });
        m.export::<C>();
    }
}

/// Where a client connects: the URI and its TLS. The URI is parsed when the app wires, so a bad
/// one is reported with the wiring errors. An `https` URI without `tls` trusts the platform's
/// roots, as `ClientTls::system_roots()` does.
#[derive(Clone, Debug)]
pub struct GrpcEndpoint {
    pub(crate) uri: String,
    pub(crate) tls: Option<ClientTls>,
}

impl GrpcEndpoint {
    pub fn new(uri: impl Into<String>) -> Self {
        GrpcEndpoint { uri: uri.into(), tls: None }
    }

    pub fn tls(mut self, tls: ClientTls) -> Self {
        self.tls = Some(tls);
        self
    }
}

/// The roots a client trusts.
#[derive(Clone, Debug)]
pub struct ClientTls {
    pub(crate) roots: Roots,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Roots {
    System,
    Pem(Vec<u8>),
}

impl ClientTls {
    /// The platform's trust store.
    pub fn system_roots() -> Self {
        ClientTls { roots: Roots::System }
    }

    /// The CA certificates in `pem`, for a service whose certificate a private CA signed.
    pub fn ca_pem(pem: impl Into<Vec<u8>>) -> Self {
        ClientTls { roots: Roots::Pem(pem.into()) }
    }

    fn config(&self) -> ClientTlsConfig {
        match &self.roots {
            Roots::System => ClientTlsConfig::new().with_native_roots(),
            Roots::Pem(pem) => ClientTlsConfig::new().ca_certificate(Certificate::from_pem(pem)),
        }
    }
}

/// `request` with `grpc-timeout` set from `exec`'s remaining deadline. There is no ambient
/// execution, so nothing forwards a deadline on its own. A deadline already passed is sent as one
/// nanosecond, the shortest `grpc-timeout`, so the callee fails it at once. The remaining time is
/// read on tokio's clock, which `ulo-tokio`'s `Timer` reads too.
pub fn outgoing<T>(exec: &ExecutionRef, mut request: tonic::Request<T>) -> tonic::Request<T> {
    if let Some(deadline) = exec.deadline() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
        request.set_timeout(remaining.max(Duration::from_nanos(1)));
    }
    request
}

/// The module's identity: the client type and the endpoint, so two endpoints of one client are two
/// modules and one endpoint imported twice is one.
struct ClientKey<C> {
    uri: String,
    tls: Option<ClientTls>,
    _client: PhantomData<fn() -> C>,
}

impl<C> Clone for ClientKey<C> {
    fn clone(&self) -> Self {
        ClientKey { uri: self.uri.clone(), tls: self.tls.clone(), _client: PhantomData }
    }
}

impl<C> PartialEq for ClientKey<C> {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri && self.tls.as_ref().map(|tls| &tls.roots) == other.tls.as_ref().map(|tls| &tls.roots)
    }
}

impl<C> Eq for ClientKey<C> {}

impl<C> Hash for ClientKey<C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uri.hash(state);
        self.tls.as_ref().map(|tls| &tls.roots).hash(state);
    }
}

/// The parsed endpoint, bound unexported beside the client it builds.
struct ClientEndpoint<C> {
    endpoint: Endpoint,
    tls: Option<ClientTls>,
    _client: PhantomData<fn() -> C>,
}

impl<C: GrpcClient> ClientEndpoint<C> {
    fn parse(endpoint: &GrpcEndpoint) -> Result<Self, BoxError> {
        let parsed = Endpoint::from_shared(endpoint.uri.clone())
            .map_err(|error| BoxError::from(format!("`{}` is not a gRPC endpoint URI: {error}", endpoint.uri)))?;
        let https = parsed.uri().scheme_str() == Some("https");
        let tls = endpoint.tls.clone().or_else(|| https.then(ClientTls::system_roots));
        Ok(ClientEndpoint { endpoint: parsed, tls, _client: PhantomData })
    }

    /// The client over a lazily connected channel. Loading the platform's roots reads the trust
    /// store, so it happens here, when the app connects, and not where the module registers.
    fn connect(&self) -> Result<C, BoxError> {
        let endpoint = match &self.tls {
            Some(tls) => self.endpoint.clone().tls_config(tls.config())?,
            None => self.endpoint.clone(),
        };
        Ok(C::from_channel(endpoint.connect_lazy()))
    }
}
