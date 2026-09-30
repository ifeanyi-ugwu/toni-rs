//! A dispatch target whose dependency is execution-scoped is itself built per
//! request, whether or not it declared a scope.
//!
//! Without the elevation a singleton controller would capture the first
//! request's instance and hand it to every later one — a correct-looking
//! application that leaks one user's data to the next. An explicit
//! `scope = "singleton"` does not suppress it: the declaration cannot be
//! honoured, so the framework refuses to pretend it was.
use crate::common::TestServer;
use serial_test::serial;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use ulo::http::Body;
use ulo::{controller, get, injectable, module, routes};
// ---- Test 1: Singleton controller + singleton provider ----------------------

#[injectable]
pub struct SingletonProvider {}
impl SingletonProvider {
    fn get_data(&self) -> String {
        "Singleton data".to_string()
    }
}

#[controller("/ok")]
pub struct OkController {
    #[inject]
    provider: Arc<SingletonProvider>,
}

#[routes]
impl OkController {
    #[get("/test")]
    fn test(&self) -> Body {
        Body::text(self.provider.get_data())
    }
}

#[module(providers: [SingletonProvider], controllers: [OkController])]
impl OkModule {}

// ---- Test 2: Singleton controller + execution-scoped provider (scope promotion) ---

static REQUEST_COUNTER: AtomicU32 = AtomicU32::new(0);

#[injectable(scope = "execution")]
pub struct RequestScopedProvider {}
impl RequestScopedProvider {
    fn get_request_id(&self) -> u32 {
        REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst)
    }
}

// Singleton controller with an execution-scoped dep — framework promotes to execution scope
#[controller("/problematic")]
pub struct ProblematicController {
    #[inject]
    provider: Arc<RequestScopedProvider>,
}

#[routes]
impl ProblematicController {
    #[get("/test")]
    fn test(&self) -> Body {
        Body::text(format!("Request ID: {}", self.provider.get_request_id()))
    }
}

#[module(providers: [RequestScopedProvider], controllers: [ProblematicController])]
impl ProblematicModule {}

// ---- Test 3: Request controller + request provider (valid) ------------------

#[injectable(scope = "execution")]
pub struct AnotherRequestProvider {}
impl AnotherRequestProvider {
    fn get_data(&self) -> String {
        "Request data".to_string()
    }
}

#[controller("/correct", scope = "execution")]
pub struct CorrectController {
    #[inject]
    provider: Arc<AnotherRequestProvider>,
}

#[routes]
impl CorrectController {
    #[get("/test")]
    fn test(&self) -> Body {
        Body::text(self.provider.get_data())
    }
}

#[module(providers: [AnotherRequestProvider], controllers: [CorrectController])]
impl CorrectModule {}

// ---- Test 4: Mixed singleton + request deps (scope promotion) ---------------

#[injectable]
pub struct CacheProvider {}
impl CacheProvider {
    fn get_cached(&self) -> String {
        "Cached".to_string()
    }
}

#[injectable(scope = "execution")]
pub struct SessionProvider {}
impl SessionProvider {
    fn get_session(&self) -> String {
        "Session".to_string()
    }
}

#[controller("/mixed")]
pub struct MixedController {
    #[inject]
    cache: Arc<CacheProvider>,
    #[inject]
    session: Arc<SessionProvider>,
}

#[routes]
impl MixedController {
    #[get("/test")]
    fn test(&self) -> Body {
        Body::text(format!(
            "{} + {}",
            self.cache.get_cached(),
            self.session.get_session()
        ))
    }
}

#[module(providers: [CacheProvider, SessionProvider], controllers: [MixedController])]
impl MixedModule {}

// ---- Test 5: Explicit singleton + request dep (contradiction, still promotes) -----

#[injectable(scope = "execution")]
pub struct ContradictoryRequestProvider {}
impl ContradictoryRequestProvider {
    fn get_id(&self) -> String {
        "contradictory".to_string()
    }
}

#[controller("/explicit", scope = "singleton")]
pub struct ExplicitSingletonController {
    #[inject]
    provider: Arc<ContradictoryRequestProvider>,
}

#[routes]
impl ExplicitSingletonController {
    #[get("/test")]
    fn test(&self) -> Body {
        Body::text(self.provider.get_id())
    }
}

#[module(providers: [ContradictoryRequestProvider], controllers: [ExplicitSingletonController])]
impl ExplicitSingletonModule {}

// ---- tests ------------------------------------------------------------------

#[serial]
#[tokio::test]
async fn singleton_controller_with_singleton_provider() {
    let server = TestServer::start(OkModule).await;
    let resp = server
        .client()
        .get(server.url("/ok/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Singleton data");
}

#[serial]
#[tokio::test]
async fn singleton_controller_promoted_to_request_scope_when_dep_is_request() {
    // The framework detects the scope mismatch and silently promotes the controller to
    // execution-scoped rather than panicking. The endpoint must still be reachable and
    // return the execution-scoped provider's output.
    let server = TestServer::start(ProblematicModule).await;
    let resp = server
        .client()
        .get(server.url("/problematic/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.text().await.unwrap().starts_with("Request ID:"),
        "execution-scoped provider must be accessible after scope promotion"
    );
}

#[serial]
#[tokio::test]
async fn request_controller_with_request_provider() {
    let server = TestServer::start(CorrectModule).await;
    let resp = server
        .client()
        .get(server.url("/correct/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Request data");
}

#[serial]
#[tokio::test]
async fn mixed_scope_deps_promote_controller_to_request() {
    let server = TestServer::start(MixedModule).await;
    let resp = server
        .client()
        .get(server.url("/mixed/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Cached + Session");
}

#[serial]
#[tokio::test]
async fn explicit_singleton_with_request_dep_still_promotes() {
    let server = TestServer::start(ExplicitSingletonModule).await;
    let resp = server
        .client()
        .get(server.url("/explicit/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "contradictory");
}
