//! An application crosses threads: it is spawned as an ordinary task, and its DI root
//! resolves from a worker that did not build it.
//!
//! Neither is a claim about throughput. No bound on the serve path changed, and how many
//! requests run at once is the runtime's business. What was blocked was composition: the
//! application could not cross a thread boundary, so a server and a client shared a process
//! only with scaffolding, and a job could not resolve a provider off the thread that built
//! the container.
//!
//! Every test here runs on a multi-thread runtime and spawns with `tokio::spawn`. A
//! `LocalSet` anywhere in this file would defeat its purpose.

use std::sync::Arc;

use serial_test::serial;
use ulo::di::Execution;
use ulo::http::Body;
use ulo::{UloFactory, controller, get, injectable, module, new, routes};
use ulo_http_axum::AxumAdapter;

#[injectable]
pub struct Greeter {}

impl Greeter {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    pub fn greeting(&self) -> &'static str {
        "served from a spawned task"
    }
}

#[controller("/send")]
pub struct SendController {
    #[inject]
    greeter: Greeter,
}

#[routes]
impl SendController {
    #[get("/who")]
    fn who(&self) -> Body {
        Body::text(self.greeter.greeting())
    }
}

#[module(controllers: [SendController], providers: [Greeter])]
struct SendModule;

#[serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_application_serves_from_a_spawned_task() {
    let mut app = UloFactory::create(SendModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
        .unwrap();

    let bound = app.bind().await.unwrap();
    let addr = bound.http.expect("HTTP not bound");
    let shutdown = app.shutdown_handle();

    // The point of the test: `tokio::spawn` takes a `Send` future, and `app` is moved into
    // one. Under `!Send` this line is what did not compile.
    let serving = tokio::spawn(app.run());

    let body = reqwest::get(format!("http://{addr}/send/who"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "served from a spawned task");

    shutdown.shutdown();
    serving.await.unwrap();
}

/// The DI root with nothing served, resolved from a task the runtime may place on any
/// worker. This is the shape a CLI command, a job or a background worker builds.
#[serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_context_resolves_from_another_thread() {
    let ctx = Arc::new(
        UloFactory::create_application_context(SendModule)
            .await
            .unwrap(),
    );

    let mut resolved = Vec::new();
    for _ in 0..4 {
        let ctx = Arc::clone(&ctx);
        resolved.push(tokio::spawn(async move {
            let execution = Execution::standalone();
            let greeter = ctx.resolve::<Greeter>(&execution).await.unwrap();
            greeter.greeting()
        }));
    }

    for task in resolved {
        assert_eq!(task.await.unwrap(), "served from a spawned task");
    }
}

/// The bootstrap crosses threads too, not only the serving that follows it.
///
/// `create` and `bind` are awaited *inside* the spawned task here. The first test builds the
/// application before spawning, so a lock guard held across an await in the DI loader passes
/// it and fails only in a crate that boots this way — which is how one reached the RPC
/// conformance harness rather than this file.
#[serial]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_application_is_built_and_bound_inside_a_spawned_task() {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel();

    let serving = tokio::spawn(async move {
        let mut app = UloFactory::create(SendModule).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let shutdown = app.shutdown_handle();
        addr_tx
            .send((bound.http.expect("HTTP not bound"), shutdown))
            .unwrap();
        app.run().await;
    });

    let (addr, shutdown) = addr_rx.await.unwrap();
    let body = reqwest::get(format!("http://{addr}/send/who"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "served from a spawned task");

    shutdown.shutdown();
    serving.await.unwrap();
}
