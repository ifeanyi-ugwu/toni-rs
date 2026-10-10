//! What the scenarios share: a host started with the suite's app, requests to it, the comparison
//! against the reference host, and raw HTTP/1.1 over a socket where the client must control the
//! connection.

use std::any::TypeId;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Method};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use ulo::{AppHandle, Dep};

use crate::app::Probe;
use crate::{Host, HyperHost, Mode, PREFIX, ROUTING_HEADER, app_for};

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

/// Fails the scenario when `error` is the client's own timeout, which shows only that nothing
/// arrived within [`PATIENCE`]; any other failure is the host refusing or closing the request.
pub(crate) fn not_a_timeout(error: &reqwest::Error, what: &str) {
    assert!(!error.is_timeout(), "{what}: the client's timeout of {PATIENCE:?} ended it, which is neither an answer nor a refusal");
}

/// A client sending one request per connection, ending each at [`PATIENCE`].
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder().pool_max_idle_per_host(0).timeout(PATIENCE).build().expect("a client")
}

/// A host serving the suite's app, with the app's handle and probe.
pub(crate) struct Running<H: Host> {
    pub(crate) host: H,
    pub(crate) mode: Mode,
    pub(crate) app: AppHandle,
    pub(crate) probe: Dep<Probe>,
}

/// `H` serving the suite's app as `H::limits()` admits it.
pub(crate) async fn start<H: Host>(mode: Mode) -> Running<H> {
    let app = app_for(H::limits()).await;
    let probe = app.get::<Probe>().await.expect("the suite's app binds its probe");
    let handle = app.handle();
    let host = H::start(app, mode).await;
    Running { host, mode, app: handle, probe }
}

/// Whether `H` is the reference, the hyper backend, which has no host around the app: nothing
/// strips a prefix, nothing writes its request store, and no middleware reads `Routing`.
pub(crate) fn is_reference<H: Host>() -> bool {
    TypeId::of::<H>() == TypeId::of::<HyperHost>()
}

impl<H: Host> Running<H> {
    /// Waits until the host's count of connections read from passes `counted`, read every 10 ms;
    /// `what` names the wait if it runs out after [`PATIENCE`].
    pub(crate) async fn read_past(&self, counted: usize, what: &str) {
        let waited = tokio::time::timeout(PATIENCE, async {
            while self.host.connections_read().is_some_and(|now| now <= counted) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        waited.await.unwrap_or_else(|_| panic!("{what} did not happen within {PATIENCE:?}"));
    }

    /// The URL of `path` in the app: under [`PREFIX`] when nested.
    pub(crate) fn url(&self, path: &str) -> String {
        let prefix = if self.mode == Mode::Nested && !is_reference::<H>() { PREFIX } else { "" };
        format!("{}{prefix}{path}", self.host.base_url())
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

    pub(crate) async fn send(&self, request: Exchange) -> Reply {
        let what = format!("{} {}", request.method, request.path);
        match request.send(&self.url(&request.path)).await {
            Ok(reply) => reply,
            Err(error) => {
                not_a_timeout(&error, &what);
                panic!("{what}: the host did not answer: {error}");
            }
        }
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

    async fn send(&self, url: &str) -> Result<Reply, reqwest::Error> {
        let mut request = client().request(self.method.clone(), url);
        for (name, value) in &self.headers {
            request = request.header(*name, value);
        }
        if let Some(body) = &self.body {
            request = request.body(body.clone());
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await?;
        Ok(Reply { status, headers, body })
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

/// Sends `request` to `host` and to the reference in the same mode, and requires the two replies
/// to match byte for byte in status, the app's headers and body. Answers the host's reply.
pub(crate) async fn same_as_reference<H: Host>(host: &Running<H>, request: Exchange) -> Reply {
    let reply = host.send(request.clone()).await;
    if !is_reference::<H>() {
        let reference = start::<HyperHost>(host.mode).await;
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
pub(crate) struct Raw {
    pub(crate) stream: TcpStream,
}

impl Raw {
    pub(crate) async fn connect(authority: &str) -> Raw {
        let stream = TcpStream::connect(authority).await.expect("the host accepts a connection");
        Raw { stream }
    }

    pub(crate) async fn write(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.expect("the request is written");
        self.stream.flush().await.expect("the request is flushed");
    }

    /// Reads until `needle` has arrived, answering everything read; `None` at the end of the
    /// stream. Fails the scenario, naming `what`, when [`PATIENCE`] passes first.
    pub(crate) async fn read_until(&mut self, needle: &[u8], what: &str) -> Option<Vec<u8>> {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while !seen.windows(needle.len()).any(|window| window == needle) {
            let Ok(read) = tokio::time::timeout_at(deadline, self.stream.read(&mut buffer)).await else {
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
        match tokio::time::timeout(PATIENCE, self.stream.read_to_end(&mut seen)).await {
            Ok(_) => seen,
            Err(_) => panic!("{what}: the host had not closed the connection after {PATIENCE:?}"),
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
