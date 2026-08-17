//! A request-scoped provider is constructed once per request, no matter how many
//! sites inject it.
//!
//! The two sites here are the ones the framework builds separately: an enhancer
//! factory (the guard, resolved before the context exists) and the controller
//! (built later, from the parts). Both join the request's cache through the
//! request parts, so the guard and the handler see the same construction.

use std::sync::atomic::{AtomicU64, Ordering};

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;
use toni::{controller, get, injectable, module, new, routes, Body as ToniBody};

use crate::common::TestServer;

/// Bumped once per `RequestId` construction — the count under test.
static BUILDS: AtomicU64 = AtomicU64::new(0);
/// The id the guard was handed, read back by the assertions.
static GUARD_SAW: AtomicU64 = AtomicU64::new(0);

#[injectable(scope = "request")]
pub struct RequestId {
    id: u64,
}

impl RequestId {
    #[new]
    fn new() -> Self {
        Self {
            id: BUILDS.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }
}

#[injectable(scope = "request")]
pub struct RecordingGuard {
    #[inject]
    request_id: RequestId,
}

#[async_trait]
impl Guard<HttpContext> for RecordingGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        GUARD_SAW.store(self.request_id.id, Ordering::SeqCst);
        true
    }
}

#[controller("/scope")]
pub struct ScopeController {
    #[inject]
    request_id: RequestId,
}

#[routes]
#[use_guards(RecordingGuard)]
impl ScopeController {
    #[get("/id")]
    fn id(&self) -> ToniBody {
        ToniBody::text(self.request_id.id.to_string())
    }
}

#[module(controllers: [ScopeController], providers: [RequestId, RecordingGuard])]
impl ScopeModule {}

#[tokio_localset_test::localset_test]
async fn request_scoped_provider_is_built_once_per_request() {
    let server = TestServer::start(ScopeModule).await;

    let first: u64 = server
        .client()
        .get(server.url("/scope/id"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
        .parse()
        .unwrap();

    // The guard and the controller each injected `RequestId`; one construction served both.
    assert_eq!(BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(GUARD_SAW.load(Ordering::SeqCst), first);

    let second: u64 = server
        .client()
        .get(server.url("/scope/id"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
        .parse()
        .unwrap();

    // Scoped to the request, not the application: the next request builds its own.
    assert_eq!(BUILDS.load(Ordering::SeqCst), 2);
    assert_eq!(GUARD_SAW.load(Ordering::SeqCst), second);
    assert_ne!(first, second);
}
