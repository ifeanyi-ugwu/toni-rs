//! The codecs a gateway's messages travel in (transports DESIGN §4.2): JSON in text frames, and
//! MessagePack through `rmp-serde` in binary frames under `codec = msgpack`.

use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::BoxError;

use crate::envelope::Frame;

/// A gateway's codec: `Json` unset, `MsgPack` under `codec = msgpack`.
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
        let _ = value;
        todo!()
    }

    /// `T` decoded from `frame`; a frame of the other kind is an error.
    pub fn decode<T: DeserializeOwned>(self, frame: &Frame) -> Result<T, BoxError> {
        let _ = frame;
        todo!()
    }
}
