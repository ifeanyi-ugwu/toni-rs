//! Test for three-level enhancer hierarchy: global < controller < method
//!
//! This test verifies that:
//! 1. Global enhancers are registered via ToniFactory
//! 2. Controller-level enhancers apply to all methods
//! 3. Method-level enhancers add to controller-level
//! 4. Execution order is: global → controller → method
//! 5. Same enhancer can be registered multiple times at different levels
//! 6. Global middleware wraps the entire enhancer pipeline (outermost layer)

use serial_test::serial;
use std::sync::{Arc, Mutex};
use toni::async_trait;
use toni::http_helpers::HttpResponse;
use toni::{
    controller, get, module, routes, use_guards, use_interceptors, Body as ToniBody, ToniFactory,
};
use toni_axum::AxumAdapter;

use toni::context::HttpContext;
use toni::traits_helpers::middleware::{Middleware, MiddlewareResult, NextHandle};
use toni::traits_helpers::{Guard, Interceptor, InterceptorNext};

// ============================================================================
// EXECUTION ORDER TRACKER
// ============================================================================

type ExecutionOrderInner = Arc<Mutex<Vec<String>>>;

#[derive(Clone)]
pub struct ExecutionOrder {
    inner: ExecutionOrderInner,
}

impl ExecutionOrder {
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

static GLOBAL_TRACKER: std::sync::OnceLock<ExecutionOrder> = std::sync::OnceLock::new();

fn get_tracker() -> ExecutionOrder {
    GLOBAL_TRACKER.get_or_init(ExecutionOrder::new).clone()
}

// ============================================================================
// GUARD IMPLEMENTATIONS
// ============================================================================

pub struct GlobalGuard;

impl GlobalGuard {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Guard<HttpContext> for GlobalGuard {
    async fn can_activate(&self, _context: &HttpContext) -> bool {
        get_tracker().track("guard:global");
        true
    }
}

pub struct ControllerGuard;

impl ControllerGuard {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Guard<HttpContext> for ControllerGuard {
    async fn can_activate(&self, _context: &HttpContext) -> bool {
        get_tracker().track("guard:controller");
        true
    }
}

pub struct MethodGuard;

impl MethodGuard {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Guard<HttpContext> for MethodGuard {
    async fn can_activate(&self, _context: &HttpContext) -> bool {
        get_tracker().track("guard:method");
        true
    }
}

// ============================================================================
// INTERCEPTOR IMPLEMENTATIONS
// ============================================================================

pub struct GlobalInterceptor;

impl GlobalInterceptor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpResponse> for GlobalInterceptor {
    async fn intercept(
        &self,
        context: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpResponse>>,
    ) -> HttpResponse {
        get_tracker().track("interceptor:global:before");
        let answer = next.run(context).await;
        get_tracker().track("interceptor:global:after");
        answer
    }
}

pub struct ControllerInterceptor;

impl ControllerInterceptor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpResponse> for ControllerInterceptor {
    async fn intercept(
        &self,
        context: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpResponse>>,
    ) -> HttpResponse {
        get_tracker().track("interceptor:controller:before");
        let answer = next.run(context).await;
        get_tracker().track("interceptor:controller:after");
        answer
    }
}

pub struct MethodInterceptor;

impl MethodInterceptor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpResponse> for MethodInterceptor {
    async fn intercept(
        &self,
        context: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpResponse>>,
    ) -> HttpResponse {
        get_tracker().track("interceptor:method:before");
        let answer = next.run(context).await;
        get_tracker().track("interceptor:method:after");
        answer
    }
}

// ============================================================================
// MIDDLEWARE IMPLEMENTATION
// ============================================================================

pub struct GlobalMiddleware;

impl GlobalMiddleware {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Middleware for GlobalMiddleware {
    async fn handle(&self, next: NextHandle) -> MiddlewareResult {
        get_tracker().track("middleware:global:before");
        let response = next.run().await;
        get_tracker().track("middleware:global:after");
        response
    }
}

// ============================================================================
// CONTROLLER WITH THREE-LEVEL ENHANCERS
// ============================================================================

#[controller("/api")]
pub struct TestController {}

#[routes]
#[use_guards(ControllerGuard{})]
#[use_interceptors(ControllerInterceptor{})]
impl TestController {
    /// Endpoint with all three levels:
    /// - Global (from ToniFactory)
    /// - Controller (from impl block)
    /// - Method (from this method)
    #[use_guards(MethodGuard{})]
    #[use_interceptors(MethodInterceptor{})]
    #[get("/three-level")]
    fn three_level_endpoint(&self) -> ToniBody {
        get_tracker().track("controller:three_level");
        ToniBody::text("Three-level test".to_string())
    }

    /// Endpoint with only global + controller levels (no method-level)
    #[get("/two-level")]
    fn two_level_endpoint(&self) -> ToniBody {
        get_tracker().track("controller:two_level");
        ToniBody::text("Two-level test".to_string())
    }

    /// Endpoint with duplicated guard at all three levels
    #[use_guards(GlobalGuard{})]
    #[get("/duplicate")]
    fn duplicate_endpoint(&self) -> ToniBody {
        get_tracker().track("controller:duplicate");
        ToniBody::text("Duplicate test".to_string())
    }
}

#[module(
    controllers: [TestController]
)]
impl TestModule {}

// ============================================================================
// TESTS
// ============================================================================

#[tokio::test]
#[serial]
async fn test_three_level_enhancer_hierarchy() {
    let tracker = get_tracker();
    tracker.clear();
    let port = 29095;

    let local = tokio::task::LocalSet::new();

    local.spawn_local(async move {
        // Create factory and register GLOBAL enhancers
        let mut factory = ToniFactory::new();
        factory
            .use_global_middleware(Arc::new(GlobalMiddleware::new()))
            .use_global_http_guards(Arc::new(GlobalGuard::new()))
            .use_global_http_interceptors(Arc::new(GlobalInterceptor::new()));

        let mut app = factory.create_with(TestModule).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", port))
            .unwrap();
        app.start().await.unwrap();
    });

    local
        .run_until(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let client = reqwest::Client::new();

            // ================================================================
            // TEST 1: Three-level hierarchy (global + controller + method)
            // ================================================================
            tracker.clear();

            let response = client
                .get(format!("http://127.0.0.1:{}/api/three-level", port))
                .send()
                .await
                .expect("Failed to call three-level endpoint");

            assert_eq!(response.status(), 200);

            let order = tracker.get_events();
            println!("Three-level execution order: {:?}", order);

            // Verify execution order: middleware wraps global → controller → method
            // Global middleware is the outermost layer, entering before any enhancer
            assert_eq!(order[0], "middleware:global:before");

            // Guards execute in order
            assert_eq!(order[1], "guard:global");
            assert_eq!(order[2], "guard:controller");
            assert_eq!(order[3], "guard:method");

            // Interceptors execute: global:before → controller:before → method:before → handler → method:after → controller:after → global:after
            assert_eq!(order[4], "interceptor:global:before");
            assert_eq!(order[5], "interceptor:controller:before");
            assert_eq!(order[6], "interceptor:method:before");

            // Controller
            assert_eq!(order[7], "controller:three_level");

            // Interceptors after (reverse order)
            assert_eq!(order[8], "interceptor:method:after");
            assert_eq!(order[9], "interceptor:controller:after");
            assert_eq!(order[10], "interceptor:global:after");

            // Global middleware closes last, after the whole pipeline unwinds
            assert_eq!(order[11], "middleware:global:after");

            // ================================================================
            // TEST 2: Two-level hierarchy (global + controller only)
            // ================================================================
            tracker.clear();

            let response = client
                .get(format!("http://127.0.0.1:{}/api/two-level", port))
                .send()
                .await
                .expect("Failed to call two-level endpoint");

            assert_eq!(response.status(), 200);

            let order = tracker.get_events();
            println!("Two-level execution order: {:?}", order);

            // Should only have global and controller enhancers, no method-level;
            // global middleware still wraps the outside
            assert_eq!(order[0], "middleware:global:before");
            assert_eq!(order[1], "guard:global");
            assert_eq!(order[2], "guard:controller");
            assert_eq!(order[3], "interceptor:global:before");
            assert_eq!(order[4], "interceptor:controller:before");
            assert_eq!(order[5], "controller:two_level");
            assert_eq!(order[6], "interceptor:controller:after");
            assert_eq!(order[7], "interceptor:global:after");
            assert_eq!(order[8], "middleware:global:after");

            // ================================================================
            // TEST 3: Duplicate enhancers (GlobalGuard appears twice)
            // ================================================================
            tracker.clear();

            let response = client
                .get(format!("http://127.0.0.1:{}/api/duplicate", port))
                .send()
                .await
                .expect("Failed to call duplicate endpoint");

            assert_eq!(response.status(), 200);

            let order = tracker.get_events();
            println!("Duplicate execution order: {:?}", order);

            // GlobalGuard should execute TWICE: once from global, once from method
            let global_guard_count = order.iter().filter(|e| *e == "guard:global").count();
            assert_eq!(global_guard_count, 2, "GlobalGuard should execute twice");

            // Verify order: middleware → global (factory) → controller → method (also global)
            assert_eq!(order[0], "middleware:global:before"); // Outermost
            assert_eq!(order[1], "guard:global"); // From factory
            assert_eq!(order[2], "guard:controller"); // From controller
            assert_eq!(order[3], "guard:global"); // From method (duplicate)
        })
        .await;
}
