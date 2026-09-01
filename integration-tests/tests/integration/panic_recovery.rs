//! `PanicRecovered` flow across HTTP.
//!
//! A panicking pipeline segment doesn't tear down the dispatcher: the unwind
//! is caught, surfaced as a typed `PanicRecovered` carrying the segment it
//! happened in, offered to the error-handler chain, and rendered through
//! `HttpError::to_response` when nothing claims it.
//!
//! Two segments are the exception, because they are the chain's own
//! machinery: a panicking `#[catch]` handler and a panicking renderer reach
//! no handler at all. Those are logged and the pipeline carries on — the next
//! handler for the first, a hardcoded envelope for the second.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use toni::{
    async_trait,
    context::HttpContext,
    controller,
    errors::{ErrorKind, HttpError, PanicRecovered, PipelineSegment},
    get, module, routes,
    toni_factory::ToniFactory,
    traits_helpers::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext},
    Body as ToniBody, HttpResponse,
};
use toni_axum::AxumAdapter;
use toni_macros::{use_error_handlers, use_guards, use_interceptors};

/// Records the segment of every `PanicRecovered` the chain hands it, then
/// declines so the default rendering still runs. A declining `#[catch]`-style
/// handler is the seam application code has on these events.
macro_rules! recording_handler {
    ($name:ident, $sink:ident) => {
        static $sink: Mutex<Vec<PipelineSegment>> = Mutex::new(Vec::new());

        struct $name;

        #[async_trait]
        impl ErrorHandler<HttpContext, HttpResponse> for $name {
            async fn handle_error(
                &self,
                error: ChainError<'_>,
                _ctx: &HttpContext,
            ) -> Option<HttpResponse> {
                if let Some(panic) = error.downcast_ref::<PanicRecovered>() {
                    $sink.lock().unwrap().push(panic.during);
                }
                None
            }
        }
    };
}

async fn start_app(module: impl toni::ModuleMetadata + 'static) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let factory = ToniFactory::new();
        let mut app = factory.create_with(module).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
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

recording_handler!(HandlerSegmentRecorder, HANDLER_SEGMENTS);

#[tokio_localset_test::localset_test]
async fn panicking_handler_renders_500_via_panic_recovered() {
    #[controller("/api")]
    pub struct PanicController {}

    #[routes]
    impl PanicController {
        #[get("/boom")]
        #[use_error_handlers(HandlerSegmentRecorder {})]
        fn boom(&self) -> Result<ToniBody, HttpError> {
            panic!("kaboom");
        }
    }

    #[module(controllers: [PanicController], providers: [])]
    impl PanicModule {}

    let addr = start_app(PanicModule).await;

    let resp = reqwest::get(format!("http://{}/api/boom", addr))
        .await
        .unwrap();

    // Default `PanicRecovered` kind is Internal → 500.
    assert_eq!(resp.status().as_u16(), 500);

    // The chain saw one typed event, carrying the `HandlerBody` segment.
    assert_eq!(
        HANDLER_SEGMENTS.lock().unwrap().clone(),
        vec![PipelineSegment::HandlerBody]
    );

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

struct PanickingGuard;

#[async_trait]
impl Guard<HttpContext> for PanickingGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        panic!("guard kaboom");
    }
}

recording_handler!(GuardSegmentRecorder, GUARD_SEGMENTS);

/// A panicking guard surfaces as 500 via the standard `PanicRecovered`
/// envelope. The chain sees the typed event tagged `PipelineSegment::Guard`,
/// so a handler can distinguish a guard panic from a handler panic.
#[tokio_localset_test::localset_test]
async fn panicking_guard_renders_500_via_panic_recovered() {
    #[controller("/api")]
    pub struct PanicGuardController {}

    #[routes]
    impl PanicGuardController {
        #[get("/guarded")]
        #[use_guards(PanickingGuard {})]
        #[use_error_handlers(GuardSegmentRecorder {})]
        fn guarded(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("unreachable"))
        }
    }

    #[module(controllers: [PanicGuardController], providers: [])]
    impl PanicGuardModule {}

    let addr = start_app(PanicGuardModule).await;

    let resp = reqwest::get(format!("http://{}/api/guarded", addr))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 500);
    assert_eq!(
        GUARD_SEGMENTS.lock().unwrap().clone(),
        vec![PipelineSegment::Guard]
    );

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
impl Interceptor<HttpContext, HttpResponse> for PanickingInterceptor {
    async fn intercept(
        &self,
        _ctx: &HttpContext,
        _next: Box<dyn InterceptorNext<HttpContext, HttpResponse>>,
    ) -> HttpResponse {
        panic!("interceptor kaboom");
    }
}

recording_handler!(InterceptorSegmentRecorder, INTERCEPTOR_SEGMENTS);

/// A panicking interceptor surfaces as 500 via the standard
/// `PanicRecovered` envelope, tagged `PipelineSegment::Middleware` (the
/// interceptor chain shares the middleware segment label).
#[tokio_localset_test::localset_test]
async fn panicking_interceptor_renders_500_via_panic_recovered() {
    #[controller("/api")]
    pub struct PanicInterceptorController {}

    #[routes]
    impl PanicInterceptorController {
        #[get("/intercepted")]
        #[use_interceptors(PanickingInterceptor {})]
        #[use_error_handlers(InterceptorSegmentRecorder {})]
        fn intercepted(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("unreachable"))
        }
    }

    #[module(controllers: [PanicInterceptorController], providers: [])]
    impl PanicInterceptorModule {}

    let addr = start_app(PanicInterceptorModule).await;

    let resp = reqwest::get(format!("http://{}/api/intercepted", addr))
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 500);
    assert_eq!(
        INTERCEPTOR_SEGMENTS.lock().unwrap().clone(),
        vec![PipelineSegment::Middleware]
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("interceptor kaboom"),
        "panic message should surface in the envelope, got: {body}",
    );
}

struct PanickingErrorHandler;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for PanickingErrorHandler {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        panic!("error-handler kaboom");
    }
}

static CHAIN_CONTINUED: AtomicUsize = AtomicUsize::new(0);

/// Counts the errors reaching it and declines. Registered before the
/// panicking handler, so the chain reaches it only by surviving that panic.
struct ChainSurvivor;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for ChainSurvivor {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        CHAIN_CONTINUED.fetch_add(1, Ordering::SeqCst);
        None
    }
}

/// A panicking error handler must not break the chain. Policy: log the panic,
/// treat it as a `None` claim, continue to the next handler. Neither handler
/// claims here, so the fallback `HttpError::to_response` renders the original
/// error — 500, since the handler ran against a `HandlerBody` panic.
#[tokio_localset_test::localset_test]
async fn panicking_error_handler_continues_chain() {
    #[controller("/api")]
    pub struct PanicEhController {}

    #[routes]
    impl PanicEhController {
        // Chain order is reverse registration, so the panicking handler runs
        // first and the survivor after it.
        #[get("/eh")]
        #[use_error_handlers(ChainSurvivor {}, PanickingErrorHandler {})]
        fn eh(&self) -> Result<ToniBody, HttpError> {
            panic!("handler kaboom");
        }
    }

    #[module(controllers: [PanicEhController], providers: [])]
    impl PanicEhModule {}

    let addr = start_app(PanicEhModule).await;

    let resp = reqwest::get(format!("http://{}/api/eh", addr))
        .await
        .unwrap();

    // Fallback rendering still produces a 500 — nothing claimed, so the
    // original `HandlerBody` panic fell through to `HttpError::to_response`.
    assert_eq!(resp.status().as_u16(), 500);

    // The handler after the panicking one still ran.
    assert_eq!(CHAIN_CONTINUED.load(Ordering::SeqCst), 1);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("handler kaboom"),
        "fallback render should preserve the original panic message; got: {body}",
    );
}

/// Domain error whose `message()` panics — used to make
/// `HttpError::to_response` itself panic, which exercises the
/// `safe_render` wrapper.
#[derive(Debug)]
struct RenderBomb;

impl std::fmt::Display for RenderBomb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RenderBomb")
    }
}

impl std::error::Error for RenderBomb {}

impl toni::Error for RenderBomb {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Internal
    }
    fn message(&self) -> std::borrow::Cow<'_, str> {
        panic!("render kaboom");
    }
}

/// A panicking renderer must not tear down the connection. It runs below the
/// chain — the last thing between the framework and the wire — so the policy
/// is to log it and substitute a hardcoded minimal 500 envelope. That envelope
/// is built from simple constructors that don't render user data, so a
/// recursive panic here is structurally impossible.
#[tokio_localset_test::localset_test]
async fn panicking_renderer_falls_back_to_safe_envelope() {
    #[controller("/api")]
    pub struct RenderPanicController {}

    #[routes]
    impl RenderPanicController {
        #[get("/render-boom")]
        fn render_boom(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::from(RenderBomb))
        }
    }

    #[module(controllers: [RenderPanicController], providers: [])]
    impl RenderPanicModule {}

    let addr = start_app(RenderPanicModule).await;

    let resp = reqwest::get(format!("http://{}/api/render-boom", addr))
        .await
        .unwrap();

    // Fallback envelope.
    assert_eq!(resp.status().as_u16(), 500);

    let body = resp.text().await.unwrap();
    assert_eq!(
        body, "Internal Server Error",
        "fallback envelope must be the hardcoded minimal body, got: {body}",
    );
}
