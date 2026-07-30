//! `ErrorObserver` — universal fire-and-forget observation hook.
//!
//! Verifies the observer fires on framework-generated errors (guard
//! rejection here) but **not** on user-handler errors (those render via
//! the active transport rendering and bypass the chain entirely).

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use toni::{
    async_trait,
    context::HttpContext,
    controller,
    errors::HttpError,
    get, module, routes,
    toni_factory::ToniFactory,
    traits_helpers::{ErrorObserver, Guard},
    Body as ToniBody,
};
use toni_axum::AxumAdapter;
use toni_macros::use_guards;

struct CountingObserver {
    count: Arc<AtomicUsize>,
    last_message: Arc<std::sync::Mutex<String>>,
}

#[async_trait]
impl ErrorObserver for CountingObserver {
    async fn observe<'a>(
        &'a self,
        error: &'a (dyn std::error::Error + Send + Sync + 'static),
        _ctx: &'a mut (dyn toni::context::HandlerContext + 'a),
    ) {
        self.count.fetch_add(1, Ordering::SeqCst);
        *self.last_message.lock().unwrap() = error.to_string();
    }
}

struct AlwaysReject;

#[async_trait]
impl Guard<HttpContext> for AlwaysReject {
    async fn can_activate(&self, _ctx: &mut HttpContext) -> bool {
        false
    }
}

async fn start_app(
    module: impl toni::ModuleMetadata + 'static,
    observer: Arc<CountingObserver>,
) -> std::net::SocketAddr {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        factory.use_global_error_observer(observer);
        let mut app = factory.create_with(module).await;
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

#[tokio_localset_test::localset_test]
async fn observer_fires_on_guard_rejection() {
    #[controller("/api")]
    pub struct GuardedController {}

    #[routes]
    impl GuardedController {
        #[get("/protected")]
        #[use_guards(AlwaysReject {})]
        fn protected(&self) -> Result<ToniBody, HttpError> {
            Ok(ToniBody::text("should not reach"))
        }
    }

    #[module(controllers: [GuardedController], providers: [])]
    impl GuardedModule {}

    let count = Arc::new(AtomicUsize::new(0));
    let last_message = Arc::new(std::sync::Mutex::new(String::new()));
    let observer = Arc::new(CountingObserver {
        count: count.clone(),
        last_message: last_message.clone(),
    });

    let addr = start_app(GuardedModule, observer).await;

    let resp = reqwest::get(format!("http://{}/api/protected", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // Observer fires on the framework-generated 403.
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(
        !last_message.lock().unwrap().is_empty(),
        "observer should have captured the error message",
    );
}

#[tokio_localset_test::localset_test]
async fn observer_fires_on_user_error() {
    // User errors render via `HttpError::to_response`, but the
    // dispatcher preserves the typed error past that boundary so observers
    // can see it too. Symmetric semantics: observers fire on every error,
    // user-typed and framework-generated alike.
    #[controller("/api")]
    pub struct UserErrController {}

    #[routes]
    impl UserErrController {
        #[get("/missing")]
        fn missing(&self) -> Result<ToniBody, HttpError> {
            Err(HttpError::not_found("user-error"))
        }
    }

    #[module(controllers: [UserErrController], providers: [])]
    impl UserErrModule {}

    let count = Arc::new(AtomicUsize::new(0));
    let last_message = Arc::new(std::sync::Mutex::new(String::new()));
    let observer = Arc::new(CountingObserver {
        count: count.clone(),
        last_message: last_message.clone(),
    });

    let addr = start_app(UserErrModule, observer).await;

    let resp = reqwest::get(format!("http://{}/api/missing", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "observer should have fired on the user-handler error",
    );
    assert!(
        last_message.lock().unwrap().contains("user-error"),
        "observer should have captured the user error message",
    );
}
