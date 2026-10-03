//! Structured error details, chosen to map one-to-one onto `google.rpc` error details, so gRPC
//! carries them natively and every other transport writes them as stable JSON (transports DESIGN
//! §2.4).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::ser::{Serialize, SerializeMap, Serializer};

/// A list of typed entries, written as the `details` member of an HTTP problem document and of
/// the WebSocket and RPC error envelope.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details(Vec<Detail>);

/// One structured detail.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    /// `google.rpc.BadRequest`.
    FieldViolations(Vec<FieldViolation>),
    /// `google.rpc.ErrorInfo`.
    ErrorInfo { reason: String, domain: String, metadata: BTreeMap<String, String> },
    /// `google.rpc.RetryInfo`; HTTP `Retry-After`.
    RetryAfter(Duration),
    /// `google.rpc.Help`.
    Help(Vec<Link>),
    /// Anything else; gRPC carries it as a `google.protobuf.Value` packed in an `Any`.
    Json(serde_json::Value),
}

/// One field that failed validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldViolation {
    /// The field's path, as the request names it.
    pub field: String,
    pub description: String,
}

/// A link a `Help` detail points the caller to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub description: String,
    pub url: String,
}

impl Details {
    pub fn new() -> Self {
        Details::default()
    }

    pub fn push(&mut self, detail: Detail) -> &mut Self {
        self.0.push(detail);
        self
    }

    pub fn with(mut self, detail: Detail) -> Self {
        self.0.push(detail);
        self
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Detail> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The first `RetryAfter`, which HTTP writes as `Retry-After` on a 429 or a 503.
    pub fn retry_after(&self) -> Option<Duration> {
        self.0.iter().find_map(|detail| match detail {
            Detail::RetryAfter(after) => Some(*after),
            _ => None,
        })
    }
}

impl FromIterator<Detail> for Details {
    fn from_iter<I: IntoIterator<Item = Detail>>(iter: I) -> Self {
        Details(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a Details {
    type Item = &'a Detail;
    type IntoIter = std::slice::Iter<'a, Detail>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl FieldViolation {
    pub fn new(field: impl Into<String>, description: impl Into<String>) -> Self {
        FieldViolation { field: field.into(), description: description.into() }
    }
}

impl Link {
    pub fn new(description: impl Into<String>, url: impl Into<String>) -> Self {
        Link { description: description.into(), url: url.into() }
    }
}

/// The stable JSON form: an array of objects, each tagged by `"type"`, so a client matches on the
/// tag the way a gRPC client matches on the `Any`'s type URL. `field`, `description`, `reason`,
/// `domain`, `metadata`, `links` and `url` are the `google.rpc` messages' own field names:
///
/// ```json
/// [
///   { "type": "field_violations", "violations": [{ "field": "email", "description": "not an email address" }] },
///   { "type": "error_info", "reason": "QUOTA", "domain": "billing.example", "metadata": { "plan": "free" } },
///   { "type": "retry_after", "seconds": 30 },
///   { "type": "help", "links": [{ "description": "plans", "url": "https://example.com/plans" }] },
///   { "type": "json", "value": { "anything": true } }
/// ]
/// ```
///
/// `seconds` is whole, a fraction rounded up, so a client honouring it never retries early.
impl Serialize for Details {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(TaggedDetail))
    }
}

struct TaggedDetail<'a>(&'a Detail);

impl Serialize for TaggedDetail<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Detail::FieldViolations(violations) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "field_violations")?;
                map.serialize_entry("violations", &Violations(violations))?;
                map.end()
            }
            Detail::ErrorInfo { reason, domain, metadata } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("type", "error_info")?;
                map.serialize_entry("reason", reason)?;
                map.serialize_entry("domain", domain)?;
                map.serialize_entry("metadata", metadata)?;
                map.end()
            }
            Detail::RetryAfter(after) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "retry_after")?;
                map.serialize_entry("seconds", &whole_seconds(*after))?;
                map.end()
            }
            Detail::Help(links) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "help")?;
                map.serialize_entry("links", &Links(links))?;
                map.end()
            }
            Detail::Json(value) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "json")?;
                map.serialize_entry("value", value)?;
                map.end()
            }
        }
    }
}

fn whole_seconds(after: Duration) -> u64 {
    let rounded_up = u64::from(after.subsec_nanos() > 0);
    after.as_secs().saturating_add(rounded_up)
}

struct Violations<'a>(&'a [FieldViolation]);

impl Serialize for Violations<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|violation| {
            Pair { first: ("field", violation.field.as_str()), second: ("description", violation.description.as_str()) }
        }))
    }
}

struct Links<'a>(&'a [Link]);

impl Serialize for Links<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(
            self.0
                .iter()
                .map(|link| Pair { first: ("description", link.description.as_str()), second: ("url", link.url.as_str()) }),
        )
    }
}

/// An object of two string members, in the order given.
struct Pair<'a> {
    first: (&'static str, &'a str),
    second: (&'static str, &'a str),
}

impl Serialize for Pair<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry(self.first.0, self.first.1)?;
        map.serialize_entry(self.second.0, self.second.1)?;
        map.end()
    }
}
