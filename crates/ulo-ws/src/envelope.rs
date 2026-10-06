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
//! setting. Under `codec = msgpack` the same keys travel as a MessagePack map in a binary frame.

use std::collections::HashMap;
use std::fmt;

use bytes::Bytes;
use serde::de::{self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use tokio_tungstenite::tungstenite::Message;
use ulo::BoxError;
use ulo_transport::{CallError, Details, ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Validate};

use crate::codec::Codec;
use crate::transport::{Reply, Ws, WsCx};

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

    pub(crate) fn into_message(self) -> Message {
        match self {
            Frame::Text(text) => Message::text(text),
            Frame::Binary(bytes) => Message::binary(bytes),
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

/// A frame as a handler's answer: the envelope's `data` as it stands, which on a JSON gateway is
/// JSON text and on a MessagePack gateway MessagePack bytes. A frame of the other kind fails the
/// answer as an internal error when it is written.
impl IntoReply<Ws> for Frame {
    fn into_reply(self, cx: &WsCx) -> Result<Reply, IntoReplyError> {
        let _ = cx;
        Ok(Reply::One(self))
    }
}

/// A message's `id`: any JSON scalar, kept as the client wrote it so the answer echoes it byte
/// for byte, and compared by equality.
///
/// On a MessagePack gateway the id is kept as the JSON text of the scalar it decoded to, and
/// written back as that scalar: equal by value, not by bytes.
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
        let decoded: Result<Option<T>, BoxError> = match &cx.inner.data {
            Data::Absent => Ok(None),
            Data::Json(raw) => serde_json::from_str::<T>(raw.get()).map(Some).map_err(BoxError::from),
            Data::MsgPack(frame) => match rmp_serde::from_slice::<DataField<T>>(frame) {
                Ok(DataField { data: Field::Present(value) }) => Ok(Some(value)),
                Ok(DataField { data: Field::Absent }) => Ok(None),
                Err(error) => Err(BoxError::from(error)),
            },
        };
        match decoded {
            Ok(Some(value)) => Ok(Payload(value)),
            Ok(None) => Err(ExtractError::Missing { param: "data" }),
            Err(error) => Err(ExtractError::Malformed { param: "data", source: cx.app().redact(error) }),
        }
    }
}

/// `Valid<Payload<T>>` checks the `T`.
impl<T: Validate> Validate for Payload<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// A received message's `data`, kept undecoded until a handler reads it.
pub(crate) enum Data {
    Absent,
    /// The `data` value's JSON text.
    Json(Box<RawValue>),
    /// The whole MessagePack message, decoded again with only its `data` key typed.
    MsgPack(Bytes),
}

/// The keys of a received message the gateway routes by.
pub(crate) struct Head {
    pub(crate) event: String,
    pub(crate) id: Option<MessageId>,
    pub(crate) data: Data,
}

/// A message the envelope cannot route: answered with the `error` envelope, kind `bad_request`,
/// carrying the `id` when one was read.
pub(crate) struct BadMessage {
    pub(crate) id: Option<MessageId>,
    pub(crate) reason: String,
}

/// The routing keys of `frame`, a data message of the kind `codec` reads.
pub(crate) fn parse(codec: Codec, event_field: &str, frame: &Frame) -> Result<Head, BadMessage> {
    match (codec, frame) {
        (Codec::Json, Frame::Text(text)) => parse_json(event_field, text),
        (Codec::MsgPack, Frame::Binary(bytes)) => parse_msgpack(event_field, bytes),
        _ => Err(BadMessage { id: None, reason: "the frame is not of the kind the gateway's codec reads".to_owned() }),
    }
}

fn parse_json(event_field: &str, text: &str) -> Result<Head, BadMessage> {
    let mut fields: HashMap<String, Box<RawValue>> = serde_json::from_str(text)
        .map_err(|_| BadMessage { id: None, reason: "a message is a JSON object".to_owned() })?;
    let id = match fields.remove("id") {
        None => None,
        Some(raw) => json_id(&raw).map_err(|reason| BadMessage { id: None, reason })?,
    };
    let event = match fields.remove(event_field) {
        Some(raw) => serde_json::from_str::<String>(raw.get())
            .map_err(|_| BadMessage { id: id.clone(), reason: format!("`{event_field}` is a string") })?,
        None => return Err(BadMessage { id, reason: format!("a message names its event in `{event_field}`") }),
    };
    let data = fields.remove("data").map_or(Data::Absent, Data::Json);
    Ok(Head { event, id, data })
}

/// A JSON scalar's text as the id; `null` is no id.
fn json_id(raw: &RawValue) -> Result<Option<MessageId>, String> {
    let text = raw.get().trim();
    match text.as_bytes().first() {
        Some(b'{' | b'[') => Err("`id` is a JSON scalar: a string, a number or a boolean".to_owned()),
        _ if text == "null" => Ok(None),
        _ => Ok(Some(MessageId(text.to_owned()))),
    }
}

fn parse_msgpack(event_field: &str, bytes: &Bytes) -> Result<Head, BadMessage> {
    let mut deserializer = rmp_serde::Deserializer::new(&bytes[..]);
    let read = HeadSeed { event_field }
        .deserialize(&mut deserializer)
        .map_err(|_| BadMessage { id: None, reason: "a message is a MessagePack map keyed by strings".to_owned() })?;
    let id = match read.id {
        None | Some(IdScalar::Null) => None,
        Some(scalar) => match serde_json::to_string(&scalar) {
            Ok(text) => Some(MessageId(text)),
            Err(_) => return Err(BadMessage { id: None, reason: "`id` is a string, a number or a boolean".to_owned() }),
        },
    };
    let Some(event) = read.event else {
        return Err(BadMessage { id, reason: format!("a message names its event in `{event_field}`") });
    };
    let data = if read.data { Data::MsgPack(bytes.clone()) } else { Data::Absent };
    Ok(Head { event, id, data })
}

/// What one pass over a MessagePack message reads: the event, the id, and whether `data` is
/// present.
struct HeadRead {
    event: Option<String>,
    id: Option<IdScalar>,
    data: bool,
}

struct HeadSeed<'a> {
    event_field: &'a str,
}

impl<'de> DeserializeSeed<'de> for HeadSeed<'_> {
    type Value = HeadRead;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<HeadRead, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for HeadSeed<'_> {
    type Value = HeadRead;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map keyed by strings")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<HeadRead, A::Error> {
        let mut read = HeadRead { event: None, id: None, data: false };
        while let Some(key) = map.next_key::<String>()? {
            if key == self.event_field {
                read.event = Some(map.next_value::<String>()?);
            } else if key == "id" {
                read.id = Some(map.next_value::<IdScalar>()?);
            } else {
                read.data |= key == "data";
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(read)
    }
}

/// An `id` as a MessagePack message carries it: a scalar, or nil for none.
enum IdScalar {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
}

impl<'de> Deserialize<'de> for IdScalar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(IdVisitor)
    }
}

struct IdVisitor;

impl<'de> Visitor<'de> for IdVisitor {
    type Value = IdScalar;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string, a number, a boolean or nil")
    }

    fn visit_unit<E: de::Error>(self) -> Result<IdScalar, E> {
        Ok(IdScalar::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<IdScalar, E> {
        Ok(IdScalar::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<IdScalar, E> {
        Ok(IdScalar::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<IdScalar, E> {
        Ok(IdScalar::I64(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<IdScalar, E> {
        Ok(IdScalar::U64(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<IdScalar, E> {
        Ok(IdScalar::F64(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<IdScalar, E> {
        Ok(IdScalar::Str(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<IdScalar, E> {
        Ok(IdScalar::Str(value))
    }
}

impl Serialize for IdScalar {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            IdScalar::Null => serializer.serialize_unit(),
            IdScalar::Bool(value) => serializer.serialize_bool(*value),
            IdScalar::I64(value) => serializer.serialize_i64(*value),
            IdScalar::U64(value) => serializer.serialize_u64(*value),
            IdScalar::F64(value) => serializer.serialize_f64(*value),
            IdScalar::Str(value) => serializer.serialize_str(value),
        }
    }
}

/// A MessagePack message decoded with only its `data` key typed, the other keys skipped.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct DataField<T> {
    #[serde(default)]
    data: Field<T>,
}

/// `data` present, its `nil` included, or absent.
enum Field<T> {
    Absent,
    Present(T),
}

impl<T> Default for Field<T> {
    fn default() -> Self {
        Field::Absent
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Field::Present)
    }
}

/// `{"id":id,"data":data}`, or `{"data":data}` without an id. `data` is already encoded by the
/// codec, and a frame of the other kind is refused.
pub(crate) fn data(codec: Codec, id: Option<&MessageId>, data: &Frame) -> Result<Message, BoxError> {
    match (codec, data) {
        (Codec::Json, Frame::Text(text)) => {
            let mut out = String::with_capacity(text.len() + 32);
            out.push('{');
            if let Some(id) = id {
                out.push_str("\"id\":");
                out.push_str(id.as_str());
                out.push(',');
            }
            out.push_str("\"data\":");
            out.push_str(text);
            out.push('}');
            Ok(Message::text(out))
        }
        (Codec::MsgPack, Frame::Binary(bytes)) => {
            let mut out = Vec::with_capacity(bytes.len() + 32);
            map_header(&mut out, if id.is_some() { 2 } else { 1 });
            if let Some(id) = id {
                write_str(&mut out, "id");
                write_id(&mut out, id)?;
            }
            write_str(&mut out, "data");
            out.extend_from_slice(bytes);
            Ok(Message::binary(out))
        }
        (Codec::Json, Frame::Binary(_)) => Err(BoxError::from("a JSON gateway's reply data is a text frame")),
        (Codec::MsgPack, Frame::Text(_)) => Err(BoxError::from("a MessagePack gateway's reply data is a binary frame")),
    }
}

/// `{"id":id,"complete":true}`: a streamed answer's end, and the answer to a message with an id
/// whose handler returned nothing.
pub(crate) fn complete(codec: Codec, id: Option<&MessageId>) -> Message {
    match codec {
        Codec::Json => match id {
            Some(id) => Message::text(format!("{{\"id\":{},\"complete\":true}}", id.as_str())),
            None => Message::text("{\"complete\":true}"),
        },
        Codec::MsgPack => {
            let mut out = Vec::with_capacity(24);
            let id = id.and_then(|id| {
                let mut encoded = Vec::new();
                write_id(&mut encoded, id).ok().map(|()| encoded)
            });
            map_header(&mut out, if id.is_some() { 2 } else { 1 });
            if let Some(id) = id {
                write_str(&mut out, "id");
                out.extend_from_slice(&id);
            }
            write_str(&mut out, "complete");
            out.push(0xc3);
            Message::binary(out)
        }
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    kind: &'a str,
    message: &'a str,
    details: &'a Details,
}

/// `{"id":id,"error":{"kind","message","details"}}`, or without the id.
pub(crate) fn error(codec: Codec, id: Option<&MessageId>, error: &CallError) -> Message {
    let body = ErrorBody { kind: error.kind().as_str(), message: error.message(), details: error.details() };
    match codec {
        Codec::Json => {
            let body = serde_json::to_string(&body)
                .unwrap_or_else(|_| "{\"kind\":\"internal\",\"message\":\"internal error\",\"details\":[]}".to_owned());
            match id {
                Some(id) => Message::text(format!("{{\"id\":{},\"error\":{body}}}", id.as_str())),
                None => Message::text(format!("{{\"error\":{body}}}")),
            }
        }
        Codec::MsgPack => {
            let mut out = Vec::with_capacity(64);
            let id = id.and_then(|id| {
                let mut encoded = Vec::new();
                write_id(&mut encoded, id).ok().map(|()| encoded)
            });
            map_header(&mut out, if id.is_some() { 2 } else { 1 });
            if let Some(id) = id {
                write_str(&mut out, "id");
                out.extend_from_slice(&id);
            }
            write_str(&mut out, "error");
            match rmp_serde::to_vec_named(&body) {
                Ok(encoded) => out.extend_from_slice(&encoded),
                Err(_) => {
                    map_header(&mut out, 2);
                    write_str(&mut out, "kind");
                    write_str(&mut out, "internal");
                    write_str(&mut out, "message");
                    write_str(&mut out, "internal error");
                }
            }
            Message::binary(out)
        }
    }
}

/// A broadcast as the adapter carries it between processes, whatever each gateway's codec:
/// `{"event":event,"data":data}` in JSON. Each process re-encodes it per gateway on delivery.
pub(crate) fn broadcast<T: Serialize + ?Sized>(event: &str, data: &T) -> Result<Bytes, serde_json::Error> {
    #[derive(Serialize)]
    struct Carried<'a, T: ?Sized> {
        event: &'a str,
        data: &'a T,
    }
    serde_json::to_vec(&Carried { event, data }).map(Bytes::from)
}

/// A carried broadcast read back: its event and its data's JSON text.
pub(crate) struct Carried {
    pub(crate) event: String,
    pub(crate) data: Box<RawValue>,
}

pub(crate) fn read_broadcast(bytes: &[u8]) -> Result<Carried, serde_json::Error> {
    #[derive(Deserialize)]
    struct Read {
        event: String,
        data: Box<RawValue>,
    }
    let Read { event, data } = serde_json::from_slice(bytes)?;
    Ok(Carried { event, data })
}

/// `{<event field>:event,"data":data}` for a gateway, from a carried broadcast.
pub(crate) fn event(codec: Codec, event_field: &str, carried: &Carried) -> Result<Message, BoxError> {
    match codec {
        Codec::Json => {
            let field = serde_json::to_string(event_field)?;
            let event = serde_json::to_string(&carried.event)?;
            Ok(Message::text(format!("{{{field}:{event},\"data\":{}}}", carried.data.get())))
        }
        Codec::MsgPack => {
            let data: serde_json::Value = serde_json::from_str(carried.data.get())?;
            let mut out = Vec::with_capacity(64);
            map_header(&mut out, 2);
            write_str(&mut out, event_field);
            write_str(&mut out, &carried.event);
            write_str(&mut out, "data");
            out.extend_from_slice(&rmp_serde::to_vec_named(&data)?);
            Ok(Message::binary(out))
        }
    }
}

/// A MessagePack map header for `len` entries; the envelope has at most three.
fn map_header(out: &mut Vec<u8>, len: u8) {
    out.push(0x80 | (len & 0x0f));
}

/// A MessagePack string.
fn write_str(out: &mut Vec<u8>, text: &str) {
    let len = text.len();
    if len < 32 {
        out.push(0xa0 | len as u8);
    } else if let Ok(len) = u8::try_from(len) {
        out.push(0xd9);
        out.push(len);
    } else if let Ok(len) = u16::try_from(len) {
        out.push(0xda);
        out.extend_from_slice(&len.to_be_bytes());
    } else {
        out.push(0xdb);
        out.extend_from_slice(&u32::try_from(len).unwrap_or(u32::MAX).to_be_bytes());
    }
    out.extend_from_slice(text.as_bytes());
}

/// An id, kept as JSON text, written as the MessagePack scalar it names.
fn write_id(out: &mut Vec<u8>, id: &MessageId) -> Result<(), BoxError> {
    let value: serde_json::Value = serde_json::from_str(id.as_str())?;
    out.extend_from_slice(&rmp_serde::to_vec(&value)?);
    Ok(())
}
