//! Payload conversion and response framing for the MQTT v5 transport.
//!
//! MQTT v5 carries the reply address (`response_topic`), the call correlation
//! (`correlation_data`), and per-call metadata (`user_properties`) in the
//! PUBLISH properties, so the body is just the raw `RpcData` bytes — the same
//! convention as `toni-nats`/`toni-rabbitmq`. Response framing and parsing are
//! the shared convention in [`toni::rpc::wire`].

use std::collections::HashMap;

use toni::rpc::RpcData;

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

#[cfg(test)]
mod tests {
    use super::*;

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
