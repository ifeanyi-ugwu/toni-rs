//! Client modules (transports DESIGN §6.3): a tonic-generated client bound under its own type, its
//! channel connected lazily, so `connect` does no network I/O.
//!
//! ```ignore
//! imports = [GrpcClientModule::<pb::UserServiceClient<Channel>>::for_root(
//!     GrpcEndpoint::new("https://users:443").tls(ClientTls::system_roots()),
//! ).keyed::<UsersApi>()]
//! ```

use std::marker::PhantomData;

use tonic::transport::Channel;
use ulo::{ExecutionRef, Module, ModuleDef, ModuleIdentity};

/// A tonic client built over a channel. `ulo-build` writes the impl for every client it
/// generates, `UserServiceClient::new(channel)`.
pub trait GrpcClient: Send + Sync + 'static {
    fn from_channel(channel: Channel) -> Self;
}

/// Binds client `C`, exported, usually keyed so two endpoints of one service coexist:
/// `Dep<pb::UserServiceClient<Channel>, UsersApi>`.
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
        todo!()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = (m, &self.endpoint);
        todo!()
    }
}

/// Where a client connects: the URI and its TLS. The URI is parsed when the app wires, so a bad
/// one is reported with the wiring errors.
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

#[derive(Clone, Debug)]
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
}

/// `request` with `grpc-timeout` set from `exec`'s remaining deadline. There is no ambient
/// execution, so nothing forwards a deadline on its own.
pub fn outgoing<T>(exec: &ExecutionRef, request: tonic::Request<T>) -> tonic::Request<T> {
    let _ = (exec, &request);
    todo!()
}
