use ulo::{AppHandle, BoundAddr, BoxError};
use ulo_net::{EndpointSpec, Tls};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};

/// The TCP link: a server binds its endpoint, a client connects to it.
pub struct Tcp {
    pub(crate) endpoint: EndpointSpec,
    pub(crate) tls: Option<Tls>,
    pub(crate) codec: Codec,
    pub(crate) max_frame: Option<u64>,
}

impl Tcp {
    /// The link on `endpoint`: text parsed in `prepare`, an `Endpoint`, or a `SocketAddr`. A
    /// server may listen on an inherited socket; a client connects to an address.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Tcp { endpoint: endpoint.into(), tls: None, codec: Codec::Json, max_frame: None }
    }

    /// TLS on the server's endpoint, loaded in `prepare`, with no ALPN.
    pub fn tls(mut self, tls: Tls) -> Self {
        self.tls = Some(tls);
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }

    /// The largest frame, in bytes, read or written; a larger length prefix closes the connection
    /// before the body is read.
    pub fn max_frame(mut self, bytes: u64) -> Self {
        self.max_frame = Some(bytes);
        self
    }
}

impl Link for Tcp {
    const NAME: &'static str = "tcp";

    fn capabilities(&self) -> Capabilities {
        todo!()
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        let _ = app;
        todo!()
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let _ = patterns;
        todo!()
    }

    async fn connect(&self) -> Result<Outbound, BoxError> {
        todo!()
    }

    async fn drain(&self) {
        todo!()
    }

    async fn close(&self) -> Result<(), BoxError> {
        todo!()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        todo!()
    }
}
