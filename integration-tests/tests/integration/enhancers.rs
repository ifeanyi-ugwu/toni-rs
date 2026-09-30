//! Enhancer stacking and the order the pipeline runs them in: global before
//! controller before method, and guards before interceptors before the handler.
//!
//! Order is the contract consumers write against — an interceptor that starts a
//! span expects the guard's rejection inside it, and a guard reading what an
//! earlier one wrote expects to run second. A reordering that preserves every
//! individual enhancer's behaviour still breaks both. The attribute forms are
//! covered alongside it, since a path-qualified or stacked attribute that is
//! dropped looks exactly like an enhancer that chose not to act.
use crate::common::{ExecutionOrder, TestServer};
use std::sync::Arc;
use ulo::async_trait;
use ulo::context::ExecutionContext;
use ulo::di::MiddlewareConsumer;
use ulo::enhancer::{Guard, Interceptor, InterceptorNext};
use ulo::http::HttpContext;
use ulo::http::HttpHandlerResult;
use ulo::http::middleware::{Middleware, MiddlewareResult, NextHandle};
use ulo::http::{Body, HttpResponse};
use ulo::{
    controller, get, injectable, key, module, post, provide, routes, use_guards, use_interceptors,
};
pub struct OrderTrackerMiddleware {
    name: String,
    tracker: ExecutionOrder,
}

impl OrderTrackerMiddleware {
    pub fn new(name: &str, tracker: ExecutionOrder) -> Self {
        Self {
            name: name.to_string(),
            tracker,
        }
    }
}

#[async_trait]
impl Middleware for OrderTrackerMiddleware {
    async fn handle(&self, mut next: NextHandle) -> MiddlewareResult {
        self.tracker.track(format!("middleware:{}", self.name));
        next.request_mut().headers_mut().insert(
            http::header::HeaderName::from_bytes(b"x-middleware-order").unwrap(),
            http::header::HeaderValue::from_str(&self.name).unwrap(),
        );
        let mut response = next.run().await?;
        response.headers.push((
            "X-Middleware-Modified".to_string(),
            format!("processed-by-{}", self.name),
        ));
        Ok(response)
    }
}

pub struct HeaderCheckMiddleware {
    required_header: String,
    tracker: ExecutionOrder,
}

impl HeaderCheckMiddleware {
    pub fn new(required_header: &str, tracker: ExecutionOrder) -> Self {
        Self {
            required_header: required_header.to_string(),
            tracker,
        }
    }
}

#[async_trait]
impl Middleware for HeaderCheckMiddleware {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        self.tracker.track("middleware:header_check");
        if !next
            .request()
            .headers()
            .contains_key(self.required_header.as_str())
        {
            let mut response = HttpResponse::new();
            response.status = 400;
            response.body = Some(Body::text(format!(
                "Missing required header: {}",
                self.required_header
            )));
            return Ok(response);
        }
        next.run().await
    }
}

#[derive(Clone)]
pub struct AdminGuard {
    tracker: ExecutionOrder,
}

impl AdminGuard {
    pub fn new(tracker: ExecutionOrder) -> Self {
        Self { tracker }
    }
}

#[async_trait]
impl Guard<HttpContext> for AdminGuard {
    async fn can_activate(&self, context: &HttpContext) -> bool {
        self.tracker.track("guard:admin");
        context
            .request()
            .headers
            .get("x-admin-token")
            .and_then(|v| v.to_str().ok())
            .map(|value| value == "secret123")
            .unwrap_or(false)
    }
}

#[derive(Clone)]
pub struct AuthGuard {
    tracker: ExecutionOrder,
}

impl AuthGuard {
    pub fn new(tracker: ExecutionOrder) -> Self {
        Self { tracker }
    }
}

#[async_trait]
impl Guard<HttpContext> for AuthGuard {
    async fn can_activate(&self, context: &HttpContext) -> bool {
        self.tracker.track("guard:auth");
        context.request().headers.contains_key("authorization")
    }
}

pub struct LoggingInterceptor {
    name: String,
    tracker: ExecutionOrder,
}

impl LoggingInterceptor {
    pub fn new(name: &str, tracker: ExecutionOrder) -> Self {
        Self {
            name: name.to_string(),
            tracker,
        }
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for LoggingInterceptor {
    async fn intercept(
        &self,
        _context: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        self.tracker
            .track(format!("interceptor:{}:before", self.name));
        let answer = next.run(_context).await;
        self.tracker
            .track(format!("interceptor:{}:after", self.name));
        answer
    }
}

/// Refuses the request with a 400 when the header marks it invalid, and never
/// calls `next` — an interceptor answering in place of the handler.
pub struct ValidationInterceptor {
    tracker: ExecutionOrder,
}

impl ValidationInterceptor {
    pub fn new(tracker: ExecutionOrder) -> Self {
        Self { tracker }
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for ValidationInterceptor {
    async fn intercept(
        &self,
        context: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        self.tracker.track("interceptor:validation");
        let is_invalid = context
            .request()
            .headers
            .get("x-valid")
            .and_then(|v| v.to_str().ok())
            .map(|value| value == "false")
            .unwrap_or(false);

        if is_invalid {
            let mut response = HttpResponse::new();
            response.status = 400;
            response.body = Some(Body::text("Validation failed".to_string()));
            return Ok(response);
        }
        next.run(context).await
    }
}

#[tokio::test]
async fn enhancers_execution_order() {
    use std::sync::OnceLock;
    static TRACKER: OnceLock<ExecutionOrder> = OnceLock::new();

    let tracker = ExecutionOrder::new();
    TRACKER.set(tracker.clone()).ok();

    fn get_tracker() -> ExecutionOrder {
        TRACKER.get().unwrap().clone()
    }

    #[injectable]
    pub struct TestService {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }
    impl TestService {
        pub fn process(&self, message: &str) -> String {
            self.tracker.track("service:process");
            format!("Processed: {}", message)
        }
    }

    #[controller("/api")]
    pub struct EnhancerController {
        #[inject]
        service: Arc<TestService>,
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }

    #[routes]
    #[use_interceptors(LoggingInterceptor::new("controller", get_tracker()))]
    impl EnhancerController {
        #[use_guards(AdminGuard::new(get_tracker()))]
        #[use_interceptors(LoggingInterceptor::new("method", get_tracker()))]
        #[get("/protected")]
        fn protected_endpoint(&self) -> Body {
            self.tracker.track("controller:protected");
            Body::text("Protected resource".to_string())
        }

        #[use_guards(AuthGuard::new(get_tracker()))]
        #[get("/auth-only")]
        fn auth_only_endpoint(&self) -> Body {
            self.tracker.track("controller:auth_only");
            Body::text("Authenticated resource".to_string())
        }

        #[use_interceptors(
            LoggingInterceptor::new("validate", get_tracker()),
            ValidationInterceptor::new(get_tracker())
        )]
        #[post("/validate")]
        fn validate_endpoint(&self) -> Body {
            self.tracker.track("controller:validate");
            let result = self.service.process("data");
            Body::text(result)
        }

        #[get("/public")]
        fn public_endpoint(&self) -> Body {
            self.tracker.track("controller:public");
            Body::text("Public resource".to_string())
        }
    }

    #[module(
        controllers: [EnhancerController],
        providers: [
            TestService,
            provide!(get_tracker()),
        ],
    )]
    impl EnhancerModule {
        fn configure_middleware(&self, consumer: &mut MiddlewareConsumer) {
            consumer
                .apply(OrderTrackerMiddleware::new("first", get_tracker()))
                .for_routes(vec!["/api/*"]);
            consumer
                .apply(HeaderCheckMiddleware::new("X-Request-ID", get_tracker()))
                .for_routes(vec!["/api/validate"]);
        }
    }

    let server = TestServer::start(EnhancerModule).await;

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/public"))
        .header("X-Request-ID", "test-123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tracker.assert_contains("middleware:first");
    tracker.assert_contains("controller:public");

    tracker.clear();
    let resp = server
        .client()
        .post(server.url("/api/validate"))
        .header("X-Request-ID", "test-456")
        .header("X-Valid", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tracker.assert_contains("middleware:first");
    tracker.assert_contains("middleware:header_check");
    tracker.assert_contains("controller:validate");
    tracker.assert_contains("service:process");

    tracker.clear();
    let resp = server
        .client()
        .post(server.url("/api/validate"))
        .header("X-Request-ID", "test-789")
        .header("X-Valid", "false")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    tracker.assert_not_contains("controller:validate");
    tracker.assert_not_contains("service:process");

    tracker.clear();
    let resp = server
        .client()
        .post(server.url("/api/validate"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    tracker.assert_not_contains("controller:validate");
}

#[tokio::test]
async fn guard_authorization() {
    use std::sync::OnceLock;

    static TRACKER: OnceLock<ExecutionOrder> = OnceLock::new();
    let tracker = ExecutionOrder::new();
    TRACKER.set(tracker.clone()).ok();

    fn get_tracker() -> ExecutionOrder {
        TRACKER.get().unwrap().clone()
    }

    #[controller("/api")]
    pub struct TestController {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }

    key!(Auth: dyn Guard<HttpContext>);

    #[routes]
    impl TestController {
        #[use_guards(Auth)]
        #[get("/auth-only")]
        fn auth_only(&self) -> Body {
            self.tracker.track("controller:auth_only");
            Body::text("Authenticated resource".to_string())
        }
    }

    #[module(
        controllers: [TestController],
        providers: [
            provide!(get_tracker()),
            provide!(Auth => async |tracker: ExecutionOrder| AuthGuard::new(tracker)),
        ],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/auth-only"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    let events = tracker.events();
    assert!(events.contains(&"guard:auth".to_string()));
    assert!(!events.contains(&"controller:auth_only".to_string()));

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/auth-only"))
        .header("Authorization", "Bearer valid-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "Authenticated resource");
    let events = tracker.events();
    assert!(events.contains(&"guard:auth".to_string()));
    assert!(events.contains(&"controller:auth_only".to_string()));
}

#[tokio::test]
async fn di_in_enhancers() {
    #[injectable]
    pub struct AuthService {}
    impl AuthService {
        pub fn validate(&self, token: &str) -> bool {
            token == "valid"
        }
    }

    struct DIGuard {
        auth: AuthService,
    }

    impl DIGuard {
        pub fn new(auth: AuthService) -> Self {
            Self { auth }
        }
    }

    #[async_trait]
    impl Guard<HttpContext> for DIGuard {
        async fn can_activate(&self, context: &HttpContext) -> bool {
            context
                .request()
                .headers
                .get("x-token")
                .and_then(|v| v.to_str().ok())
                .map(|token| self.auth.validate(token))
                .unwrap_or(false)
        }
    }

    #[controller("/api")]
    pub struct TestController {}

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            Body::text("ok".to_string())
        }
    }

    #[module(
        providers: [AuthService],
        controllers: [TestController],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/api/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn the_http_role_collections_are_the_global_enhancers() {
    use std::sync::OnceLock;

    static TRACKER: OnceLock<ExecutionOrder> = OnceLock::new();

    let tracker = ExecutionOrder::new();
    TRACKER.set(tracker.clone()).ok();

    fn get_tracker() -> ExecutionOrder {
        TRACKER.get().unwrap().clone()
    }

    #[injectable]
    pub struct GlobalGuard {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }
    #[async_trait]
    impl Guard<HttpContext> for GlobalGuard {
        async fn can_activate(&self, _context: &HttpContext) -> bool {
            self.tracker.track("global_guard");
            true
        }
    }

    #[injectable]
    pub struct GlobalInterceptor {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }
    #[async_trait]
    impl Interceptor<HttpContext, HttpHandlerResult> for GlobalInterceptor {
        async fn intercept(
            &self,
            context: &HttpContext,
            next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
        ) -> HttpHandlerResult {
            self.tracker.track("global_interceptor:before");
            let answer = next.run(context).await;
            self.tracker.track("global_interceptor:after");
            answer
        }
    }

    #[controller("/api")]
    pub struct TestController {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }

    #[routes]
    impl TestController {
        #[get("/test")]
        fn test(&self) -> Body {
            self.tracker.track("controller:test");
            Body::text("ok".to_string())
        }
    }

    #[module(
        providers: [
            provide!(get_tracker()),
            provide!(into dyn Guard<HttpContext> => GlobalGuard),
            provide!(into dyn Interceptor<HttpContext, HttpHandlerResult> => GlobalInterceptor),
        ],
        controllers: [TestController],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;
    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/test"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tracker.assert_order(&[
        "global_guard",
        "global_interceptor:before",
        "controller:test",
        "global_interceptor:after",
    ]);
}

// Regression: the enhancer scan matched path-qualified attributes for stripping but
// collected them via `Path::get_ident`, which fails on multi-segment paths — the
// attribute vanished without applying the enhancer.
#[tokio::test]
async fn path_qualified_enhancer_attrs() {
    use std::sync::OnceLock;

    static TRACKER: OnceLock<ExecutionOrder> = OnceLock::new();
    let tracker = ExecutionOrder::new();
    TRACKER.set(tracker.clone()).ok();

    fn get_tracker() -> ExecutionOrder {
        TRACKER.get().unwrap().clone()
    }

    #[controller("/api")]
    pub struct TestController {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }

    #[routes]
    #[ulo::use_interceptors(LoggingInterceptor::new("qualified", get_tracker()))]
    impl TestController {
        #[ulo::use_guards(AuthGuard::new(get_tracker()))]
        #[get("/guarded")]
        fn guarded(&self) -> Body {
            self.tracker.track("controller:guarded");
            Body::text("ok".to_string())
        }
    }

    #[module(
        controllers: [TestController],
        providers: [provide!(get_tracker())],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/guarded"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    tracker.assert_contains("guard:auth");
    tracker.assert_not_contains("controller:guarded");

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/guarded"))
        .header("Authorization", "Bearer token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tracker.assert_contains("interceptor:qualified:before");
    tracker.assert_contains("controller:guarded");
}

// Regression: same-kind attributes stacked on one handler were collected into a map
// keyed by attribute name, so the second silently replaced the first.
#[tokio::test]
async fn stacked_enhancer_attrs_accumulate() {
    use std::sync::OnceLock;

    static TRACKER: OnceLock<ExecutionOrder> = OnceLock::new();
    let tracker = ExecutionOrder::new();
    TRACKER.set(tracker.clone()).ok();

    fn get_tracker() -> ExecutionOrder {
        TRACKER.get().unwrap().clone()
    }

    #[controller("/api")]
    pub struct TestController {
        #[inject]
        tracker: Arc<ExecutionOrder>,
    }

    #[routes]
    impl TestController {
        #[use_guards(AuthGuard::new(get_tracker()))]
        #[use_guards(AdminGuard::new(get_tracker()))]
        #[get("/stacked")]
        fn stacked(&self) -> Body {
            self.tracker.track("controller:stacked");
            Body::text("ok".to_string())
        }
    }

    #[module(
        controllers: [TestController],
        providers: [provide!(get_tracker())],
    )]
    impl TestModule {}

    let server = TestServer::start(TestModule).await;

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/stacked"))
        .header("Authorization", "Bearer token")
        .header("X-Admin-Token", "secret123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let events = tracker.events();
    let auth = events.iter().position(|e| e == "guard:auth");
    let admin = events.iter().position(|e| e == "guard:admin");
    assert!(auth.is_some(), "first stacked guard ran: {events:?}");
    assert!(admin.is_some(), "second stacked guard ran: {events:?}");
    assert!(auth < admin, "guards run in declaration order: {events:?}");
    tracker.assert_contains("controller:stacked");

    tracker.clear();
    let resp = server
        .client()
        .get(server.url("/api/stacked"))
        .header("Authorization", "Bearer token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    tracker.assert_not_contains("controller:stacked");
}
