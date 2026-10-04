//! The message envelope, defined here since no specification covers one (transports DESIGN §4.2):
//!
//! ```text
//! → {"event":"chat.send","id":7,"data":{...}}
//! ← {"id":7,"data":{...}}                                  single answer
//! ← {"id":7,"data":{...}} ... {"id":7,"complete":true}     streamed answer
//! ← {"id":7,"error":{"kind":"forbidden","message":"...","details":[...]}}
//! → {"event":"cancel","id":7}                              fires ClientCancelled
//! ```
//!
//! `cancel` is a reserved event name; `complete`, `error` and `data` are server-side keys. A
//! message without an `id` is fire-and-forget. The event field's name is the gateway's `event`
//! setting.

use std::fmt;

use bytes::Bytes;
use serde::de::DeserializeOwned;
use ulo_transport::{ExtractError, FromCall};

use crate::transport::{Ws, WsCx};

/// One data message's content: a text frame or a binary frame. In a handler's reply it is the
/// envelope's `data`, encoded by the gateway's codec; through `Connection::send` it is written as
/// one message as it stands, which is how a hand-written gateway speaks its own protocol.
#[derive(Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Bytes),
}

impl Frame {
    pub fn text(text: impl Into<String>) -> Frame {
        Frame::Text(text.into())
    }

    pub fn binary(bytes: impl Into<Bytes>) -> Frame {
        Frame::Binary(bytes.into())
    }

    /// `value` as JSON text.
    pub fn json<T: serde::Serialize + ?Sized>(value: &T) -> Result<Frame, serde_json::Error> {
        serde_json::to_string(value).map(Frame::Text)
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Frame::Text(text) => Some(text),
            Frame::Binary(_) => None,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Frame::Text(text) => text.as_bytes(),
            Frame::Binary(bytes) => bytes,
        }
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Frame::Text(text) => f.debug_tuple("Frame::Text").field(&text.len()).finish(),
            Frame::Binary(bytes) => f.debug_tuple("Frame::Binary").field(&bytes.len()).finish(),
        }
    }
}

/// A message's `id`: any JSON scalar, kept as the client wrote it so the answer echoes it byte
/// for byte, and compared by equality.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageId(pub(crate) String);

impl MessageId {
    /// The id's JSON text: `7`, `"a1"`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The message's `data`, decoded by the gateway's codec. A message without a `data` key fails as
/// `ExtractError::Missing { param: "data" }`, one that does not decode as `Malformed`. It consumes
/// the payload, so a handler takes one.
#[derive(Clone, Debug)]
pub struct Payload<T>(pub T);

impl<T> Payload<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Ws> for Payload<T> {
    const CONSUMES_BODY: bool = true;

    async fn from_call(cx: &WsCx) -> Result<Self, ExtractError> {
        let _ = cx;
        todo!()
    }
}
