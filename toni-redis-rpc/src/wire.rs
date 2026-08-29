//! Wire contract between [`RedisClientTransport`] and [`RedisAdapter`].
//!
//! Both halves are toni — the envelope is never meant to be hand-typed against
//! `redis-cli`, so `data` carries the externally-tagged [`RpcData`] form
//! (`{"Json":…}` / `{"Text":…}` / `{"Binary":[…]}`) rather than a bare value.
//! That keeps all three payload variants lossless across the hop.
//!
//! Response framing and parsing are the shared convention in
//! [`toni::rpc::wire`].
//!
//! [`RedisClientTransport`]: crate::RedisClientTransport
//! [`RedisAdapter`]: crate::RedisAdapter

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use toni::rpc::RpcData;

/// The channel carrying stream-cancel notices (ADR-0032). Every server
/// instance subscribes it, so each sees every notice and only the instance
/// holding the call acts on it.
pub(crate) const CANCEL_CHANNEL: &str = "toni:rpc:cancel";

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
}
