//! An enhancer contributed to HTTP's global guard or interceptor collection is
//! built from its own declaration, dependencies included, and runs on every
//! request.
//!
//! The contribution builds its own instance, apart from the plain registration
//! of the same type beside it. The dependency is what makes that construction
//! observable: a global that ran without its injected tracker would still
//! answer requests.
use crate::common::TestServer;
use serial_test::serial;
use std::sync::{Arc, Mutex, OnceLock};
use ulo::async_trait;
use ulo::enhancer::{Guard, Interceptor, InterceptorNext};
use ulo::http::Body;
use ulo::http::HttpContext;
use ulo::http::HttpHandlerResult;
use ulo::{controller, get, injectable, module, new, provide, routes};
static TRACKER: OnceLock<ExecutionTracker> = OnceLock::new();

fn get_tracker() -> ExecutionTracker {
    TRACKER.get().unwrap().clone()
}

#[derive(Clone)]
pub struct ExecutionTracker {
    inner: Arc<Mutex<Vec<String>>>,
}

impl ExecutionTracker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn track(&self, event: &str) {
        self.inner.lock().unwrap().push(event.to_string());
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    pub fn get_events(&self) -> Vec<String> {
        self.inner.lock().unwrap().clone()
    }
}

#[injectable]
pub struct MockService {
    name: String,
}
impl MockService {
    #[new]
    pub fn new() -> Self {
        Self {
            name: "MockService".to_string(),
        }
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }
}

#[injectable]
pub struct GlobalGuardWithDI {
    service: Arc<MockService>,
    tracker: Arc<ExecutionTracker>,
}
impl GlobalGuardWithDI {
    #[new]
    pub fn new(service: Arc<MockService>, tracker: Arc<ExecutionTracker>) -> Self {
        Self { service, tracker }
    }
}

#[async_trait]
impl Guard<HttpContext> for GlobalGuardWithDI {
    async fn can_activate(&self, _context: &HttpContext) -> bool {
        self.tracker
            .track(&format!("guard:global:{}", self.service.get_name()));
        true
    }
}

#[injectable]
pub struct GlobalInterceptorWithDI {
    service: Arc<MockService>,
    tracker: Arc<ExecutionTracker>,
}
impl GlobalInterceptorWithDI {
    #[new]
    pub fn new(service: Arc<MockService>, tracker: Arc<ExecutionTracker>) -> Self {
        Self { service, tracker }
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for GlobalInterceptorWithDI {
    async fn intercept(
        &self,
        context: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        self.tracker.track(&format!(
            "interceptor:global:{}:before",
            self.service.get_name()
        ));
        let answer = next.run(context).await;
        self.tracker.track(&format!(
            "interceptor:global:{}:after",
            self.service.get_name()
        ));
        answer
    }
}

#[controller("/api")]
pub struct TestController {
    tracker: Arc<ExecutionTracker>,
}

#[routes]
impl TestController {
    #[new]
    pub fn new(tracker: Arc<ExecutionTracker>) -> Self {
        Self { tracker }
    }

    #[get("/test")]
    fn test_endpoint(&self) -> Body {
        self.tracker.track("controller:handler");
        Body::text("OK".to_string())
    }
}

#[module(
    controllers: [TestController],
    providers: [
        provide!(get_tracker()),
        MockService,
        GlobalGuardWithDI,
        GlobalInterceptorWithDI,
        provide!(into dyn Guard<HttpContext> => GlobalGuardWithDI),
        provide!(into dyn Interceptor<HttpContext, HttpHandlerResult> => GlobalInterceptorWithDI),
    ]
)]
impl TestModule {}

#[serial]
#[tokio::test]
async fn global_collection_enhancers_with_di() {
    TRACKER.set(ExecutionTracker::new()).ok();
    let tracker = get_tracker();

    let server = TestServer::start(TestModule).await;

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let events = tracker.get_events();
    assert!(
        events
            .iter()
            .any(|e| e.contains("guard:global:MockService")),
        "the global guard must run and its injected MockService must be accessible"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("interceptor:global:MockService:before")),
        "the global interceptor's before must run"
    );
    assert!(
        events.iter().any(|e| e == "controller:handler"),
        "controller must run after guards and before interceptor after"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("interceptor:global:MockService:after")),
        "the global interceptor's after must run"
    );

    // guard runs before controller
    let guard_pos = events
        .iter()
        .position(|e| e.contains("guard:global"))
        .unwrap();
    let ctrl_pos = events
        .iter()
        .position(|e| e == "controller:handler")
        .unwrap();
    assert!(guard_pos < ctrl_pos);
}
