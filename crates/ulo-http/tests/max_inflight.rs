//! The server's in-flight bound: a request over it is answered 503 with `Retry-After` before any
//! routing, and `Count::Default` bounds at `Count::DEFAULT_MAX_INFLIGHT`, 1,024 requests.
//!
//! Each held request waits in its handler on a gate the test opens; the test calls the service as
//! a backend would.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use ulo::{App, AppHandle, Dep, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::{AppService, ConnInfo, HttpBody, Request, Response, StatusCode};
use ulo_transport::Count;

use common::{Keeper, written};

const PATIENCE: Duration = Duration::from_secs(10);

/// Held requests count themselves in `held` and wait until `open` fires.
#[derive(Clone, Default)]
struct Gate {
    held: Arc<AtomicUsize>,
    open: Arc<Notify>,
}

#[injectable]
struct Held {
    gate: Dep<Gate>,
}

#[routes]
impl Held {
    #[ulo_http::get("/held")]
    async fn held(&self) -> &'static str {
        let opened = self.gate.open.notified();
        self.gate.held.fetch_add(1, Ordering::SeqCst);
        opened.await;
        "released"
    }

    #[ulo_http::get("/fast")]
    async fn fast(&self) -> &'static str {
        "fast"
    }
}

struct Root(Gate);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Held>();
    }
}

struct Running {
    svc: AppService,
    gate: Gate,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start(max_inflight: Count) -> Running {
        let keeper = Keeper::default();
        let gate = Gate::default();
        let app = App::builder(Root(gate.clone()))
            .timer(ulo_tokio::Timer)
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()).max_inflight(max_inflight))
            .listen()
            .await
            .expect("the app listens");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { svc: keeper.service(), gate, handle, serving }
    }

    fn get(&self, path: &str) -> impl Future<Output = Response> + Send + use<> {
        let (head, ()) = http::Request::builder().uri(path).body(()).expect("the request builds").into_parts();
        let req = Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
        self.svc.call(req)
    }

    /// `n` requests to `/held`, each in a task of its own, once every one is in its handler.
    async fn hold(&self, n: usize) -> Vec<tokio::task::JoinHandle<Response>> {
        let tasks: Vec<_> = (0..n).map(|_| tokio::spawn(self.get("/held"))).collect();
        let held = Arc::clone(&self.gate.held);
        tokio::time::timeout(PATIENCE, async move {
            while held.load(Ordering::SeqCst) < n {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{n} requests did not all reach the held handler"));
        tasks
    }

    /// Opens the gate and asserts every held request was answered.
    async fn release(&self, tasks: Vec<tokio::task::JoinHandle<Response>>) {
        self.gate.open.notify_waiters();
        for task in tasks {
            let response = tokio::time::timeout(PATIENCE, task).await.expect("a held request was answered").expect("its task completes");
            assert_eq!(response.status(), StatusCode::OK, "a held request within the bound was not answered 200");
        }
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

/// A request over the bound: 503 with `Retry-After`, the server's `shed_retry_after`, one second.
async fn assert_shed(response: Response) {
    let retry_after = response.headers().get(http::header::RETRY_AFTER).cloned();
    let (status, body) = written(response).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "a request over the bound was answered: {}", String::from_utf8_lossy(&body));
    assert_eq!(retry_after.as_ref().map(|value| value.as_bytes()), Some(b"1".as_slice()), "the refusal carries no `Retry-After: 1`");
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_over_the_bound_is_503_with_retry_after() {
    let running = Running::start(Count::Max(2)).await;
    let held = running.hold(2).await;
    assert_shed(tokio::time::timeout(PATIENCE, running.get("/fast")).await.expect("the service answered")).await;
    running.release(held).await;
    let (status, _) = written(running.get("/fast").await).await;
    assert_eq!(status, StatusCode::OK, "a request within the bound once the held ones ended was not answered");
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_default_bound_is_1024_requests() {
    let bound = usize::try_from(Count::DEFAULT_MAX_INFLIGHT).expect("the bound fits a usize");
    assert_eq!(bound, 1024);
    let running = Running::start(Count::Default).await;
    let held = running.hold(bound).await;
    assert_shed(tokio::time::timeout(PATIENCE, running.get("/fast")).await.expect("the service answered")).await;
    running.release(held).await;
    running.stop().await;
}
