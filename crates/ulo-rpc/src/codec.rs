//! The codecs a link's frames travel in (transports DESIGN §5.2): JSON at `Default`, readable from
//! any client, and CBOR through `ciborium` under `codec = cbor`, which carries raw bytes.
//!
//! A frame is a map keyed by the one-letter names of the grammar, `t` first. `d` is spliced in as
//! the payload item it already is: raw JSON text on a JSON link, a CBOR item on a CBOR link. `h`
//! is a map written in order, a repeated name written once per value, and read back the same way.
//! An empty payload is written as no `d` and read back from a missing or `null` `d`.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::time::Duration;

use bytes::Bytes;
use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use ulo::BoxError;
use ulo_transport::{Detail, Details, ErrorKind, FieldViolation, Link};

use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{Capabilities, FrameUnencodable};
use crate::transport::CallHeaders;

/// A link's codec, chosen on its builder: `Json` unset, declaring `binary: false`, or `Cbor`,
/// declaring `binary: true`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Codec {
    #[default]
    Json,
    Cbor,
}

/// CBOR's `null`, which an empty payload decodes as.
const CBOR_NULL: [u8; 1] = [0xf6];

impl Codec {
    /// Whether the codec carries raw bytes, `Capabilities::binary`.
    pub fn binary(self) -> bool {
        matches!(self, Codec::Cbor)
    }

    /// The codec a link's capabilities declare. The set is closed, so `binary` names it: a link
    /// carrying raw bytes speaks CBOR and one that does not speaks JSON.
    pub(crate) fn of(capabilities: &Capabilities) -> Codec {
        if capabilities.binary { Codec::Cbor } else { Codec::Json }
    }

    /// One whole frame, as TCP, UDP and Redis carry it and every reply lane does. A frame whose
    /// `d` is not an item of this codec, JSON bytes on a CBOR link or bytes that are no JSON on a
    /// JSON link, fails with [`FrameUnencodable`].
    pub fn encode_frame(self, frame: &Frame) -> Result<Bytes, FrameUnencodable> {
        self.write_frame(frame).map_err(|source| FrameUnencodable { codec: self, source })
    }

    fn write_frame(self, frame: &Frame) -> Result<Bytes, BoxError> {
        let mut out = Vec::new();
        match self {
            Codec::Json => {
                let d = match frame_data(frame) {
                    Some(data) if !data.is_empty() => Some(RawValue::from_string(String::from_utf8(data.0.to_vec())?)?),
                    _ => None,
                };
                serde_json::to_writer(&mut out, &Wire { frame, d: d.as_deref() })?;
            }
            Codec::Cbor => {
                let d = match frame_data(frame) {
                    Some(data) if !data.is_empty() => Some(ciborium::from_reader::<ciborium::Value, _>(data.as_bytes())?),
                    _ => None,
                };
                ciborium::into_writer(&Wire { frame, d: d.as_ref() }, &mut out)?;
            }
        }
        Ok(Bytes::from(out))
    }

    pub fn decode_frame(self, bytes: &[u8]) -> Result<Frame, BoxError> {
        match self {
            Codec::Json => {
                let wire: WireIn<Box<RawValue>> = serde_json::from_slice(bytes)?;
                wire.into_frame(|raw| Ok(Data(Bytes::copy_from_slice(raw.get().as_bytes()))))
            }
            Codec::Cbor => {
                let wire: WireIn<ciborium::Value> = ciborium::from_reader(bytes)?;
                wire.into_frame(|value| {
                    let mut out = Vec::new();
                    ciborium::into_writer(&value, &mut out)?;
                    Ok(Data(Bytes::from(out)))
                })
            }
        }
    }

    /// A payload: `value` encoded as a frame's `d`.
    pub fn encode<T: Serialize + ?Sized>(self, value: &T) -> Result<Data, BoxError> {
        self.encode_checked(value).map(|(data, _)| data)
    }

    pub fn decode<T: DeserializeOwned>(self, data: &Data) -> Result<T, BoxError> {
        match self {
            Codec::Json => {
                let bytes: &[u8] = if data.is_empty() { b"null" } else { data.as_bytes() };
                Ok(serde_json::from_slice(bytes)?)
            }
            Codec::Cbor => {
                let bytes: &[u8] = if data.is_empty() { &CBOR_NULL } else { data.as_bytes() };
                Ok(ciborium::from_reader(bytes)?)
            }
        }
    }

    /// [`encode`](Self::encode), answering too whether the value wrote raw bytes, which a JSON link
    /// carries only as an array of numbers: the client refuses such a payload before any I/O.
    pub(crate) fn encode_checked<T: Serialize + ?Sized>(self, value: &T) -> Result<(Data, bool), BoxError> {
        let mut out = Vec::new();
        let mut wrote_bytes = false;
        match self {
            Codec::Json => {
                let mut serializer = serde_json::Serializer::with_formatter(&mut out, Detecting { wrote_bytes: &mut wrote_bytes });
                value.serialize(&mut serializer)?;
            }
            Codec::Cbor => ciborium::into_writer(value, &mut out)?,
        }
        Ok((Data(Bytes::from(out)), wrote_bytes))
    }
}

/// serde_json's compact output, noting whether anything serialized raw bytes.
struct Detecting<'a> {
    wrote_bytes: &'a mut bool,
}

impl serde_json::ser::Formatter for Detecting<'_> {
    fn write_byte_array<W>(&mut self, writer: &mut W, value: &[u8]) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        *self.wrote_bytes = true;
        serde_json::ser::CompactFormatter.write_byte_array(writer, value)
    }
}

fn frame_data(frame: &Frame) -> Option<&Data> {
    match frame {
        Frame::Req { data, .. } | Frame::Evt { data, .. } | Frame::Res { data, .. } | Frame::Item { data, .. } | Frame::In { data, .. } => {
            Some(data)
        }
        _ => None,
    }
}

/// A frame as written, `d` already in the codec's own item type.
struct Wire<'a, D: ?Sized> {
    frame: &'a Frame,
    d: Option<&'a D>,
}

impl<D: Serialize + ?Sized> Serialize for Wire<'_, D> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let d = usize::from(self.d.is_some());
        let len = 1 + match self.frame {
            Frame::Req { .. } => 3 + d,
            Frame::Evt { .. } | Frame::Open { .. } => 2 + d,
            Frame::Res { .. } | Frame::Item { .. } | Frame::In { .. } => 1 + d,
            Frame::Err { .. } | Frame::Credit { .. } => 2,
            Frame::End { .. } | Frame::InEnd { .. } | Frame::Cancel { .. } => 1,
            Frame::Goaway => 0,
        };
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("t", self.frame.kind())?;
        match self.frame {
            Frame::Req { id, pattern, headers, .. } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("p", pattern)?;
                map.serialize_entry("h", &HeadersOut(headers))?;
            }
            Frame::Evt { pattern, headers, .. } => {
                map.serialize_entry("p", pattern)?;
                map.serialize_entry("h", &HeadersOut(headers))?;
            }
            Frame::Open { id, pattern, headers } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("p", pattern)?;
                map.serialize_entry("h", &HeadersOut(headers))?;
            }
            Frame::Res { id, .. } | Frame::Item { id, .. } | Frame::In { id, .. } => map.serialize_entry("id", id)?,
            Frame::Err { id, error } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("e", &ErrorOut(error))?;
            }
            Frame::Credit { id, n } => {
                map.serialize_entry("id", id)?;
                map.serialize_entry("n", n)?;
            }
            Frame::End { id } | Frame::InEnd { id } | Frame::Cancel { id } => map.serialize_entry("id", id)?,
            Frame::Goaway => {}
        }
        if let Some(d) = self.d {
            map.serialize_entry("d", d)?;
        }
        map.end()
    }
}

struct HeadersOut<'a>(&'a CallHeaders);

impl Serialize for HeadersOut<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in self.0.iter() {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

struct ErrorOut<'a>(&'a ErrorBody);

impl Serialize for ErrorOut<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("kind", self.0.kind.as_str())?;
        map.serialize_entry("message", &self.0.message)?;
        map.serialize_entry("details", &self.0.details)?;
        map.end()
    }
}

/// A frame as read. Unknown members are ignored, so a later grammar may add one.
#[derive(Deserialize)]
#[serde(bound(deserialize = "D: Deserialize<'de>"))]
struct WireIn<D> {
    t: String,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    p: Option<String>,
    #[serde(default)]
    h: Option<HeadersIn>,
    #[serde(default = "Option::default")]
    d: Option<D>,
    #[serde(default)]
    e: Option<ErrorIn>,
    #[serde(default)]
    n: Option<u64>,
}

impl<D> WireIn<D> {
    fn into_frame(self, data: impl Fn(D) -> Result<Data, BoxError>) -> Result<Frame, BoxError> {
        let WireIn { t, id, p, h, d, e, n } = self;
        let kind = t.as_str();
        let id = || id.ok_or_else(|| missing(kind, "id"));
        let pattern = || p.clone().ok_or_else(|| missing(kind, "p"));
        let headers = || CallHeaders { entries: h.clone().map(|h| h.0).unwrap_or_default() };
        let data = || -> Result<Data, BoxError> {
            match d {
                Some(d) => data(d),
                None => Ok(Data::default()),
            }
        };
        Ok(match kind {
            "req" => Frame::Req { id: id()?, pattern: pattern()?, headers: headers(), data: data()? },
            "evt" => Frame::Evt { pattern: pattern()?, headers: headers(), data: data()? },
            "res" => Frame::Res { id: id()?, data: data()? },
            "err" => {
                let id = id()?;
                let error = e.ok_or_else(|| missing(kind, "e"))?;
                Frame::Err { id, error: error.into_body() }
            }
            "item" => Frame::Item { id: id()?, data: data()? },
            "end" => Frame::End { id: id()? },
            "open" => Frame::Open { id: id()?, pattern: pattern()?, headers: headers() },
            "in" => Frame::In { id: id()?, data: data()? },
            "in_end" => Frame::InEnd { id: id()? },
            "cancel" => Frame::Cancel { id: id()? },
            "credit" => Frame::Credit { id: id()?, n: n.ok_or_else(|| missing(kind, "n"))? },
            "goaway" => Frame::Goaway,
            other => return Err(format!("`{other}` is not a frame kind").into()),
        })
    }
}

fn missing(kind: &str, field: &str) -> BoxError {
    format!("a `{kind}` frame lacks `{field}`").into()
}

/// `h` in order, a repeated name kept once per value.
#[derive(Clone)]
struct HeadersIn(Vec<(String, String)>);

impl<'de> Deserialize<'de> for HeadersIn {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> Visitor<'de> for Entries {
            type Value = HeadersIn;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map of header names to text values")
            }

            fn visit_unit<E: de::Error>(self) -> Result<HeadersIn, E> {
                Ok(HeadersIn(Vec::new()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<HeadersIn, A::Error> {
                let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((name, value)) = map.next_entry::<String, String>()? {
                    entries.push((name, value));
                }
                Ok(HeadersIn(entries))
            }
        }

        deserializer.deserialize_any(Entries)
    }
}

#[derive(Deserialize)]
struct ErrorIn {
    kind: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    details: serde_json::Value,
}

impl ErrorIn {
    fn into_body(self) -> ErrorBody {
        ErrorBody::new(kind_from_wire(&self.kind), self.message, details_from(self.details))
    }
}

/// The inverse of `ErrorKind::as_str`; a name this version does not know is `Internal`.
pub(crate) fn kind_from_wire(kind: &str) -> ErrorKind {
    match kind {
        "bad_request" => ErrorKind::BadRequest,
        "unauthorized" => ErrorKind::Unauthorized,
        "forbidden" => ErrorKind::Forbidden,
        "not_found" => ErrorKind::NotFound,
        "conflict" => ErrorKind::Conflict,
        "unprocessable" => ErrorKind::Unprocessable,
        "too_many_requests" => ErrorKind::TooManyRequests,
        "timeout" => ErrorKind::Timeout,
        "unavailable" => ErrorKind::Unavailable,
        "unimplemented" => ErrorKind::Unimplemented,
        _ => ErrorKind::Internal,
    }
}

/// `Details`' stable JSON form read back: each entry by its `"type"` tag, an entry with a tag
/// this version does not know kept whole as `Detail::Json`.
fn details_from(value: serde_json::Value) -> Details {
    let serde_json::Value::Array(entries) = value else {
        return Details::new();
    };
    entries.into_iter().map(detail_from).collect()
}

fn detail_from(entry: serde_json::Value) -> Detail {
    let text = |value: &serde_json::Value, key: &str| value.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let list = |value: &serde_json::Value, key: &str| value.get(key).and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("field_violations") => Detail::FieldViolations(
            list(&entry, "violations").iter().map(|v| FieldViolation::new(text(v, "field"), text(v, "description"))).collect(),
        ),
        Some("error_info") => {
            let metadata: BTreeMap<String, String> = entry
                .get("metadata")
                .and_then(serde_json::Value::as_object)
                .map(|object| object.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
                .unwrap_or_default();
            Detail::ErrorInfo { reason: text(&entry, "reason"), domain: text(&entry, "domain"), metadata }
        }
        Some("retry_after") => {
            Detail::RetryAfter(Duration::from_secs(entry.get("seconds").and_then(serde_json::Value::as_u64).unwrap_or(0)))
        }
        Some("help") => Detail::Help(list(&entry, "links").iter().map(|l| Link::new(text(l, "description"), text(l, "url"))).collect()),
        Some("json") => Detail::Json(entry.get("value").cloned().unwrap_or(serde_json::Value::Null)),
        _ => Detail::Json(entry),
    }
}
