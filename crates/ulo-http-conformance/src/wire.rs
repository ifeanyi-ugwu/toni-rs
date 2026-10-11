//! What the scenarios share: a host started with the suite's app, requests to it, the comparison
//! against the reference host, and raw HTTP/1.1 over a connection where the client must control
//! it. Every wait is bounded by the app's `Timer` and every task runs on the app's `Runtime`.

use std::any::TypeId;
use std::fmt;
use std::future::{Future, poll_fn};
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{AsyncReadExt, AsyncWriteExt};
use http::header::{ACCEPT, HOST};
use http::{HeaderMap, Method};
use http_body_util::{BodyExt, Full};
use ulo::{AppHandle, Dep, Runtime, Timer};
use ulo_hyper_serve::FuturesIo;

use crate::app::Probe;
use crate::{Harness, Host, HyperHost, Mode, PREFIX, ROUTING_HEADER, app_for};

/// The headers the app writes, which the byte-identical comparison covers. A host adds its own
/// (`date`, `server`, rocket's security headers) and frames the body its own way, so framing and
/// host headers are left out.
const COMPARED: &[&str] = &[
    "content-type",
    "allow",
    "cache-control",
    "retry-after",
    "www-authenticate",
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "vary",
];

/// The longest any one exchange may take before the scenario fails. A wait this long ends the
/// scenario as a failure wherever it ends: a client timeout is neither an answer nor a refusal.
pub(crate) const PATIENCE: Duration = Duration::from_secs(10);

/// `fut`'s output, or `None` once `after` has passed on `timer`'s clock first.
pub(crate) async fn within<F: Future>(timer: &dyn Timer, after: Duration, fut: F) -> Option<F::Output> {
    let mut fut = pin!(fut);
    let mut expired = timer.sleep(after);
    poll_fn(|cx| {
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        if expired.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// Why a request the client sent got no response.
#[derive(Debug)]
pub(crate) enum Failure {
    /// The client's own timeout of [`PATIENCE`] ended it.
    TimedOut,
    /// The host refused or closed the connection, or answered something that is not HTTP.
    Failed(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::TimedOut => write!(f, "no response within {PATIENCE:?}"),
            Failure::Failed(reason) => f.write_str(reason),
        }
    }
}

fn failed(what: &str, error: impl fmt::Display) -> Failure {
    Failure::Failed(format!("{what}: {error}"))
}

/// Fails the scenario when `error` is the client's own timeout, which shows only that nothing
/// arrived within [`PATIENCE`]; any other failure is the host refusing or closing the request.
pub(crate) fn not_a_timeout(error: &Failure, what: &str) {
    assert!(
        !matches!(error, Failure::TimedOut),
        "{what}: the client's timeout of {PATIENCE:?} ended it, which is neither an answer nor a refusal"
    );
}

/// A host serving the suite's app, with the app's handle, its runtime and probe.
pub(crate) struct Running<H: Host> {
    pub(crate) host: H,
    pub(crate) mode: Mode,
    pub(crate) app: AppHandle,
    pub(crate) runtime: Arc<dyn Runtime>,
    pub(crate) probe: Dep<Probe>,
}

/// `H` serving the suite's app as `H::limits()` admits it, on the app's runtime.
pub(crate) async fn start<H: Host>(mode: Mode) -> Running<H> {
    let app = app_for(H::limits(), <H::Harness as Harness>::runtime()).await;
    let probe = app.get::<Probe>().await.expect("the suite's app binds its probe");
    let handle = app.handle();
    let runtime = Arc::clone(handle.runtime().expect("the suite's app is given a runtime"));
    let host = H::start(app, mode).await;
    Running { host, mode, app: handle, runtime, probe }
}

/// Whether `H` is the reference, the hyper backend, which has no host around the app: nothing
/// strips a prefix, nothing writes its request store, and no middleware reads `Routing`.
pub(crate) fn is_reference<H: Host>() -> bool {
    TypeId::of::<H>() == TypeId::of::<HyperHost<H::Harness>>()
}

impl<H: Host> Running<H> {
    /// The app's clock, which bounds every wait.
    pub(crate) fn timer(&self) -> &dyn Timer {
        &*self.runtime
    }

    /// Waits until the host's count of connections read from passes `counted`, read every 10 ms;
    /// `what` names the wait if it runs out after [`PATIENCE`].
    pub(crate) async fn read_past(&self, counted: usize, what: &str) {
        let waited = within(self.timer(), PATIENCE, async {
            while self.host.connections_read().is_some_and(|now| now <= counted) {
                self.timer().sleep(Duration::from_millis(10)).await;
            }
        });
        waited.await.unwrap_or_else(|| panic!("{what} did not happen within {PATIENCE:?}"));
    }

    /// The URL of `path` in the app: under [`PREFIX`] when nested.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{}", self.host.base_url(), self.target(path))
    }

    /// The path the client sends for `path` in the app.
    pub(crate) fn target(&self, path: &str) -> String {
        let prefix = if self.mode == Mode::Nested && !is_reference::<H>() { PREFIX } else { "" };
        format!("{prefix}{path}")
    }

    /// The pattern `Routing` names for an app route, the mount prefix applied.
    pub(crate) fn route(&self, pattern: &str) -> String {
        self.target(pattern)
    }

    /// `host:port` of the host's listener.
    pub(crate) fn authority(&self) -> String {
        let base = self.host.base_url();
        base.trim_start_matches("http://").trim_end_matches('/').to_owned()
    }

    /// The host's listener as a socket address, which the harness connects to.
    pub(crate) fn addr(&self) -> SocketAddr {
        let authority = self.authority();
        authority.parse().unwrap_or_else(|_| panic!("the host's base URL names `{authority}`, not a socket address"))
    }

    /// A connection to the host, opened by the harness.
    pub(crate) async fn connection(&self) -> <H::Harness as Harness>::Stream {
        within(self.timer(), PATIENCE, <H::Harness as Harness>::connect(self.addr()))
            .await
            .unwrap_or_else(|| panic!("connecting to the host did not finish within {PATIENCE:?}"))
            .unwrap_or_else(|error| panic!("the host does not accept a connection: {error}"))
    }

    /// A raw HTTP/1.1 connection to the host.
    pub(crate) async fn raw(&self) -> Raw<<H::Harness as Harness>::Stream> {
        Raw { stream: self.connection().await, runtime: Arc::clone(&self.runtime) }
    }

    pub(crate) async fn send(&self, request: Exchange) -> Reply {
        let what = format!("{} {}", request.method, request.path);
        match self.request(&request).await {
            Ok(reply) => reply,
            Err(error) => {
                not_a_timeout(&error, &what);
                panic!("{what}: the host did not answer: {error}");
            }
        }
    }

    /// `request` on a connection of its own, as HTTP/1.1, the response read to its end, all of it
    /// within [`PATIENCE`].
    pub(crate) async fn request(&self, request: &Exchange) -> Result<Reply, Failure> {
        let exchange = async {
            let stream = <H::Harness as Harness>::connect(self.addr()).await.map_err(|error| failed("connecting", error))?;
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(FuturesIo::new(stream)).await.map_err(|error| failed("the HTTP/1.1 handshake", error))?;
            // Dropped, the handle detaches: the connection ends once the sender and the response
            // body are gone.
            drop(self.runtime.spawn(Box::pin(async move {
                let _ = connection.await;
            })));
            let mut builder = http::Request::builder().method(request.method.clone()).uri(self.target(&request.path));
            builder = builder.header(HOST, self.authority());
            if !request.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(ACCEPT.as_str())) {
                builder = builder.header(ACCEPT, "*/*");
            }
            for (name, value) in &request.headers {
                builder = builder.header(*name, value);
            }
            let body = Full::new(request.body.clone().unwrap_or_default());
            let outgoing = builder.body(body).map_err(|error| failed("building the request", error))?;
            let response = sender.send_request(outgoing).await.map_err(|error| failed("the request", error))?;
            let (parts, body) = response.into_parts();
            let body = body.collect().await.map_err(|error| failed("the response body", error))?.to_bytes();
            Ok(Reply { status: parts.status.as_u16(), headers: parts.headers, body })
        };
        within(self.timer(), PATIENCE, exchange).await.unwrap_or(Err(Failure::TimedOut))
    }

    pub(crate) async fn stop(self) {
        self.host.stop().await;
    }
}

/// One request a scenario sends.
#[derive(Clone, Debug)]
pub(crate) struct Exchange {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Option<Bytes>,
}

impl Exchange {
    pub(crate) fn new(method: Method, path: &str) -> Self {
        Exchange { method, path: path.to_owned(), headers: Vec::new(), body: None }
    }

    pub(crate) fn get(path: &str) -> Self {
        Exchange::new(Method::GET, path)
    }

    pub(crate) fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub(crate) fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = Some(body.into());
        self
    }
}

/// A response as the client read it.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

impl Reply {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The `Routing` label the host's test middleware wrote.
    pub(crate) fn routing(&self) -> Option<&str> {
        self.header(ROUTING_HEADER)
    }

    /// Status, the compared headers and the body, as one value two replies are compared by.
    fn shape(&self) -> (u16, Vec<(&'static str, Option<String>)>, Bytes) {
        let headers = COMPARED.iter().map(|name| (*name, self.header(name).map(str::to_owned))).collect();
        (self.status, headers, self.body.clone())
    }
}

/// Sends `request` to `host` and to the reference in the same mode on the same runtime, and
/// requires the two replies to match byte for byte in status, the app's headers and body. Answers
/// the host's reply.
pub(crate) async fn same_as_reference<H: Host>(host: &Running<H>, request: Exchange) -> Reply {
    let reply = host.send(request.clone()).await;
    if !is_reference::<H>() {
        let reference = start::<HyperHost<H::Harness>>(host.mode).await;
        let expected = reference.send(request.clone()).await;
        reference.stop().await;
        assert_eq!(
            reply.shape(),
            expected.shape(),
            "{} {} differs from the reference host",
            request.method,
            request.path
        );
    }
    reply
}

/// A raw HTTP/1.1 connection to the host, for scenarios that control the connection itself.
pub(crate) struct Raw<S> {
    pub(crate) stream: S,
    runtime: Arc<dyn Runtime>,
}

impl<S: futures_io::AsyncRead + futures_io::AsyncWrite + Unpin> Raw<S> {
    pub(crate) async fn write(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("the request is written");
        self.stream.flush().await.expect("the request is flushed");
    }

    /// Reads until `needle` has arrived, answering everything read; `None` at the end of the
    /// stream. Fails the scenario, naming `what`, when [`PATIENCE`] passes first.
    pub(crate) async fn read_until(&mut self, needle: &[u8], what: &str) -> Option<Vec<u8>> {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 1024];
        let deadline = self.runtime.now() + PATIENCE;
        while !seen.windows(needle.len()).any(|window| window == needle) {
            let left = deadline.saturating_duration_since(self.runtime.now());
            let Some(read) = within(&*self.runtime, left, self.stream.read(&mut buffer)).await else {
                panic!("{what}: nothing more arrived within {PATIENCE:?}, which is neither an answer nor a close");
            };
            match read {
                Ok(0) | Err(_) => return None,
                Ok(read) => seen.extend_from_slice(&buffer[..read]),
            }
        }
        Some(seen)
    }

    /// Reads until the host closes the connection, answering everything read. Fails the
    /// scenario, naming `what`, when the host has not closed it after [`PATIENCE`].
    pub(crate) async fn read_to_close(&mut self, what: &str) -> Vec<u8> {
        let mut seen = Vec::new();
        match within(&*self.runtime, PATIENCE, self.stream.read_to_end(&mut seen)).await {
            Some(_) => seen,
            None => panic!("{what}: the host had not closed the connection after {PATIENCE:?}"),
        }
    }
}

/// The status code of a raw response's first line.
pub(crate) fn status_of(raw: &[u8]) -> Option<u16> {
    let text = String::from_utf8_lossy(raw);
    text.lines().next()?.split_whitespace().nth(1)?.parse().ok()
}

/// Whether a raw response's head carries `name: value`, both compared without case.
pub(crate) fn has_header(raw: &[u8], name: &str, value: &str) -> bool {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    head.lines().skip(1).any(|line| {
        line.split_once(':').is_some_and(|(found, values)| {
            found.trim().eq_ignore_ascii_case(name)
                && values.split(',').any(|token| token.trim().eq_ignore_ascii_case(value))
        })
    })
}
