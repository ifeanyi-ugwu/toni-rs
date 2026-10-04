//! The gRPC server (transports DESIGN §6.2): HTTP/2 over `ulo-hyper-serve`, so endpoints, inherited
//! sockets and TLS have one story with HTTP's. tonic's own `transport::Server` is not used.

use ulo::{Bound, BoundAddr, BoxError, DrainToken, Mounted};
use ulo_net::{EndpointSpec, Tls};
use ulo_transport::Count;

use crate::transport::Grpc;

/// The core's `Server` for [`Grpc`]: `app.bind(ulo_grpc::Server::new("0.0.0.0:50051"))`.
///
/// `prepare` builds the path table from the mounted handlers, refusing two handlers for one path,
/// builds the pre-dispatch stage from `PreDispatch<Grpc>`, refuses `Count::Max(0)` on each count
/// and `Bound::After(Duration::ZERO)` on `handshake_timeout`, resolves the endpoints and loads TLS
/// with ALPN `h2`. Health reports SERVING for every known service after `bind` and NOT_SERVING
/// from the drain on. Reflection, `grpc.reflection.v1` and `v1alpha`, is served in debug builds
/// and behind `reflection(true)` in release builds.
pub struct Server {
    pub(crate) endpoints: Vec<EndpointSpec>,
    pub(crate) tls: Option<Tls>,
    pub(crate) max_inflight: Count,
    pub(crate) max_per_connection: Count,
    pub(crate) max_concurrent_streams: Count,
    pub(crate) handshake_timeout: Bound,
    pub(crate) reflection: bool,
    pub(crate) descriptor_sets: Vec<&'static [u8]>,
}

impl Server {
    /// A server on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Server {
            endpoints: vec![endpoint.into()],
            tls: None,
            max_inflight: Count::Default,
            max_per_connection: Count::Default,
            max_concurrent_streams: Count::Default,
            handshake_timeout: Bound::Default,
            reflection: cfg!(debug_assertions),
            descriptor_sets: Vec::new(),
        }
    }

    /// One more endpoint the same server listens on.
    pub fn endpoint(mut self, endpoint: impl Into<EndpointSpec>) -> Self {
        self.endpoints.push(endpoint.into());
        self
    }

    /// TLS on every endpoint, ALPN `h2`, loaded in `prepare`.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Calls in flight across the server: unbounded at `Count::Default`; over it a call is
    /// answered UNAVAILABLE, which clients treat as retryable. `Count::Max(0)` is refused.
    pub fn max_inflight(mut self, calls: Count) -> Self {
        self.max_inflight = calls;
        self
    }

    /// Calls in flight per connection, refused UNAVAILABLE over it. `Count::Max(0)` is refused.
    pub fn max_per_connection(mut self, calls: Count) -> Self {
        self.max_per_connection = calls;
        self
    }

    /// `SETTINGS_MAX_CONCURRENT_STREAMS` per connection, excess streams refused with
    /// `REFUSED_STREAM`: hyper's own at `Count::Default`. `Count::Max(0)` is refused.
    pub fn max_concurrent_streams(mut self, streams: Count) -> Self {
        self.max_concurrent_streams = streams;
        self
    }

    /// How long a TLS handshake may take: 30 seconds at `Bound::Default`.
    /// `Bound::After(Duration::ZERO)` is refused in `prepare`.
    pub fn handshake_timeout(mut self, timeout: Bound) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    /// Serves reflection in a release build; on in debug builds unset.
    pub fn reflection(mut self, enabled: bool) -> Self {
        self.reflection = enabled;
        self
    }

    /// An encoded file descriptor set reflection serves, as `include_proto!` exposes it:
    /// `.file_descriptor_set(pb::FILE_DESCRIPTOR_SET)`.
    pub fn file_descriptor_set(mut self, encoded: &'static [u8]) -> Self {
        self.descriptor_sets.push(encoded);
        self
    }
}

impl ulo::Server for Server {
    type Transport = Grpc;

    async fn prepare(&mut self, mounted: Mounted<'_, Grpc>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!()
    }

    async fn bind(&mut self, mounted: Mounted<'_, Grpc>) -> Result<(), BoxError> {
        let _ = mounted;
        todo!()
    }

    async fn serve(&self) -> Result<(), BoxError> {
        todo!()
    }

    /// GOAWAY through `ulo-hyper-serve`'s drain; health switches to NOT_SERVING.
    async fn drain(&self, token: DrainToken) {
        let _ = token;
        todo!()
    }

    async fn close(&self) -> Result<(), BoxError> {
        todo!()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        todo!()
    }
}
