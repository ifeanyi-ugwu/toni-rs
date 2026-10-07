//! The body an error handler answers a timed-out request with, as the backend sees it through
//! `AppService::call`: buffered whatever its size when it ends on the first poll, up to 1 MiB and
//! no further when the server waits on it, and, when it is a stream, its end reported to
//! `on_stream_end` when the backend has written the buffered copy, not when the service read it,
//! and `CutOff(Deadline)` when the body is dropped for the canonical 504.
//!
//! The backend here is the test's `Keeper` (`common`).

mod common;

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{StreamExt, stream};
use http_body::Body as _;
use ulo::{App, AppHandle, BoxError, Bound, CancelReason, ErrorHandler, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, StreamOutcome, injectable, routes};
use ulo_http::{AppService, Bytes, ConnInfo, Http, HttpBody, HttpCx, MB, Request, Response, StatusCode, Timeout};
use ulo_transport::{CallError, ErrorKind, IntoReply};

use common::{Keeper, written};

const DEADLINE: Duration = Duration::from_millis(50);
const GRACE: Duration = Duration::from_secs(2);

/// The chunk the large answers are streamed in.
const CHUNK: usize = 64 * 1024;

/// How the error handler's stream ended, as its `on_stream_end` callback received it.
#[derive(Clone, Default)]
struct Ended(Arc<Mutex<Option<StreamOutcome>>>);

impl Ended {
    fn get(&self) -> Option<StreamOutcome> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn timed_out(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().is_some_and(|call| call.kind() == ErrorKind::Timeout)
}

/// Answers a `Timeout` with 503 and `claimed` as a one-item stream, recording its end in [`Ended`].
struct TrackedOnTimeout;

impl ErrorHandler<Http> for TrackedOnTimeout {
    async fn handle(&self, err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let ended = cx.exec().get::<Ended>().await?;
        let ended = Ended::clone(&ended);
        cx.exec().on_stream_end(move |outcome| *ended.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome));
        let body = HttpBody::stream(stream::iter([Ok::<_, Infallible>(Bytes::from_static(b"claimed"))]));
        let mut response = body.into_reply(cx)?;
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

/// `len` bytes in [`CHUNK`]-sized chunks, every one ready.
fn chunks(len: usize) -> impl Iterator<Item = Result<Bytes, Infallible>> + Send {
    (0..len).step_by(CHUNK).map(move |start| Ok(Bytes::from(vec![b'x'; CHUNK.min(len - start)])))
}

/// Answers a `Timeout` with 503 and a stream of `len` bytes that is pending once before its first
/// chunk, so the server waits on it; every chunk is ready after that. Its end is recorded in
/// [`Ended`].
struct WaitedOnTimeout(usize);

impl ErrorHandler<Http> for WaitedOnTimeout {
    async fn handle(&self, err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let ended = cx.exec().get::<Ended>().await?;
        let ended = Ended::clone(&ended);
        cx.exec().on_stream_end(move |outcome| *ended.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome));
        let pending_once = stream::once(tokio::task::yield_now()).filter_map(|()| async { None });
        let mut response = Response::new(HttpBody::stream(pending_once.chain(stream::iter(chunks(self.0)))));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

/// Answers a `Timeout` with 503 and a stream of `len` bytes in 512 KiB chunks, every chunk and the
/// end ready at once: a body produced before it is read.
struct ReadyOnTimeout(usize);

impl ErrorHandler<Http> for ReadyOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let len = self.0;
        let chunks = (0..len).step_by(8 * CHUNK).map(move |start| Ok::<_, Infallible>(Bytes::from(vec![b'x'; (8 * CHUNK).min(len - start)])));
        let mut response = Response::new(HttpBody::stream(stream::iter(chunks)));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

/// How many times [`CountedOnTimeout`]'s stream was polled.
static POLLS: AtomicUsize = AtomicUsize::new(0);

/// How long [`CountedOnTimeout`]'s one chunk takes to arrive, well inside the grace.
const CHUNK_DELAY: Duration = Duration::from_millis(100);

/// Answers a `Timeout` with 503 and a one-chunk stream that is pending for [`CHUNK_DELAY`],
/// counting its polls in [`POLLS`].
struct CountedOnTimeout;

impl ErrorHandler<Http> for CountedOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let mut late = Box::pin(stream::once(async {
            tokio::time::sleep(CHUNK_DELAY).await;
            Ok::<_, Infallible>(Bytes::from_static(b"late"))
        }));
        let counted = stream::poll_fn(move |cx| {
            POLLS.fetch_add(1, Ordering::Relaxed);
            late.poll_next_unpin(cx)
        });
        let mut response = Response::new(HttpBody::stream(counted));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

#[injectable]
struct Slow;

#[routes]
#[meta(Timeout::after(DEADLINE))]
impl Slow {
    #[ulo_http::get("/tracked")]
    #[error_handlers(value = TrackedOnTimeout)]
    async fn tracked(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/at-cap")]
    #[error_handlers(value = WaitedOnTimeout(MB as usize))]
    async fn at_cap(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/over-cap")]
    #[error_handlers(value = WaitedOnTimeout(MB as usize + 1))]
    async fn over_cap(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/counted")]
    #[error_handlers(value = CountedOnTimeout)]
    async fn counted(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/ready")]
    #[error_handlers(value = ReadyOnTimeout(2 * MB as usize))]
    async fn ready(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }
}

struct Root(Ended);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Slow>();
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
        let server = ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()).timeout_grace(Bound::After(GRACE));
        let app = App::builder(Root(ended.clone()))
            .timer(ulo_tokio::Timer)
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(server)
            .listen()
            .await
            .expect("the app listens");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        let svc = keeper.service();
        Running { svc, ended, handle, serving }
    }

    /// `GET path`, answered by the service: the response with its body not yet read.
    async fn call(&self, path: &str) -> Response {
        let (head, ()) = http::Request::get(path).body(()).expect("the request builds").into_parts();
        let req = Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
        tokio::time::timeout(DEADLINE + GRACE * 2, self.svc.call(req)).await.expect("the service answered within the grace")
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

#[tokio::test]
async fn a_buffered_stream_reports_completed_once_written() {
    let running = Running::start().await;

    let response = running.call("/tracked").await;
    assert_eq!(running.ended.get(), None, "the stream's end was reported before the backend wrote the buffered body");
    let (status, body) = written(response).await;
    assert_eq!((status, body.as_slice()), (StatusCode::SERVICE_UNAVAILABLE, b"claimed".as_slice()));
    assert_eq!(running.ended.get(), Some(StreamOutcome::Completed));
    running.stop().await;
}

#[tokio::test]
async fn a_buffered_stream_dropped_before_its_write_reports_cut_off() {
    let running = Running::start().await;

    let response = running.call("/tracked").await;
    assert_eq!(running.ended.get(), None, "the stream's end was reported before the backend wrote the buffered body");
    drop(response);
    match running.ended.get() {
        Some(StreamOutcome::CutOff(_)) => {}
        other => panic!("a buffered body the peer left before reported {other:?}, not `CutOff`"),
    }
    running.stop().await;
}

#[tokio::test]
async fn a_stream_ended_on_the_first_poll_is_written_whatever_its_size() {
    let running = Running::start().await;

    let response = running.call("/ready").await;
    assert_eq!(response.body().size_hint().exact(), Some(2 * MB), "the buffered body does not carry its exact length");
    let (status, body) = written(response).await;
    assert_eq!((status, body.len() as u64), (StatusCode::SERVICE_UNAVAILABLE, 2 * MB));
    running.stop().await;
}

#[tokio::test]
async fn a_waited_on_body_of_exactly_the_cap_is_written_with_its_length() {
    let running = Running::start().await;

    let response = running.call("/at-cap").await;
    assert_eq!(response.body().size_hint().exact(), Some(MB), "the buffered body does not carry its exact length");
    let (status, body) = written(response).await;
    assert_eq!((status, body.len() as u64), (StatusCode::SERVICE_UNAVAILABLE, MB));
    running.stop().await;
}

#[tokio::test]
async fn a_waited_on_body_one_byte_over_the_cap_answers_504() {
    let running = Running::start().await;

    let (status, _) = written(running.call("/over-cap").await).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    running.stop().await;
}

#[tokio::test]
async fn a_body_dropped_for_the_504_reports_its_stream_cut_off_by_the_deadline() {
    let running = Running::start().await;

    let (status, _) = written(running.call("/over-cap").await).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(
        running.ended.get(),
        Some(StreamOutcome::CutOff(Some(CancelReason::Deadline))),
        "the error handler's stream dropped over the cap"
    );
    running.stop().await;
}

#[tokio::test]
async fn a_pending_body_is_polled_when_it_wakes_the_reader() {
    let running = Running::start().await;

    let (status, body) = written(running.call("/counted").await).await;
    assert_eq!((status, body.as_slice()), (StatusCode::SERVICE_UNAVAILABLE, b"late".as_slice()));
    let polls = POLLS.load(Ordering::Relaxed);
    assert!(polls <= 8, "a body pending for {CHUNK_DELAY:?} was polled {polls} times while the server waited on it");
    running.stop().await;
}
