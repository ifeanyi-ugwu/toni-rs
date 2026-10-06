//! The codecs a gateway's messages travel in (transports DESIGN §4.2): JSON in text frames, and
//! MessagePack through `rmp-serde` in binary frames under `codec = msgpack`.

use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::BoxError;

use crate::envelope::Frame;

/// A gateway's codec: `Json` unset, `MsgPack` under `codec = msgpack`.
///
/// MessagePack writes a struct as a map keyed by field name (`rmp_serde::to_vec_named`), the form
/// a client library in another language reads without knowing the field order.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Codec {
    #[default]
    Json,
    MsgPack,
}

impl Codec {
    /// `value` encoded as one frame: text for JSON, binary for MessagePack.
    pub fn encode<T: Serialize + ?Sized>(self, value: &T) -> Result<Frame, BoxError> {
        match self {
            Codec::Json => Ok(Frame::Text(serde_json::to_string(value)?)),
            Codec::MsgPack => Ok(Frame::Binary(Bytes::from(rmp_serde::to_vec_named(value)?))),
        }
    }

    /// `T` decoded from `frame`; a frame of the other kind is an error.
    pub fn decode<T: DeserializeOwned>(self, frame: &Frame) -> Result<T, BoxError> {
        match (self, frame) {
            (Codec::Json, Frame::Text(text)) => Ok(serde_json::from_str(text)?),
            (Codec::MsgPack, Frame::Binary(bytes)) => Ok(rmp_serde::from_slice(bytes)?),
            (Codec::Json, Frame::Binary(_)) => Err(BoxError::from("a JSON gateway's messages are text frames")),
            (Codec::MsgPack, Frame::Text(_)) => Err(BoxError::from("a MessagePack gateway's messages are binary frames")),
        }
    }

    /// Whether `frame` is of the kind this codec reads: text for JSON, binary for MessagePack.
    pub(crate) fn reads(self, frame: &Frame) -> bool {
        matches!((self, frame), (Codec::Json, Frame::Text(_)) | (Codec::MsgPack, Frame::Binary(_)))
    }
}
