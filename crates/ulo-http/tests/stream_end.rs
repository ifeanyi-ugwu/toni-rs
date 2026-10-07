//! A streaming response body reports its end to `on_stream_end` however the response was built,
//! including a `Response::new(HttpBody::stream(..))` handed over without `into_reply`, as an error
//! handler or an interceptor answers: `Completed` once the backend has written it to its end,
//! `CutOff(Disconnected)` when the backend drops it before, the peer having left. A `HEAD` answered
//! by the `GET` handler drops the stream unwritten, and reports it cut off.

mod common;

use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream;
use ulo::{
    App, AppHandle, BoxError, CancelReason, ErrorHandler, Interceptor, Module, ModuleDef, ModuleIdentity, Next, Signal, StreamOutcome,
    injectable, routes,
};
use ulo_http::{AppService, Bytes, ConnInfo, Http, HttpBody, HttpCx, Request, Response, StatusCode};
use ulo_transport::{CallError, ErrorKind};

use common::{Keeper, written};

const PATIENCE: Duration = Duration::from_secs(5);

/// How the answer's stream ended, as its `on_stream_end` callback received it.
#[derive(Clone, Default)]
struct Ended(Arc<Mutex<Option<StreamOutcome>>>);

impl Ended {
    fn get(&self) -> Option<StreamOutcome> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A two-chunk stream as a body, in a response built without `into_reply`, its end recorded in
/// [`Ended`].
async fn untracked(cx: &HttpCx) -> Result<Response, BoxError> {
    let ended = cx.exec().get::<Ended>().await?;
    let ended = Ended::clone(&ended);
    cx.exec().on_stream_end(move |outcome| *ended.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome));
    let chunks = [Ok::<_, Infallible>(Bytes::from_static(b"first ")), Ok(Bytes::from_static(b"second"))];
    Ok(Response::new(HttpBody::stream(stream::iter(chunks))))
}

/// Claims every error with [`untracked`]'s response.
struct Claim;

impl ErrorHandler<Http> for Claim {
    async fn handle(&self, _err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        untracked(cx).await
    }
}

/// Answers with [`untracked`]'s response instead of running the handler.
struct Answer;

impl Interceptor<Http> for Answer {
    async fn intercept(&self, cx: &HttpCx, _next: Next<'_, Http>) -> Result<Response, BoxError> {
        untracked(cx).await
    }
}

#[injectable]
struct Streams;

#[routes]
impl Streams {
    #[ulo_http::get("/claimed")]
    #[error_handlers(value = Claim)]
    async fn claimed(&self) -> Result<&'static str, CallError> {
        Err(CallError::new(ErrorKind::Unavailable, "refused"))
    }

    #[ulo_http::get("/intercepted")]
    #[interceptors(value = Answer)]
    async fn intercepted(&self) -> &'static str {
        "the handler ran"
    }
}

struct Root(Ended);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Streams>();
    }
}

struct Running {
    svc: AppService,
    ended: Ended,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start() -> Running {
        let keeper = Keeper::default();
        let ended = Ended::default();
        let app = App::builder(Root(ended.clone()))
            .timer(ulo_tokio::Timer)
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()))
            .listen()
            .await
            .expect("the app listens");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { svc: keeper.service(), ended, handle, serving }
    }

    /// `GET path`, answered by the service: the response with its body not yet read.
    async fn call(&self, path: &str) -> Response {
        self.request(http::Method::GET, path).await
    }

    async fn request(&self, method: http::Method, path: &str) -> Response {
        let (head, ()) = http::Request::builder().method(method).uri(path).body(()).expect("the request builds").into_parts();
        let req = Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
        tokio::time::timeout(PATIENCE, self.svc.call(req)).await.expect("the service answered")
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

async fn completes_once_written(path: &str) {
    let running = Running::start().await;

    let response = running.call(path).await;
    assert_eq!(running.ended.get(), None, "{path}: the stream's end was reported before the backend wrote it");
    let (status, body) = written(response).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, b"first second".as_slice()), "{path}");
    assert_eq!(running.ended.get(), Some(StreamOutcome::Completed), "{path}: the stream written to its end");
    running.stop().await;
}

async fn cut_off_when_dropped(path: &str) {
    let running = Running::start().await;

    let response = running.call(path).await;
    drop(response);
    assert_eq!(
        running.ended.get(),
        Some(StreamOutcome::CutOff(Some(CancelReason::Disconnected))),
        "{path}: the stream the peer left before its end"
    );
    running.stop().await;
}

#[tokio::test]
async fn an_error_handlers_stream_reports_completed_once_written() {
    completes_once_written("/claimed").await;
}

#[tokio::test]
async fn an_error_handlers_stream_dropped_before_its_end_reports_cut_off() {
    cut_off_when_dropped("/claimed").await;
}

#[tokio::test]
async fn an_interceptors_stream_reports_completed_once_written() {
    completes_once_written("/intercepted").await;
}

#[tokio::test]
async fn an_interceptors_stream_dropped_before_its_end_reports_cut_off() {
    cut_off_when_dropped("/intercepted").await;
}

#[tokio::test]
async fn a_head_answer_reports_its_unwritten_stream_cut_off() {
    let running = Running::start().await;

    let (status, body) = written(running.request(http::Method::HEAD, "/claimed").await).await;
    assert_eq!((status, body.len()), (StatusCode::OK, 0), "a `HEAD` answer carries no body");
    match running.ended.get() {
        Some(StreamOutcome::CutOff(_)) => {}
        other => panic!("the stream a `HEAD` answer dropped unwritten reported {other:?}, not `CutOff`"),
    }
    running.stop().await;
}
