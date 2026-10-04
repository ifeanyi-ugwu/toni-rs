//! The frame grammar, defined here since no specification governs one (transports DESIGN §5.2).
//! `t` carries the kind's name:
//!
//! ```text
//! req     {t:"req",id,p,h,d}      one request, unary or server-streaming
//! evt     {t:"evt",p,h,d}         event, no reply
//! res     {t:"res",id,d}          single reply
//! err     {t:"err",id,e:{kind,message,details}}
//! item    {t:"item",id,d}         reply-stream item
//! end     {t:"end",id}            reply stream ended cleanly
//! open    {t:"open",id,p,h}       start a streamed request
//! in      {t:"in",id,d}           request-stream item
//! in_end  {t:"in_end",id}         request stream ended
//! cancel  {t:"cancel",id}         caller gave up: fires ClientCancelled
//! credit  {t:"credit",id,n}       flow control, reserved
//! goaway  {t:"goaway"}            server draining: no new calls on this connection
//! ```
//!
//! `id` is a `u64` the caller allocates per connection on TCP and UDP; a broker carries the
//! correlation natively. TCP, UDP and Redis carry the whole frame; on NATS, AMQP, MQTT and Kafka
//! the request lane carries `d` as the message body, `p` as the subject, queue or topic and `h` as
//! the broker's headers, and the reply lane carries `res`, `err`, `item` and `end` frames.

use std::fmt;

use bytes::Bytes;
use ulo_transport::{Details, ErrorKind};

use crate::transport::CallHeaders;

/// One frame of the grammar.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Req { id: u64, pattern: String, headers: CallHeaders, data: Data },
    Evt { pattern: String, headers: CallHeaders, data: Data },
    Res { id: u64, data: Data },
    Err { id: u64, error: ErrorBody },
    Item { id: u64, data: Data },
    End { id: u64 },
    Open { id: u64, pattern: String, headers: CallHeaders },
    In { id: u64, data: Data },
    InEnd { id: u64 },
    Cancel { id: u64 },
    /// Reserved: no link sends it, and every link honours the grammar that names it.
    Credit { id: u64, n: u64 },
    Goaway,
}

impl Frame {
    /// The frame's `t`: `"req"`, `"evt"`, `"res"`, ...
    pub fn kind(&self) -> &'static str {
        match self {
            Frame::Req { .. } => "req",
            Frame::Evt { .. } => "evt",
            Frame::Res { .. } => "res",
            Frame::Err { .. } => "err",
            Frame::Item { .. } => "item",
            Frame::End { .. } => "end",
            Frame::Open { .. } => "open",
            Frame::In { .. } => "in",
            Frame::InEnd { .. } => "in_end",
            Frame::Cancel { .. } => "cancel",
            Frame::Credit { .. } => "credit",
            Frame::Goaway => "goaway",
        }
    }

    /// The frame's `id`; `None` for `evt` and `goaway`.
    pub fn id(&self) -> Option<u64> {
        match self {
            Frame::Req { id, .. }
            | Frame::Res { id, .. }
            | Frame::Err { id, .. }
            | Frame::Item { id, .. }
            | Frame::End { id }
            | Frame::Open { id, .. }
            | Frame::In { id, .. }
            | Frame::InEnd { id }
            | Frame::Cancel { id }
            | Frame::Credit { id, .. } => Some(*id),
            Frame::Evt { .. } | Frame::Goaway => None,
        }
    }
}

/// A frame's payload, `d`, encoded by the link's codec: JSON text on a JSON link, a CBOR item on a
/// CBOR link. On a broker's request lane it is the message body as it stands, which is what lets
/// a broker's own tools reach a handler.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Data(pub(crate) Bytes);

impl Data {
    /// Payload bytes already encoded by the link's codec.
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Data(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Data").field(&self.0.len()).finish()
    }
}

/// An `err` frame's `e`: the kind as its wire name (`"bad_request"`), the public message, and the
/// details (transports DESIGN §2.4).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct ErrorBody {
    pub kind: ErrorKind,
    pub message: String,
    pub details: Details,
}

impl ErrorBody {
    pub fn new(kind: ErrorKind, message: impl Into<String>, details: Details) -> Self {
        ErrorBody { kind, message: message.into(), details }
    }
}

/// What a handler's payload is, recorded on its handler value by the attribute's probe:
/// `Payload<Bytes>` and `Bytes` are `Binary`, every other type `Serde`. `prepare` refuses a
/// `Binary` handler on a link whose capabilities declare `binary: false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    #[default]
    Serde,
    Binary,
}
