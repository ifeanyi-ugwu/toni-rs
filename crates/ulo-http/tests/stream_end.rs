//! A streaming response body reports its end to `on_stream_end` however the response was built,
//! including a `Response::new(HttpBody::stream(..))` handed over without `into_reply`, as an error
//! handler or an interceptor answers: `Completed` once the backend has written it to its end,
//! `CutOff(Disconnected)` when the backend drops it before, the peer having left. An `Sse` reports
//! the same way, and `CutOff(None)` when it ends in an `error` event. A response that owes the
//! client no body reports its stream `Completed` though the backend never writes it: a `HEAD`
//! answer, whether a `GET` or a `HEAD` handler built it, with no log, and a 204 or 304, with a
//! `warn` naming the status.
//!
//! The tests run on a current-thread runtime and call the service on the test's task, so the
//! service's events reach the [`Capture`] the test installs as the thread's default subscriber.

mod common;

use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::{
    App, AppHandle, BoxError, CancelReason, Dep, ErrorHandler, ExecutionRef, Interceptor, Module, ModuleDef, ModuleIdentity, Next, Signal,
    StreamOutcome, injectable, routes,
};
use ulo_http::{AppService, Bytes, ConnInfo, Event, Http, HttpBody, HttpCx, Request, Response, Sse, StatusCode};
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

/// Records the end of the execution's stream in `ended`.
fn record_end(exec: &ExecutionRef, ended: &Ended) {
    let ended = ended.clone();
    exec.on_stream_end(move |outcome| *ended.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome));
}

/// A two-chunk stream as the body of a response of `status`, built without `into_reply`, its end
/// recorded in `ended`.
fn two_chunks(exec: &ExecutionRef, ended: &Ended, status: StatusCode) -> Response {
    record_end(exec, ended);
    let chunks = [Ok::<_, Infallible>(Bytes::from_static(b"first ")), Ok(Bytes::from_static(b"second"))];
    let mut response = Response::new(HttpBody::stream(stream::iter(chunks)));
    *response.status_mut() = status;
    response
}

/// [`two_chunks`] with status 200, its end recorded in the app's [`Ended`].
async fn untracked(cx: &HttpCx) -> Result<Response, BoxError> {
    let ended = cx.exec().get::<Ended>().await?;
    Ok(two_chunks(cx.exec(), &ended, StatusCode::OK))
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
struct Streams {
    ended: Dep<Ended>,
}

#[routes]
impl Streams {
    #[ulo_http::head("/head")]
    async fn head(&self, exec: ExecutionRef) -> Response {
        two_chunks(&exec, &self.ended, StatusCode::OK)
    }

    #[ulo_http::get("/no-content")]
    async fn no_content(&self, exec: ExecutionRef) -> Response {
        two_chunks(&exec, &self.ended, StatusCode::NO_CONTENT)
    }

    #[ulo_http::get("/not-modified")]
    async fn not_modified(&self, exec: ExecutionRef) -> Response {
        two_chunks(&exec, &self.ended, StatusCode::NOT_MODIFIED)
    }

    #[ulo_http::get("/events")]
    async fn events(&self, exec: ExecutionRef) -> Sse<impl futures_util::Stream<Item = Event> + Send + use<>> {
        record_end(&exec, &self.ended);
        Sse::new(stream::iter([Event::default().data("first"), Event::default().data("second")]))
    }

    #[ulo_http::get("/events-failing")]
    async fn events_failing(&self, exec: ExecutionRef) -> Sse<impl futures_util::Stream<Item = Result<Event, CallError>> + Send + use<>> {
        record_end(&exec, &self.ended);
        Sse::new(stream::iter([Ok(Event::default().data("first")), Err(CallError::new(ErrorKind::Unavailable, "gone"))]))
    }

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
            .runtime(ulo_tokio::Tokio::current())
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
async fn an_sse_stream_reports_completed_once_written() {
    let running = Running::start().await;

    let response = running.call("/events").await;
    assert_eq!(running.ended.get(), None, "the stream's end was reported before the backend wrote it");
    let (status, body) = written(response).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, b"data: first\n\ndata: second\n\n".as_slice()));
    assert_eq!(running.ended.get(), Some(StreamOutcome::Completed), "the event stream written to its end");
    running.stop().await;
}

#[tokio::test]
async fn an_sse_stream_ending_in_an_error_event_reports_cut_off() {
    let running = Running::start().await;

    let (status, body) = written(running.call("/events-failing").await).await;
    let body = String::from_utf8_lossy(&body);
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("data: first\n\nevent: error\n"), "the stream did not end in an `error` event: {body:?}");
    assert_eq!(
        running.ended.get(),
        Some(StreamOutcome::CutOff(None)),
        "a stream ended by an `error` event, though it then returned `None`"
    );
    running.stop().await;
}

/// `method path` answered with a status that owes no body: the response dropped unread, as a
/// backend drops a body it never writes, and what the stream reported and the `warn` lines
/// returned.
async fn owes_no_body(method: http::Method, path: &str) -> (StatusCode, Option<StreamOutcome>, Vec<String>) {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let response = running.request(method, path).await;
    let status = response.status();
    drop(response);
    let ended = running.ended.get();
    running.stop().await;
    (status, ended, capture.warnings())
}

#[tokio::test]
async fn a_head_answered_by_the_get_handler_reports_its_dropped_stream_completed() {
    let (status, ended, warnings) = owes_no_body(http::Method::HEAD, "/claimed").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ended, Some(StreamOutcome::Completed), "the `GET` answer's stream a `HEAD` answer dropped");
    assert!(warnings.is_empty(), "a `HEAD` answer logged: {warnings:?}");
}

#[tokio::test]
async fn a_head_handlers_stream_reports_completed() {
    let (status, ended, warnings) = owes_no_body(http::Method::HEAD, "/head").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ended, Some(StreamOutcome::Completed), "the stream a `HEAD` handler answered");
    assert!(warnings.is_empty(), "a `HEAD` answer logged: {warnings:?}");
}

async fn forbidden_by_status(path: &str, expected: StatusCode) {
    let (status, ended, warnings) = owes_no_body(http::Method::GET, path).await;
    assert_eq!(status, expected, "{path}");
    assert_eq!(ended, Some(StreamOutcome::Completed), "{path}: the stream its status forbids");
    let discarded = format!("a response carried a streaming body its status forbids; the body was discarded status={}", expected.as_u16());
    assert!(warnings.iter().any(|line| line.starts_with(&discarded)), "{path}: no `warn` that the body was discarded: {warnings:?}");
}

#[tokio::test]
async fn a_204_carrying_a_stream_reports_completed_and_warns() {
    forbidden_by_status("/no-content", StatusCode::NO_CONTENT).await;
}

#[tokio::test]
async fn a_304_carrying_a_stream_reports_completed_and_warns() {
    forbidden_by_status("/not-modified", StatusCode::NOT_MODIFIED).await;
}

/// Every `warn` and `error` event on the thread it is the default subscriber of, as its message
/// followed by `name=value` for each other field.
#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<String>>>,
    next_span: Arc<AtomicU64>,
}

impl Capture {
    fn warnings(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_span() || *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &TraceEvent<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push(format!("{}{}", line.message, line.fields));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields.push_str(&format!(" {}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}
