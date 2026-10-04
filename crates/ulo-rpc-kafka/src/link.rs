use ulo::{AppHandle, BoxError};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};

/// The Kafka link.
pub struct Kafka {
    pub(crate) brokers: String,
    pub(crate) group: Option<String>,
    pub(crate) reply_topic: Option<String>,
    pub(crate) codec: Codec,
}

impl Kafka {
    /// The link on `brokers`, a comma-separated `host:port` list, connected lazily.
    pub fn brokers(brokers: impl Into<String>) -> Self {
        Kafka { brokers: brokers.into(), group: None, reply_topic: None, codec: Codec::Json }
    }

    /// The consumer group server instances share.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The client's reply topic, one per logical client and reused across restarts; unset, one per
    /// process.
    pub fn reply_topic(mut self, topic: impl Into<String>) -> Self {
        self.reply_topic = Some(topic.into());
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }
}

impl Link for Kafka {
    const NAME: &'static str = "kafka";

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
