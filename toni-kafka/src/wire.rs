//! Payload conversion, header handling, and response framing for the Kafka
//! transport.
//!
//! Kafka message headers carry the reply addressing (`toni-reply-to`,
//! `toni-correlation-id`) and per-call metadata, so the body is just the raw
//! `RpcData` bytes — the same convention as `toni-nats`. Response framing
//! (`{"response":…}` / `{"err":{"message","status"}}`) matches the other RPC
//! transports.

use std::collections::HashMap;

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, Headers, OwnedHeaders};
use serde_json::json;
use toni::rpc::{RpcClientError, RpcData, RpcError};

/// Header naming the topic a reply should be published to. Present ⇒
/// request-response; absent ⇒ fire-and-forget.
pub(crate) const HEADER_REPLY_TO: &str = "toni-reply-to";
/// Header correlating a reply with its request.
pub(crate) const HEADER_CORRELATION_ID: &str = "toni-correlation-id";

/// Pre-create the given topics (1 partition, RF 1) so consumers assign their
/// partitions at join time instead of waiting for a metadata refresh to notice
/// an auto-created topic. Best-effort: an already-existing topic is fine, and a
/// failure here just falls back to broker auto-create.
pub(crate) async fn ensure_topics(brokers: &str, topics: &[String]) {
    if topics.is_empty() {
        return;
    }
    let admin: AdminClient<DefaultClientContext> = match ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .create()
    {
        Ok(admin) => admin,
        Err(e) => {
            tracing::warn!(error = %e, "ensure_topics: admin client create failed");
            return;
        }
    };
    let new_topics: Vec<NewTopic> = topics
        .iter()
        .map(|t| NewTopic::new(t, 1, TopicReplication::Fixed(1)))
        .collect();
    if let Err(e) = admin.create_topics(&new_topics, &AdminOptions::new()).await {
        tracing::warn!(error = %e, "ensure_topics: create_topics failed");
    }
}

pub(crate) fn data_to_bytes(data: RpcData) -> Vec<u8> {
    match data {
        RpcData::Json(v) => v.to_string().into_bytes(),
        RpcData::Text(s) => s.into_bytes(),
        RpcData::Binary(b) => b,
    }
}

pub(crate) fn bytes_to_data(bytes: &[u8]) -> RpcData {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(v) => RpcData::Json(v),
        Err(_) => RpcData::Binary(bytes.to_vec()),
    }
}

/// Build the header set for an outbound record: the optional control headers
/// plus every user metadata entry.
pub(crate) fn build_headers(
    reply_to: Option<&str>,
    correlation_id: Option<&str>,
    metadata: &HashMap<String, String>,
) -> OwnedHeaders {
    let mut headers = OwnedHeaders::new();
    if let Some(reply_to) = reply_to {
        headers = headers.insert(Header {
            key: HEADER_REPLY_TO,
            value: Some(reply_to),
        });
    }
    if let Some(correlation_id) = correlation_id {
        headers = headers.insert(Header {
            key: HEADER_CORRELATION_ID,
            value: Some(correlation_id),
        });
    }
    for (key, value) in metadata {
        headers = headers.insert(Header {
            key: key.as_str(),
            value: Some(value.as_str()),
        });
    }
    headers
}

/// Read a single header value as a UTF-8 string.
pub(crate) fn header_str<H: Headers>(headers: Option<&H>, key: &str) -> Option<String> {
    let headers = headers?;
    for i in 0..headers.count() {
        let header = headers.get(i);
        if header.key == key {
            return header
                .value
                .map(|v| String::from_utf8_lossy(v).into_owned());
        }
    }
    None
}

/// Collect user metadata from the headers, skipping the reserved control keys.
pub(crate) fn metadata_from_headers<H: Headers>(headers: Option<&H>) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    if let Some(headers) = headers {
        for i in 0..headers.count() {
            let header = headers.get(i);
            if header.key == HEADER_REPLY_TO || header.key == HEADER_CORRELATION_ID {
                continue;
            }
            if let Some(value) = header.value {
                metadata.insert(
                    header.key.to_string(),
                    String::from_utf8_lossy(value).into_owned(),
                );
            }
        }
    }
    metadata
}

pub(crate) fn frame_response(outcome: Result<Option<RpcData>, RpcError>) -> Vec<u8> {
    match outcome {
        Ok(Some(RpcData::Binary(b))) => b,
        Ok(Some(RpcData::Json(v))) => json!({ "response": v }).to_string().into_bytes(),
        Ok(Some(RpcData::Text(s))) => json!({ "response": s }).to_string().into_bytes(),
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

pub(crate) fn frame_panic() -> Vec<u8> {
    json!({ "err": { "message": "internal server error", "status": "error" } })
        .to_string()
        .into_bytes()
}

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
        assert_eq!(
            parse_response(&bytes).unwrap().as_json(),
            Some(&json!({"sum": 5}))
        );
    }

    #[test]
    fn framework_error_parses_back_as_remote() {
        let bytes = frame_response(Err(RpcError::Forbidden("nope".to_string())));
        match parse_response(&bytes) {
            Err(RpcClientError::Remote { status, .. }) => assert_eq!(status, "forbidden"),
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn metadata_round_trips_through_headers_skipping_control_keys() {
        let mut md = HashMap::new();
        md.insert("trace".to_string(), "abc".to_string());
        let headers = build_headers(Some("reply.topic"), Some("7"), &md);

        assert_eq!(
            header_str(Some(&headers), HEADER_REPLY_TO).as_deref(),
            Some("reply.topic")
        );
        assert_eq!(
            header_str(Some(&headers), HEADER_CORRELATION_ID).as_deref(),
            Some("7")
        );
        let back = metadata_from_headers(Some(&headers));
        assert_eq!(back.get("trace").map(String::as_str), Some("abc"));
        assert!(!back.contains_key(HEADER_REPLY_TO));
        assert!(!back.contains_key(HEADER_CORRELATION_ID));
    }
}
