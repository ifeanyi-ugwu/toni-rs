//! Payload conversion and response framing for the AMQP transport.
//!
//! AMQP addressing (reply queue, correlation id, metadata headers) lives in
//! the message properties, so the body is just the raw `RpcData` bytes — the
//! same convention as `toni-nats`. The response framing
//! (`{"response":…}` / `{"err":{"message","status"}}`) matches the other RPC
//! transports so a reader who knows one reads this one for free.

use std::collections::HashMap;

use lapin::types::{AMQPValue, FieldTable};
use serde_json::json;
use toni::rpc::{RpcClientError, RpcData, RpcError};

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

/// Frame a handler outcome into the reply body. `RpcData::Binary` is sent raw
/// (the client falls back to `Binary` when the body is not a JSON envelope);
/// everything else is wrapped in `{"response":…}`.
pub(crate) fn frame_response(outcome: Result<Option<RpcData>, RpcError>) -> Vec<u8> {
    match outcome {
        Ok(Some(RpcData::Binary(b))) => b,
        Ok(Some(RpcData::Json(v))) => json!({ "response": v }).to_string().into_bytes(),
        Ok(Some(RpcData::Text(s))) => json!({ "response": s }).to_string().into_bytes(),
        // #[event_pattern] handler but the caller set reply_to — send an ack.
        Ok(None) => json!({ "response": null }).to_string().into_bytes(),
        Err(RpcError::AppError(arc)) => match RpcError::AppError(arc).to_data() {
            RpcData::Binary(b) => b,
            RpcData::Json(v) => json!({ "response": v }).to_string().into_bytes(),
            RpcData::Text(s) => json!({ "response": s }).to_string().into_bytes(),
        },
        Err(e) => json!({
            "err": { "message": e.to_string(), "status": error_status(&e) }
        })
        .to_string()
        .into_bytes(),
    }
}

/// Reply body for a panicked handler. The panic is logged at the call site.
pub(crate) fn frame_panic() -> Vec<u8> {
    json!({ "err": { "message": "internal server error", "status": "error" } })
        .to_string()
        .into_bytes()
}

/// Parse a reply body back into [`RpcData`], unwrapping the
/// `{"response"}` / `{"err"}` envelope. Falls back to raw `Binary`.
pub(crate) fn parse_response(bytes: &[u8]) -> Result<RpcData, RpcClientError> {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(v) => {
            if let Some(response) = v.get("response") {
                Ok(RpcData::json(response.clone()))
            } else if let Some(err) = v.get("err") {
                let message = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                let status = err
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("error")
                    .to_string();
                Err(RpcClientError::Remote { message, status })
            } else {
                Ok(RpcData::json(v))
            }
        }
        Err(_) => Ok(RpcData::Binary(bytes.to_vec())),
    }
}

fn error_status(e: &RpcError) -> &'static str {
    match e {
        RpcError::PatternNotFound(_) => "not_found",
        RpcError::Forbidden(_) => "forbidden",
        RpcError::Internal(_) => "error",
        RpcError::AppError(_) => unreachable!(
            "RpcError::AppError is framed into the Ok+envelope branch before \
             reaching wire-Err framing"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_then_parse_is_identity_for_json_response() {
        let bytes = frame_response(Ok(Some(RpcData::json(json!({"sum": 5})))));
        let parsed = parse_response(&bytes).unwrap();
        assert_eq!(parsed.as_json(), Some(&json!({"sum": 5})));
    }

    #[test]
    fn framework_error_parses_back_as_remote() {
        let bytes = frame_response(Err(RpcError::Forbidden("nope".to_string())));
        match parse_response(&bytes) {
            Err(RpcClientError::Remote { status, .. }) => assert_eq!(status, "forbidden"),
            other => panic!("expected Remote error, got {other:?}"),
        }
    }

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
