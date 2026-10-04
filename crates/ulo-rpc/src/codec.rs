//! The codecs a link's frames travel in (transports DESIGN §5.2): JSON at `Default`, readable from
//! any client, and CBOR through `ciborium` under `codec = cbor`, which carries raw bytes.

use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::BoxError;

use crate::frame::{Data, Frame};

/// A link's codec, chosen on its builder: `Json` unset, declaring `binary: false`, or `Cbor`,
/// declaring `binary: true`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Codec {
    #[default]
    Json,
    Cbor,
}

impl Codec {
    /// Whether the codec carries raw bytes, `Capabilities::binary`.
    pub fn binary(self) -> bool {
        matches!(self, Codec::Cbor)
    }

    /// One whole frame, as TCP, UDP and Redis carry it and every reply lane does.
    pub fn encode_frame(self, frame: &Frame) -> Result<Bytes, BoxError> {
        let _ = frame;
        todo!()
    }

    pub fn decode_frame(self, bytes: &[u8]) -> Result<Frame, BoxError> {
        let _ = bytes;
        todo!()
    }

    /// A payload: `value` encoded as a frame's `d`.
    pub fn encode<T: Serialize + ?Sized>(self, value: &T) -> Result<Data, BoxError> {
        let _ = value;
        todo!()
    }

    pub fn decode<T: DeserializeOwned>(self, data: &Data) -> Result<T, BoxError> {
        let _ = data;
        todo!()
    }
}
