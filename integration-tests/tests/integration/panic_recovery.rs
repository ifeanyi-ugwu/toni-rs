//! `PanicRecovered` flow + observer-panic isolation across HTTP.
//!
//! Two contracts:
//!
//! 1. A panicking handler doesn't tear down the dispatcher — the unwind is
//!    caught, surfaced as `PanicRecovered`, fanned through observers + the
//!    chain, and rendered through `HttpError::to_response`.
//! 2. A panicking observer doesn't propagate either — the dispatcher swallows
//!    the unwind (logging it via tracing) and continues to subsequent
//!    observers + the chain so one bad logger doesn't kill the request.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use toni::{
    Body as ToniBody, HttpResponse, async_trait, context::HttpContext, controller,
    errors::{HttpError, PanicRecovered, PipelineSegment},
    get, module, toni_factory::ToniFactory,
    traits_helpers::{ErrorObserver, Guard, Interceptor, InterceptorNext},
};
use toni_axum::AxumAdapter;
use toni_macros::{use_guards, use_interceptors};

struct CountingObserver {
    count: Arc<AtomicUsize>,
    captured_segment: Arc<std::sync::Mutex<Option<PipelineSegment>>>,
}

#[async_trait]
impl ErrorObserver for CountingObserver {
    async fn observe<'a>(
        &'a self,
        error: &'a (dyn std::error::Error + Send + Sync + 'static),
        _ctx: &'a (dyn toni::context::HandlerContext + 'a),
    ) {
        self.count.fetch_add(1, Ordering::SeqCst);
        if let Some(panic) = error.downcast_ref::<PanicRecovered>() {
            *self.captured_segment.lock().unwrap() = Some(panic.during);
        }
    }
}

/// Observer that always panics — verifies the dispatcher catches it and
/// keeps going.
struct PanickingObserver {
    other_count: Arc<AtomicUsize>,
}

#[async_trait]
impl ErrorObserver for PanickingObserver {
    async fn observe<'a>(
        &'a self,
        _error: &'a (dyn std::error::Error + Send + Sync + 'static),
        _ctx: &'a (dyn toni::context::HandlerContext + 'a),
    ) {
        // Bump the other-counter via side effect so the test can detect this
        // observer was reached even though it panics afterwards.
        self.other_count.fetch_add(1, Ordering::SeqCst);
        panic!("observer panic on purpose");
    }
}

async fn start_app(
    module: toni::module_helpers::module_enum::ModuleDefinition,
    observers: Vec<Arc<dyn ErrorObserver>>,
) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        for o in observers {
            factory.use_global_error_observer(o);
        }
        let mut app = factory.create_with(module).await;
        app.use_http_adapter(AxumAdapter::new(), 0, "127.0.0.1")
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = addr_tx.send(bound.http.expect("HTTP adapter not bound"));
        app.run().await;
    });

    tokio::task::spawn_local(async move {
        local.await;
    });

    addr_rx.await.unwrap()
}

#[tokio_localset_test::localset_test]
async fn panicking_handler_renders_500_via_panic_recovered() {
    #[controller("/api", pub struct PanicController {})]
    impl PanicController {
        #[get("/boom")]
        fn boom(&self) -> Result<ToniBody, HttpError> {
            panic!("kaboom");
        }
    }

    #[module(controllers: [PanicController], providers: [])]
    impl PanicModule {}

    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(CountingObserver {
        count: count.clone(),
        captured_segment: captured.clone(),
    });

    let addr = start_app(PanicModule::module_definition(), vec![observer]).await;

    let resp = reqwest::get(format!("http://{}/api/boom", addr))
        .await
        .unwrap();

    // Default `PanicRecovered` kind is Internal → 500.
    assert_eq!(resp.status().as_u16(), 500);

    // Observer fired exactly once with a `PanicRecovered` carrying the
    // `HandlerBody` segment.
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::HandlerBody));

    // Body carries the panic message in the canonical envelope.
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 500);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("kaboom"),
        "panic message should surface in the envelope, got: {body}",
    );
}

struct AlwaysReject;

#[async_trait]
impl Guard<HttpContext> for AlwaysReject {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        false
    }
}

#[tokio_localset_test::localset_test]
async fn panicking_observer_does_not_break_dispatch() {
    // A guard rejection produces a `GuardRejection`. We register two
    // observers: a panicking one and a counter. The panicker fires first
    // (later registration → higher priority via reverse iteration), panics,
    // gets caught; the counter still observes the same error.
    #[controller("/api", pub struct GuardedController {})]
    impl GuardedController {
        #[get("/protected")]
        #[use_guards(AlwaysReject {})]
        fn protected(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("unreachable"))
        }
    }

    #[module(controllers: [GuardedController], providers: [])]
    impl GuardedModule {}

    let panicker_hits = Arc::new(AtomicUsize::new(0));
    let counter_hits = Arc::new(AtomicUsize::new(0));
    let panicker = Arc::new(PanickingObserver {
        other_count: panicker_hits.clone(),
    });
    let counter = Arc::new(CountingObserver {
        count: counter_hits.clone(),
        captured_segment: Arc::new(std::sync::Mutex::new(None)),
    });

    let addr = start_app(
        GuardedModule::module_definition(),
        vec![panicker, counter],
    )
    .await;

    let resp = reqwest::get(format!("http://{}/api/protected", addr))
        .await
        .unwrap();

    // Guard rejection still produces 403 — observer panic doesn't break
    // the dispatch path.
    assert_eq!(resp.status().as_u16(), 403);

    // Both observers were reached. The panicking one bumped its counter
    // before panicking, the counting one ran after.
    assert_eq!(panicker_hits.load(Ordering::SeqCst), 1);
    assert_eq!(counter_hits.load(Ordering::SeqCst), 1);
}

struct PanickingGuard;

#[async_trait]
impl Guard<HttpContext> for PanickingGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        panic!("guard kaboom");
    }
}

/// A panicking guard surfaces as 500 via the standard `PanicRecovered`
/// envelope. The observer sees the typed event tagged
/// `PipelineSegment::Guard` so logging / telemetry can distinguish a
/// guard panic from a handler panic.
#[tokio_localset_test::localset_test]
async fn panicking_guard_renders_500_via_panic_recovered() {
    #[controller("/api", pub struct PanicGuardController {})]
    impl PanicGuardController {
        #[get("/guarded")]
        #[use_guards(PanickingGuard {})]
        fn guarded(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("unreachable"))
        }
    }

    #[module(controllers: [PanicGuardController], providers: [])]
    impl PanicGuardModule {}

    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(CountingObserver {
        count: count.clone(),
        captured_segment: captured.clone(),
    });

    let addr = start_app(PanicGuardModule::module_definition(), vec![observer]).await;

    let resp = reqwest::get(format!("http://{}/api/guarded", addr))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 500);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Guard));

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("guard kaboom"),
        "panic message should surface in the envelope, got: {body}",
    );
}

struct PanickingInterceptor;

#[async_trait]
impl Interceptor<HttpContext> for PanickingInterceptor {
    async fn intercept(
        &self,
        _ctx: &mut HttpContext,
        _next: Box<dyn InterceptorNext<HttpContext>>,
    ) {
        panic!("interceptor kaboom");
    }
}

/// A panicking interceptor surfaces as 500 via the standard
/// `PanicRecovered` envelope. The observer sees the typed event tagged
/// `PipelineSegment::Middleware` (the interceptor chain shares the
/// middleware segment label) so a slow logger can sort logs by stage.
#[tokio_localset_test::localset_test]
async fn panicking_interceptor_renders_500_via_panic_recovered() {
    #[controller("/api", pub struct PanicInterceptorController {})]
    impl PanicInterceptorController {
        #[get("/intercepted")]
        #[use_interceptors(PanickingInterceptor {})]
        fn intercepted(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("unreachable"))
        }
    }

    #[module(controllers: [PanicInterceptorController], providers: [])]
    impl PanicInterceptorModule {}

    let count = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(None));
    let observer = Arc::new(CountingObserver {
        count: count.clone(),
        captured_segment: captured.clone(),
    });

    let addr = start_app(PanicInterceptorModule::module_definition(), vec![observer]).await;

    let resp = reqwest::get(format!("http://{}/api/intercepted", addr))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 500);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(*captured.lock().unwrap(), Some(PipelineSegment::Middleware));

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("interceptor kaboom"),
        "panic message should surface in the envelope, got: {body}",
    );
}
