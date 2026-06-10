//! Payload conversion and response framing for the MQTT v5 transport.
//!
//! MQTT v5 carries the reply address (`response_topic`), the call correlation
//! (`correlation_data`), and per-call metadata (`user_properties`) in the
//! PUBLISH properties, so the body is just the raw `RpcData` bytes — the same
//! convention as `toni-nats`/`toni-rabbitmq`. Response framing
//! (`{"response":…}` / `{"err":{"message","status"}}`) matches the other RPC
//! transports.

use std::collections::HashMap;

use serde_json::json;
use toni::rpc::{RpcClientError, RpcData, RpcError};

/// Serialize an outbound payload to MQTT body bytes.
pub(crate) fn data_to_bytes(data: RpcData) -> Vec<u8> {
    match data {
        RpcData::Json(v) => v.to_string().into_bytes(),
        RpcData::Text(s) => s.into_bytes(),
        RpcData::Binary(b) => b,
    }
}

/// Decode an inbound MQTT body. JSON when it parses, raw bytes otherwise.
pub(crate) fn bytes_to_data(bytes: &[u8]) -> RpcData {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(v) => RpcData::Json(v),
        Err(_) => RpcData::Binary(bytes.to_vec()),
    }
}

/// Copy MQTT v5 user properties into the string-keyed metadata map. Later
/// duplicate keys win — MQTT permits repeated keys, but `metadata` is a flat
/// map.
pub(crate) fn user_properties_to_metadata(props: &[(String, String)]) -> HashMap<String, String> {
    props.iter().cloned().collect()
}

/// Frame a handler outcome into the reply body. `RpcData::Binary` is sent raw
/// (the client falls back to `Binary` when the body is not a JSON envelope);
/// everything else is wrapped in `{"response":…}`.
pub(crate) fn frame_response(outcome: Result<Option<RpcData>, RpcError>) -> Vec<u8> {
    match outcome {
        Ok(Some(RpcData::Binary(b))) => b,
        Ok(Some(RpcData::Json(v))) => json!({ "response": v }).to_string().into_bytes(),
        Ok(Some(RpcData::Text(s))) => json!({ "response": s }).to_string().into_bytes(),
        // #[event_pattern] handler but the caller set response_topic — send an ack.
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
    fn user_properties_become_metadata() {
        let props = vec![
            ("trace".to_string(), "abc".to_string()),
            ("tenant".to_string(), "acme".to_string()),
        ];
        let m = user_properties_to_metadata(&props);
        assert_eq!(m.get("trace").map(String::as_str), Some("abc"));
        assert_eq!(m.get("tenant").map(String::as_str), Some("acme"));
    }
}
