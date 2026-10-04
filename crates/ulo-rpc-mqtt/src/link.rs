use ulo::{AppHandle, BoxError};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Codec, Link, Outbound, Pattern};

/// The MQTT v5 link.
pub struct Mqtt {
    pub(crate) url: String,
    pub(crate) group: Option<String>,
    pub(crate) qos: QoS,
    pub(crate) codec: Codec,
}

impl Mqtt {
    /// The link on `url`, `mqtt://host:1883` or `mqtts://host:8883`, parsed in `prepare` and
    /// connected lazily.
    pub fn url(url: impl Into<String>) -> Self {
        Mqtt { url: url.into(), group: None, qos: QoS::AtLeastOnce, codec: Codec::Json }
    }

    /// The shared-subscription group server instances share, in place of the root module's full
    /// type path.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// The QoS every publish and subscription uses: `AtLeastOnce` unset. At `AtMostOnce` a miss
    /// has no signal and is the client's `Timeout`.
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }

    /// `Codec::Cbor` carries raw bytes and declares `binary: true`; JSON unset.
    pub fn codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
    }
}

/// An MQTT quality of service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QoS {
    /// QoS 0: nothing is acknowledged, so a miss has no signal.
    AtMostOnce,
    /// QoS 1, with PUBACK.
    #[default]
    AtLeastOnce,
    /// QoS 2, with PUBREC.
    ExactlyOnce,
}

impl Link for Mqtt {
    const NAME: &'static str = "mqtt";

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
