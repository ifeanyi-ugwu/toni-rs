//! Payload conversion and response framing for the AMQP transport.
//!
//! AMQP addressing (reply queue, correlation id, metadata headers) lives in
//! the message properties, so the body is just the raw `RpcData` bytes — the
//! same convention as `toni-nats`. Response framing and parsing are the shared
//! convention in [`toni::rpc::wire`].

use std::collections::HashMap;

use lapin::types::{AMQPValue, FieldTable};
use toni::rpc::RpcData;

/// Serialize an outbound payload to AMQP body bytes.
pub(crate) fn data_to_bytes(data: RpcData) -> Vec<u8> {
    match data {
        RpcData::Json(v) => v.to_string().into_bytes(),
        RpcData::Text(s) => s.into_bytes(),
        RpcData::Binary(b) => b,
    }
}

/// Decode an inbound AMQP body. JSON when it parses, raw bytes otherwise.
pub(crate) fn bytes_to_data(bytes: &[u8]) -> RpcData {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(v) => RpcData::Json(v),
        Err(_) => RpcData::Binary(bytes.to_vec()),
    }
}

/// Copy AMQP headers into the string-keyed metadata map. String-valued headers
/// pass through verbatim; scalar numbers/bools are stringified; structured
/// values (arrays, tables) are skipped — `metadata` is a flat string channel.
pub(crate) fn headers_to_metadata(headers: &FieldTable) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    for (key, value) in headers.inner() {
        if let Some(s) = amqp_value_to_string(value) {
            metadata.insert(key.to_string(), s);
        }
    }
    metadata
}

fn amqp_value_to_string(value: &AMQPValue) -> Option<String> {
    match value {
        AMQPValue::LongString(s) => Some(s.to_string()),
        AMQPValue::ShortString(s) => Some(s.to_string()),
        AMQPValue::Boolean(b) => Some(b.to_string()),
        AMQPValue::ShortShortInt(n) => Some(n.to_string()),
        AMQPValue::ShortShortUInt(n) => Some(n.to_string()),
        AMQPValue::ShortInt(n) => Some(n.to_string()),
        AMQPValue::ShortUInt(n) => Some(n.to_string()),
        AMQPValue::LongInt(n) => Some(n.to_string()),
        AMQPValue::LongUInt(n) => Some(n.to_string()),
        AMQPValue::LongLongInt(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_headers_become_metadata() {
        let mut table = FieldTable::default();
        table.insert("trace".into(), AMQPValue::LongString("abc".into()));
        table.insert("count".into(), AMQPValue::LongInt(7));
        let m = headers_to_metadata(&table);
        assert_eq!(m.get("trace").map(String::as_str), Some("abc"));
        assert_eq!(m.get("count").map(String::as_str), Some("7"));
    }
}
