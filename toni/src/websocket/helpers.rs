use std::collections::HashMap;

use crate::http_helpers::RequestPart;

use super::{WsClient, WsHandshake};

/// Create a WsClient from HTTP upgrade request parts.
///
/// Extracts handshake headers from `parts.headers` and query params from `parts.uri`.
pub fn create_client_from_parts(parts: &RequestPart) -> WsClient {
    let headers: HashMap<String, String> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_lowercase(),
                value.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    let query: HashMap<String, String> = parts
        .uri
        .query()
        .map(|q| {
            q.split('&')
                .filter_map(|pair| {
                    let mut iter = pair.splitn(2, '=');
                    let key = iter.next()?.to_string();
                    let val = iter.next().unwrap_or("").to_string();
                    Some((key, val))
                })
                .collect()
        })
        .unwrap_or_default();

    WsClient {
        id: uuid::Uuid::new_v4().to_string(),
        handshake: WsHandshake {
            headers,
            query,
            remote_addr: None,
        },
        extensions: Default::default(),
    }
}
