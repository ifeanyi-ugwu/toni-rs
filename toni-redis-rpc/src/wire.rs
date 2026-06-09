//! Wire contract between [`RedisClientTransport`] and [`RedisAdapter`].
//!
//! Both halves are toni — the envelope is never meant to be hand-typed against
//! `redis-cli`, so `data` carries the externally-tagged [`RpcData`] form
//! (`{"Json":…}` / `{"Text":…}` / `{"Binary":[…]}`) rather than a bare value.
//! That keeps all three payload variants lossless across the hop.
//!
//! The response framing deliberately matches `toni-nats`
//! (`{"response":…}` / `{"err":{"message","status"}}`) so a reader who knows
//! one transport reads the other for free, and so the client-side parse is
//! identical.
//!
//! [`RedisClientTransport`]: crate::RedisClientTransport
//! [`RedisAdapter`]: crate::RedisAdapter

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use toni::rpc::{RpcClientError, RpcData, RpcError};

/// Published to a handler's pattern channel for every call.
///
/// `reply_to` present means request-response — the server publishes the
/// response envelope there. Absent means fire-and-forget (`emit`); the server
/// runs the handler and sends nothing back.
#[derive(Serialize, Deserialize)]
pub(crate) struct RequestEnvelope {
    pub data: RpcData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// Frame a successful or failed handler outcome into the bytes published to the
/// caller's reply channel. Mirrors `toni-nats`: `RpcData::Binary` is sent raw
/// (the client falls back to `Binary` when the payload is not a JSON envelope);
/// everything else is wrapped in `{"response":…}`.
pub(crate) fn frame_response(outcome: Result<Option<RpcData>, RpcError>) -> Vec<u8> {
    match outcome {
        Ok(Some(RpcData::Binary(b))) => b,
        Ok(Some(RpcData::Json(v))) => json!({ "response": v }).to_string().into_bytes(),
        Ok(Some(RpcData::Text(s))) => json!({ "response": s }).to_string().into_bytes(),
        // #[event_pattern] handler but the caller set reply_to — send an ack so
        // the pending request closes instead of timing out.
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

/// Bytes published when the handler panicked. The panic is already logged at
/// the call site; the caller sees a generic internal error.
pub(crate) fn frame_panic() -> Vec<u8> {
    json!({ "err": { "message": "internal server error", "status": "error" } })
        .to_string()
        .into_bytes()
}

/// Parse the reply-channel payload back into an [`RpcData`], unwrapping the
/// `{"response"}` / `{"err"}` envelope. Falls back to raw `Binary` when the
/// payload is not a recognized envelope.
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
    fn envelope_round_trips_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("trace".to_string(), "abc".to_string());
        let env = RequestEnvelope {
            data: RpcData::json(serde_json::json!({"a": 1})),
            reply_to: Some("toni:rpc:reply:c:7".to_string()),
            metadata,
        };

        let bytes = serde_json::to_vec(&env).unwrap();
        let back: RequestEnvelope = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(back.reply_to.as_deref(), Some("toni:rpc:reply:c:7"));
        assert_eq!(back.metadata.get("trace").map(String::as_str), Some("abc"));
        assert_eq!(back.data.as_json(), Some(&serde_json::json!({"a": 1})));
    }

    #[test]
    fn fire_and_forget_envelope_omits_reply_to() {
        let env = RequestEnvelope {
            data: RpcData::text("hi"),
            reply_to: None,
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("reply_to"), "got: {json}");
        assert!(!json.contains("metadata"), "got: {json}");
    }

    #[test]
    fn frame_then_parse_is_identity_for_json_response() {
        let bytes = frame_response(Ok(Some(RpcData::json(serde_json::json!({"sum": 5})))));
        let parsed = parse_response(&bytes).unwrap();
        assert_eq!(parsed.as_json(), Some(&serde_json::json!({"sum": 5})));
    }

    #[test]
    fn framework_error_parses_back_as_remote() {
        let bytes = frame_response(Err(RpcError::Forbidden("nope".to_string())));
        match parse_response(&bytes) {
            Err(RpcClientError::Remote { status, .. }) => assert_eq!(status, "forbidden"),
            other => panic!("expected Remote error, got {other:?}"),
        }
    }
}
