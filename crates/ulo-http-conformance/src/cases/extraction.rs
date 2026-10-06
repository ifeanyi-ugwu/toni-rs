//! Scenarios: extraction.

use http::Method;

use crate::wire::{Exchange, same_as_reference, start};
use crate::{Host, Mode};

/// Each extraction failure's status, 400, 413, 415 and 422, with its problem document.
pub async fn failures<H: Host>(mode: Mode) {
    let host = start::<H>(mode).await;
    let json = "application/json";
    let cases = [
        (Exchange::get("/page?page=many"), 400),
        (
            Exchange::new(Method::POST, "/small").header("content-type", json).body(r#"{"name":"far-too-long-for-sixteen"}"#),
            413,
        ),
        (Exchange::new(Method::POST, "/json").header("content-type", "text/plain").body(r#"{"name":"a"}"#), 415),
        (Exchange::new(Method::POST, "/valid").header("content-type", json).body(r#"{"name":""}"#), 422),
    ];
    for (exchange, status) in cases {
        let path = exchange.path.clone();
        let reply = same_as_reference(&host, exchange).await;
        assert_eq!(reply.status, status, "{path}");
        assert!(
            reply.header("content-type").is_some_and(|value| value.starts_with("application/problem+json")),
            "{path} answers without a problem document"
        );
    }
    host.stop().await;
}
