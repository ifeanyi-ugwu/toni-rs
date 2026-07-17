//! `CorsMiddleware` end-to-end on the global chain.
//!
//! The preflight cases depend on the pre-routing contract: `/data` registers
//! only GET, so an `OPTIONS /data` preflight reaches the middleware solely
//! because the chain runs before the router answers 405.

use std::sync::Arc;

use toni::middleware::{AllowedOrigins, CorsMiddleware, CorsOptions};
use toni::{controller, get, module, routes, Body as ToniBody, ToniFactory};

use crate::common::TestServer;

#[controller("/")]
pub struct DataController {}

#[routes]
impl DataController {
    #[get("/data")]
    fn data(&self) -> ToniBody {
        ToniBody::text("payload")
    }
}

#[module(controllers: [DataController])]
impl CorsModule {}

async fn permissive_server() -> TestServer {
    let mut factory = ToniFactory::new();
    factory.use_global_middleware(Arc::new(CorsMiddleware::permissive()));
    TestServer::start_with(factory, CorsModule).await
}

async fn allowlist_server() -> TestServer {
    let mut factory = ToniFactory::new();
    factory.use_global_middleware(Arc::new(CorsMiddleware::new(CorsOptions {
        origins: AllowedOrigins::List(vec!["http://allowed.dev".into()]),
        credentials: true,
        exposed_headers: vec!["x-request-id".into()],
        max_age: Some(600),
        ..CorsOptions::default()
    })));
    TestServer::start_with(factory, CorsModule).await
}

fn header<'a>(resp: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

#[tokio_localset_test::localset_test]
async fn preflight_answered_without_options_route() {
    let server = permissive_server().await;

    let resp = server
        .client()
        .request(reqwest::Method::OPTIONS, server.url("/data"))
        .header("origin", "http://example.com")
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "x-custom")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204);
    assert_eq!(header(&resp, "access-control-allow-origin"), Some("*"));
    assert!(header(&resp, "access-control-allow-methods")
        .unwrap()
        .contains("GET"));
    // No allowed_headers configured — the requested set is reflected.
    assert_eq!(
        header(&resp, "access-control-allow-headers"),
        Some("x-custom")
    );
    assert!(header(&resp, "vary").unwrap().contains("Origin"));
}

#[tokio_localset_test::localset_test]
async fn actual_request_is_decorated() {
    let server = permissive_server().await;

    let resp = server
        .client()
        .get(server.url("/data"))
        .header("origin", "http://example.com")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "access-control-allow-origin"), Some("*"));
    assert_eq!(resp.text().await.unwrap(), "payload");
}

#[tokio_localset_test::localset_test]
async fn same_origin_request_untouched() {
    let server = permissive_server().await;

    let resp = server
        .client()
        .get(server.url("/data"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "access-control-allow-origin"), None);
}

#[tokio_localset_test::localset_test]
async fn allowlist_echoes_origin_with_credentials() {
    let server = allowlist_server().await;

    let resp = server
        .client()
        .get(server.url("/data"))
        .header("origin", "http://allowed.dev")
        .send()
        .await
        .unwrap();

    assert_eq!(
        header(&resp, "access-control-allow-origin"),
        Some("http://allowed.dev"),
        "credentials forbid *, the origin must be echoed"
    );
    assert_eq!(
        header(&resp, "access-control-allow-credentials"),
        Some("true")
    );
    assert_eq!(
        header(&resp, "access-control-expose-headers"),
        Some("x-request-id")
    );
}

#[tokio_localset_test::localset_test]
async fn disallowed_origin_gets_no_cors_headers() {
    let server = allowlist_server().await;

    let resp = server
        .client()
        .get(server.url("/data"))
        .header("origin", "http://evil.dev")
        .send()
        .await
        .unwrap();

    // Forwarded — the browser enforces the block via the missing header.
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "access-control-allow-origin"), None);

    let preflight = server
        .client()
        .request(reqwest::Method::OPTIONS, server.url("/data"))
        .header("origin", "http://evil.dev")
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap();

    assert_eq!(preflight.status(), 204);
    assert_eq!(header(&preflight, "access-control-allow-origin"), None);
}

#[tokio_localset_test::localset_test]
async fn preflight_carries_max_age() {
    let server = allowlist_server().await;

    let resp = server
        .client()
        .request(reqwest::Method::OPTIONS, server.url("/data"))
        .header("origin", "http://allowed.dev")
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204);
    assert_eq!(header(&resp, "access-control-max-age"), Some("600"));
}
