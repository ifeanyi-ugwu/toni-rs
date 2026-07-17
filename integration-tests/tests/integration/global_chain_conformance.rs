//! Conformance suite for the global middleware chain's pre-routing contract.
//!
//! The contract ([`AdapterContext`]): the global chain observes every inbound
//! HTTP request before route resolution, may short-circuit with a response,
//! and the request it forwards is the one the router matches on.
//!
//! Every HTTP adapter must pass this suite — `conformance_suite!` instantiates
//! the six cases per adapter. The discriminating cases are the ones a
//! post-routing anchor cannot satisfy: method mismatch (405), CORS preflight
//! to a route without an OPTIONS handler, and a path rewrite that changes
//! which route runs.
//!
//! `#[serial]` must precede `#[localset_test]` — the localset macro rebuilds
//! the function and drops any attribute written after it.
//!
//! [`AdapterContext`]: toni::AdapterContext

use std::sync::{Arc, Mutex, OnceLock};

use toni::traits_helpers::middleware::{Middleware, MiddlewareResult, NextHandle};
use toni::{async_trait, controller, get, module, routes, Body as ToniBody, ToniFactory};

use crate::common::TestServer;

static EVENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn events() -> &'static Mutex<Vec<String>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn track(event: impl Into<String>) {
    events().lock().unwrap().push(event.into());
}

fn drain() -> Vec<String> {
    std::mem::take(&mut *events().lock().unwrap())
}

/// Outermost: records every request the chain sees and stamps every response.
struct Recording;

#[async_trait]
impl Middleware for Recording {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        let req = next.request();
        track(format!("chain:{} {}", req.method(), req.uri().path()));
        let mut response = next.run().await?;
        response.headers.push(("x-chain".into(), "1".into()));
        Ok(response)
    }
}

/// Short-circuits any request carrying an `x-block` header.
struct Block;

#[async_trait]
impl Middleware for Block {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        if next.request().headers().contains_key("x-block") {
            return Ok(toni::HttpResponse::forbidden().text("blocked").build());
        }
        next.run().await
    }
}

/// Answers OPTIONS requests directly — the CORS preflight shape.
struct Preflight;

#[async_trait]
impl Middleware for Preflight {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        if next.request().method().as_str() == "OPTIONS" {
            return Ok(toni::HttpResponse::no_content()
                .header("access-control-allow-origin", "*")
                .build());
        }
        next.run().await
    }
}

/// Rewrites `/rewritten` to `/alpha` — routing must match on the new path.
struct Rewrite;

#[async_trait]
impl Middleware for Rewrite {
    async fn handle(&self, mut next: NextHandle) -> MiddlewareResult {
        if next.request().uri().path() == "/rewritten" {
            *next.request_mut().uri_mut() = "/alpha".parse().unwrap();
        }
        next.run().await
    }
}

#[controller("/")]
pub struct ConformanceController {}

#[routes]
impl ConformanceController {
    #[get("/probe")]
    fn probe(&self) -> ToniBody {
        track("handler:probe");
        ToniBody::text("probe")
    }

    #[get("/alpha")]
    fn alpha(&self) -> ToniBody {
        track("handler:alpha");
        ToniBody::text("alpha")
    }
}

#[module(controllers: [ConformanceController])]
impl ConformanceModule {}

async fn boot(adapter: impl toni::HttpAdapter + 'static) -> TestServer {
    let mut factory = ToniFactory::new();
    factory
        .use_global_middleware(Arc::new(Recording))
        .use_global_middleware(Arc::new(Block))
        .use_global_middleware(Arc::new(Preflight))
        .use_global_middleware(Arc::new(Rewrite));
    TestServer::start_adapter(factory, ConformanceModule, adapter).await
}

async fn case_matched_route(server: TestServer) {
    drain();

    let resp = server
        .client()
        .get(server.url("/probe"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("x-chain").unwrap(), "1");
    assert_eq!(resp.text().await.unwrap(), "probe");
    assert_eq!(drain(), vec!["chain:GET /probe", "handler:probe"]);
}

async fn case_unknown_path(server: TestServer) {
    drain();

    let resp = server
        .client()
        .get(server.url("/missing"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    assert_eq!(
        resp.headers().get("x-chain").map(|v| v.to_str().unwrap()),
        Some("1"),
        "global chain must run on 404s"
    );
    assert_eq!(drain(), vec!["chain:GET /missing"]);
}

async fn case_method_mismatch(server: TestServer) {
    drain();

    // /probe only has GET — the adapter's native router answers 405.
    let resp = server
        .client()
        .post(server.url("/probe"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 405);
    assert_eq!(
        resp.headers().get("x-chain").map(|v| v.to_str().unwrap()),
        Some("1"),
        "global chain must run on method mismatches (405)"
    );
    assert_eq!(drain(), vec!["chain:POST /probe"]);
}

async fn case_preflight(server: TestServer) {
    drain();

    // No OPTIONS handler exists for /probe — middleware must still see and
    // answer the request. This is the CORS preflight shape.
    let resp = server
        .client()
        .request(reqwest::Method::OPTIONS, server.url("/probe"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        204,
        "preflight must be answered by middleware"
    );
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("*")
    );
    assert_eq!(drain(), vec!["chain:OPTIONS /probe"]);
}

async fn case_short_circuit(server: TestServer) {
    drain();

    let resp = server
        .client()
        .get(server.url("/probe"))
        .header("x-block", "1")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 403);
    assert_eq!(resp.text().await.unwrap(), "blocked");
    assert_eq!(drain(), vec!["chain:GET /probe"], "handler must not run");
}

async fn case_rewrite(server: TestServer) {
    drain();

    // No /rewritten route exists; middleware rewrites the path to /alpha.
    let resp = server
        .client()
        .get(server.url("/rewritten"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "routing must match on the middleware-rewritten path"
    );
    assert_eq!(resp.text().await.unwrap(), "alpha");
    assert_eq!(drain(), vec!["chain:GET /rewritten", "handler:alpha"]);
}

macro_rules! conformance_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            use serial_test::serial;

            #[serial(global_chain)]
            #[tokio_localset_test::localset_test]
            async fn chain_runs_on_matched_route() {
                super::case_matched_route(super::boot($adapter).await).await;
            }

            #[serial(global_chain)]
            #[tokio_localset_test::localset_test]
            async fn chain_runs_on_unknown_path() {
                super::case_unknown_path(super::boot($adapter).await).await;
            }

            #[serial(global_chain)]
            #[tokio_localset_test::localset_test]
            async fn chain_runs_on_method_mismatch() {
                super::case_method_mismatch(super::boot($adapter).await).await;
            }

            #[serial(global_chain)]
            #[tokio_localset_test::localset_test]
            async fn middleware_answers_preflight() {
                super::case_preflight(super::boot($adapter).await).await;
            }

            #[serial(global_chain)]
            #[tokio_localset_test::localset_test]
            async fn middleware_short_circuits_before_handler() {
                super::case_short_circuit(super::boot($adapter).await).await;
            }

            #[serial(global_chain)]
            #[tokio_localset_test::localset_test]
            async fn request_rewrite_changes_matched_route() {
                super::case_rewrite(super::boot($adapter).await).await;
            }
        }
    };
}

conformance_suite!(axum, toni_axum::AxumAdapter::new());
conformance_suite!(poem, toni_poem::PoemAdapter::new());
conformance_suite!(salvo, toni_salvo::SalvoAdapter::new());
conformance_suite!(actix, toni_actix::ActixAdapter::new());
conformance_suite!(rocket, toni_rocket::RocketAdapter::new());
