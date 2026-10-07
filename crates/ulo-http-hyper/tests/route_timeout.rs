//! What a client receives when an error handler answers a request whose route timeout passed: a
//! response whose body ends within the grace as the error handler wrote it, a `Body::stream` of
//! one payload included and written with its exact length, and an `Sse`, still open when the grace
//! runs out, replaced by the canonical 504, none of its events written, with a `warn` line telling
//! the author why; so is a body whose frames are always ready and never end. A body the server
//! waits on that yields more than the 1 MiB it buffers is replaced the same way as soon as it
//! passes the cap; a body already produced, a `Full` of 2 MiB, is written whole.
//!
//! The tests run on a current-thread runtime, so the server's tasks emit their events on the test's
//! thread, where [`Capture`] is the default subscriber. The client is raw HTTP/1.1 over a socket,
//! reading until the server closes the connection.

use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::{App, AppHandle, BoxError, Bound, ErrorHandler, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::{Bytes, Event, Http, HttpBody, HttpCx, Response, Sse, StatusCode, Timeout};
use ulo_transport::{CallError, ErrorKind, IntoReply};

const DEADLINE: Duration = Duration::from_millis(200);
const GRACE: Duration = Duration::from_millis(500);

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(5);

const WARNING: &str = "an error handler answered a timed-out call with a stream; the stream was ended at the deadline";

const OVERSIZED: &str =
    "an error handler answered a timed-out call with a body larger than the buffer for a reply after the deadline; it was dropped at the deadline";

/// What [`LargeOnTimeout`] streams: past the 1 MiB the server buffers after a deadline.
const LARGE: usize = 1024 * 1024 + 64 * 1024;

/// What [`FullOnTimeout`] answers: twice the 1 MiB the server buffers of a body it waits on.
const FULL: usize = 2 * 1024 * 1024;

fn timed_out(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().is_some_and(|call| call.kind() == ErrorKind::Timeout)
}

/// Answers a `Timeout` with an event stream that never ends, one event every 20 ms: far below
/// the 1 MiB the server buffers by the time the grace runs out.
struct SseOnTimeout;

impl ErrorHandler<Http> for SseOnTimeout {
    async fn handle(&self, err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let ticks = stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Event::default().data("tick"), ()))
        });
        Ok(Sse::new(ticks).into_reply(cx)?)
    }
}

/// Answers a `Timeout` with a body that never ends and whose frames, all empty, are always ready:
/// it never returns `Pending` and never reaches the buffer's cap, so only the grace ends it, and
/// only if reading it yields to the runtime, whose timer this current-thread test shares.
struct SpinningOnTimeout;

impl ErrorHandler<Http> for SpinningOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(Response::new(HttpBody::stream(stream::repeat_with(|| Ok::<_, Infallible>(Bytes::new())))))
    }
}

/// Answers a `Timeout` with 503 and the body `claimed`.
struct OneOnTimeout;

impl ErrorHandler<Http> for OneOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let mut response = Response::new(HttpBody::from("claimed"));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

/// How long [`StreamedOnTimeout`]'s one payload takes to arrive, well inside the grace.
const PAYLOAD_DELAY: Duration = Duration::from_millis(50);

/// Answers a `Timeout` with 503 and the body `claimed`, sent through `Body::stream` as one payload
/// that arrives after [`PAYLOAD_DELAY`].
struct StreamedOnTimeout;

impl ErrorHandler<Http> for StreamedOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let payload = stream::once(async {
            tokio::time::sleep(PAYLOAD_DELAY).await;
            Ok::<_, Infallible>(Bytes::from_static(b"claimed"))
        });
        let mut response = Response::new(HttpBody::stream(payload));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

/// Answers a `Timeout` with a body of [`LARGE`] bytes in 64 KiB chunks, pending once before the
/// first, so the server waits on it, and every one ready after that.
struct LargeOnTimeout;

impl ErrorHandler<Http> for LargeOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let pending_once = stream::once(tokio::task::yield_now()).filter_map(|()| async { None });
        let chunks = stream::iter((0..LARGE / (64 * 1024)).map(|_| Ok::<_, Infallible>(Bytes::from(vec![b'x'; 64 * 1024]))));
        let mut response = Response::new(HttpBody::stream(pending_once.chain(chunks)));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

/// Answers a `Timeout` with 503 and a `Full` body of [`FULL`] bytes.
struct FullOnTimeout;

impl ErrorHandler<Http> for FullOnTimeout {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        let mut response = Response::new(HttpBody::from_bytes(vec![b'x'; FULL]));
        *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
        Ok(response)
    }
}

#[injectable]
struct Slow;

#[routes]
#[meta(Timeout::after(DEADLINE))]
impl Slow {
    #[ulo_http::get("/streamed")]
    #[error_handlers(value = SseOnTimeout)]
    async fn streamed(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/answered")]
    #[error_handlers(value = OneOnTimeout)]
    async fn answered(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/payload")]
    #[error_handlers(value = StreamedOnTimeout)]
    async fn payload(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/spinning")]
    #[error_handlers(value = SpinningOnTimeout)]
    async fn spinning(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/large")]
    #[error_handlers(value = LargeOnTimeout)]
    async fn large(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }

    #[ulo_http::get("/full")]
    #[error_handlers(value = FullOnTimeout)]
    async fn full(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Slow>();
    }
}

struct Running {
    addr: std::net::SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start() -> Running {
        let server = ulo_http_hyper::Server::new("127.0.0.1:0").timeout_grace(Bound::After(GRACE));
        let app = App::builder(Root)
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
        let addr = app.addresses().first().map(|bound| bound.addr).expect("the app is bound");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { addr, handle, serving }
    }

    /// `GET path` on a connection of its own, read until the server closes it: the status line
    /// and the body after the headers.
    async fn get(&self, path: &str) -> (String, String) {
        let (status, _head, body) = self.exchange(path).await;
        (status, body)
    }

    /// `GET path` on a connection of its own, read until the server closes it: the status line,
    /// the head, and the body after the headers.
    async fn exchange(&self, path: &str) -> (String, String, String) {
        let exchange = async {
            let mut socket = TcpStream::connect(self.addr).await.expect("the client connects");
            let request = format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
            socket.write_all(request.as_bytes()).await.expect("the request is written");
            let mut response = Vec::new();
            socket.read_to_end(&mut response).await.expect("the response is read");
            response
        };
        let response = match tokio::time::timeout(PATIENCE, exchange).await {
            Ok(response) => String::from_utf8_lossy(&response).into_owned(),
            Err(_) => panic!("the response to `GET {path}` did not end within {PATIENCE:?}"),
        };
        let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = head.lines().next().unwrap_or_default().to_owned();
        (status, head.to_owned(), body.to_owned())
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

#[tokio::test]
async fn an_event_stream_answered_after_the_route_timeout_is_replaced_by_504_when_the_grace_ends() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let began = Instant::now();
    let (status, body) = running.get("/streamed").await;
    let took = began.elapsed();
    assert_eq!(status, "HTTP/1.1 504 Gateway Timeout", "body: {body}");
    assert!(body.contains("the request did not complete within its time limit"), "body: {body}");
    assert!(!body.contains("tick"), "an event of the stream was written: {body}");
    assert!(took >= DEADLINE + GRACE, "answered after {took:?}, before the {DEADLINE:?} route timeout and the {GRACE:?} grace had passed");
    let warned = capture.warnings();
    assert!(
        warned.iter().any(|line| line.contains(WARNING) && line.contains("route=/streamed")),
        "the server logged no warning naming the route: {warned:?}"
    );
    running.stop().await;
}

#[tokio::test]
async fn an_always_ready_body_answered_after_the_route_timeout_is_replaced_by_504_when_the_grace_ends() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let (status, body) = running.get("/spinning").await;
    assert_eq!(status, "HTTP/1.1 504 Gateway Timeout", "body: {body}");
    let warned = capture.warnings();
    assert!(
        warned.iter().any(|line| line.contains(WARNING) && line.contains("route=/spinning")),
        "the server logged no warning naming the route: {warned:?}"
    );
    running.stop().await;
}

#[tokio::test]
async fn a_response_of_known_length_answered_after_the_route_timeout_is_the_response() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let (status, body) = running.get("/answered").await;
    assert_eq!((status.as_str(), body.as_str()), ("HTTP/1.1 503 Service Unavailable", "claimed"));
    let warned = capture.warnings();
    assert!(!warned.iter().any(|line| line.contains(WARNING)), "a response of known length was logged as a stream: {warned:?}");
    running.stop().await;
}

#[tokio::test]
async fn a_single_payload_streamed_after_the_route_timeout_is_written_with_its_length() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let (status, head, body) = running.exchange("/payload").await;
    assert_eq!((status.as_str(), body.as_str()), ("HTTP/1.1 503 Service Unavailable", "claimed"), "head: {head}");
    let head = head.to_ascii_lowercase();
    assert!(
        head.contains("content-length: 7") && !head.contains("transfer-encoding"),
        "the payload was not written with its exact length: {head}"
    );
    let warned = capture.warnings();
    assert!(!warned.iter().any(|line| line.contains(WARNING)), "a single payload was logged as a stream: {warned:?}");
    running.stop().await;
}

#[tokio::test]
async fn a_full_body_over_the_buffer_answered_after_the_route_timeout_is_written_whole() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let (status, head, body) = running.exchange("/full").await;
    assert_eq!((status.as_str(), body.len()), ("HTTP/1.1 503 Service Unavailable", FULL), "head: {head}");
    assert!(body.bytes().all(|byte| byte == b'x'), "the body is not the one the error handler answered");
    assert!(head.to_ascii_lowercase().contains(&format!("content-length: {FULL}")), "the body was not written with its exact length: {head}");
    let warned = capture.warnings();
    assert!(!warned.iter().any(|line| line.contains(OVERSIZED)), "a body already produced was refused as oversized: {warned:?}");
    running.stop().await;
}

#[tokio::test]
async fn a_waited_on_body_over_the_buffer_answered_after_the_route_timeout_is_replaced_by_504_before_the_grace_ends() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let running = Running::start().await;

    let began = Instant::now();
    let (status, body) = running.get("/large").await;
    let took = began.elapsed();
    assert_eq!(status, "HTTP/1.1 504 Gateway Timeout", "body of {} bytes", body.len());
    assert!(body.contains("the request did not complete within its time limit"), "body of {} bytes", body.len());
    assert!(took < DEADLINE + GRACE, "answered after {took:?}, once the {GRACE:?} grace had run out rather than at the buffer's cap");
    let warned = capture.warnings();
    assert!(
        warned.iter().any(|line| line.contains(OVERSIZED) && line.contains("route=/large")),
        "the server logged no warning naming the route: {warned:?}"
    );
    running.stop().await;
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
