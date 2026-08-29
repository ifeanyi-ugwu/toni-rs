//! Payload conversion, header handling, and response framing for the Kafka
//! transport.
//!
//! Kafka message headers carry the reply addressing (`toni-reply-to`,
//! `toni-correlation-id`) and per-call metadata, so the body is just the raw
//! `RpcData` bytes — the same convention as `toni-nats`. Response framing and
//! parsing are the shared convention in [`toni::rpc::wire`].

use std::collections::HashMap;

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::message::{Header, Headers, OwnedHeaders};
use toni::rpc::RpcData;

/// The topic carrying stream-cancel notices (ADR-0032). Every server
/// instance consumes it in its own consumer group, so each sees every notice
/// and only the instance holding the call acts on it.
pub(crate) const CANCEL_TOPIC: &str = "toni.rpc.cancel";

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

#[cfg(test)]
mod tests {
    use super::*;

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
