use ulo::{AppHandle, BoundAddr, BoxError};
use ulo_net::EndpointSpec;
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};

/// The UDP link: a server binds its endpoint, a client sends to it.
pub struct Udp {
    pub(crate) endpoint: EndpointSpec,
    pub(crate) codec: Codec,
}

impl Udp {
    /// The link on `endpoint`: text parsed in `prepare`, or a `SocketAddr`. An inherited endpoint
    /// is refused in `prepare`, since the activation adopts listening TCP sockets alone.
    pub fn new(endpoint: impl Into<EndpointSpec>) -> Self {
        Udp { endpoint: endpoint.into(), codec: Codec::Json }
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }
}

impl Link for Udp {
    const NAME: &'static str = "udp";

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
