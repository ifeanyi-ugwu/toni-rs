use ulo::{AppHandle, BoxError};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};

/// The Redis link.
pub struct Redis {
    pub(crate) url: String,
    pub(crate) codec: Codec,
}

impl Redis {
    /// The link on `url`, `redis://host:6379` or `rediss://host:6380`, parsed in `prepare` and
    /// connected lazily.
    pub fn url(url: impl Into<String>) -> Self {
        Redis { url: url.into(), codec: Codec::Json }
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }
}

impl Link for Redis {
    const NAME: &'static str = "redis";

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
