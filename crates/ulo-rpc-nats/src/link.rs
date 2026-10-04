use ulo::{AppHandle, BoxError};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};

/// The NATS link.
pub struct Nats {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) codec: Codec,
}

impl Nats {
    /// The link on `url`, `nats://host:4222` or `tls://host:4222`, parsed in `prepare` and
    /// connected lazily: the server side at `bind`, the client side on its first call.
    pub fn url(url: impl Into<String>) -> Self {
        Nats { url: url.into(), group: None, codec: Codec::Json }
    }

    /// The queue group server instances share, in place of the root module's full type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }
}

impl Link for Nats {
    const NAME: &'static str = "nats";

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
}
