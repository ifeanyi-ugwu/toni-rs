//! The WebSocket conformance suite against `ulo-ws`'s serving surface with no socket and no hyper,
//! on smol: a server builds the `GatewayTable` in `prepare`, as `table.rs`'s does, and serves each
//! connection over an in-memory pipe, reading request heads itself, answering each with the
//! table's handshake decision and handing a switched connection to `Switch::serve`. A failure here
//! is the hub's, since nothing between the suite's client and the table is socket or HTTP library
//! code.
//!
//! The server's HTTP/1.1 is the least the suite needs and what hyper does at the drain: requests
//! one after another on a connection, a body read by its `Content-Length`, and at the drain a
//! connection idle between requests closed, a request already arriving answered first.

use std::future::{Future, poll_fn};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_util::{AsyncReadExt, AsyncWriteExt, StreamExt};
use ulo::app::Connected;
use ulo::{App, BoundAddr, BoxError, DrainToken, Mounted, Runtime, Transport};
use ulo_http::Upgraded;
use ulo_http_conformance::{Harness, OnSmol};
use ulo_smol::Smol;
use ulo_transport::TaskSet;
use ulo_transport::__private::Watch;
use ulo_transport::prepare::Failures;
use ulo_ws::{GatewayDefaults, GatewayTable, Handshake, Port, Ws};
use ulo_ws_conformance::{Host, ws_conformance_suite};

/// Each pipe direction's buffer, in bytes.
const PIPE: usize = 256 * 1024;

/// The longest request head the server reads.
const MAX_HEAD: usize = 64 * 1024;

/// One end of a connection: what it reads from one pipe and writes to the other.
struct Pipe {
    read: piper::Reader,
    write: piper::Writer,
}

/// Both ends of a fresh connection, the client's first.
fn connection() -> (Pipe, Pipe) {
    let (to_server_read, to_server_write) = piper::pipe(PIPE);
    let (to_client_read, to_client_write) = piper::pipe(PIPE);
    (Pipe { read: to_client_read, write: to_server_write }, Pipe { read: to_server_read, write: to_client_write })
}

impl futures_io::AsyncRead for Pipe {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().read).poll_read(cx, buf)
    }
}

impl futures_io::AsyncWrite for Pipe {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().write).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().write).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().write).poll_close(cx)
    }
}

/// Where `connect` finds a bound server: the address it reports, never bound, and the channel its
/// `serve` takes connections from.
static SERVERS: Mutex<Vec<(SocketAddr, UnboundedSender<Pipe>)>> = Mutex::new(Vec::new());

/// The port of the next server's address, unique within the test binary.
static NEXT_PORT: AtomicU16 = AtomicU16::new(1);

/// What the server, its connections and the host value share.
struct Shared {
    addr: SocketAddr,
    /// Connections that have read their first byte.
    read: AtomicUsize,
    table: Mutex<Option<GatewayTable>>,
    runtime: Mutex<Option<Arc<dyn Runtime>>>,
    incoming: Mutex<Option<UnboundedReceiver<Pipe>>>,
    /// Raised by the drain: idle connections close, busy ones close after their answer.
    draining: Watch<bool>,
    /// Raised by the close: `serve` stops taking connections and aborts what is left.
    closing: Watch<bool>,
    /// Connection tasks still running, which the drain waits for.
    live: Watch<usize>,
}

/// A server with no socket and no HTTP library.
struct PipeServer {
    shared: Arc<Shared>,
}

impl PipeServer {
    fn new() -> PipeServer {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), NEXT_PORT.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = unbounded();
        SERVERS.lock().unwrap_or_else(PoisonError::into_inner).push((addr, sender));
        let shared = Shared {
            addr,
            read: AtomicUsize::new(0),
            table: Mutex::new(None),
            runtime: Mutex::new(None),
            incoming: Mutex::new(Some(receiver)),
            draining: Watch::new(false),
            closing: Watch::new(false),
            live: Watch::new(0),
        };
        PipeServer { shared: Arc::new(shared) }
    }

    fn table(&self) -> Option<GatewayTable> {
        self.shared.table.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ulo::Server for PipeServer {
    type Transport = Ws;

    async fn prepare(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        let table = GatewayTable::own_port("PipeServer", &mounted, &GatewayDefaults::default(), &mut failures);
        failures.into_result()?;
        *self.shared.table.lock().unwrap_or_else(PoisonError::into_inner) = Some(table);
        *self.shared.runtime.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(mounted.runtime()));
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        if let Some(table) = self.table() {
            table.start();
        }
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        let Some(mut incoming) = self.shared.incoming.lock().unwrap_or_else(PoisonError::into_inner).take() else { return Ok(()) };
        let (Some(table), Some(runtime)) = (self.table(), self.shared.runtime.lock().unwrap_or_else(PoisonError::into_inner).clone()) else {
            return Err("the pipe server was asked to serve before it was prepared".into());
        };
        let mut connections = TaskSet::new(Arc::clone(&runtime) as Arc<dyn ulo::Spawn>);
        loop {
            let next = {
                let mut closing = pin!(self.shared.closing.wait_for(|closing| *closing));
                let mut accepted = incoming.next();
                poll_fn(|cx| {
                    if closing.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(None);
                    }
                    Pin::new(&mut accepted).poll(cx)
                })
                .await
            };
            let Some(pipe) = next else { break };
            self.shared.live.modify(|live| *live += 1);
            let live = Live(Arc::clone(&self.shared));
            connections.spawn(serve_connection(pipe, table.clone(), Arc::clone(&self.shared), live));
        }
        connections.abort_all();
        connections.join_all().await;
        Ok(())
    }

    async fn drain(&self, token: DrainToken) {
        self.shared.draining.modify(|draining| *draining = true);
        let connections = self.shared.live.wait_for(|live| *live == 0);
        match self.table() {
            Some(table) => {
                futures_util::future::join(table.drain(token), connections).await;
            }
            None => connections.await,
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.shared.closing.modify(|closing| *closing = true);
        SERVERS.lock().unwrap_or_else(PoisonError::into_inner).retain(|(addr, _)| *addr != self.shared.addr);
        if let Some(table) = self.table() {
            table.close().await;
        }
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        vec![BoundAddr::new(<Ws as Transport>::KEY, self.shared.addr)]
    }
}

/// A connection task's place in the count the drain waits on, given back however the task ends.
struct Live(Arc<Shared>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.live.modify(|live| *live -= 1);
    }
}

/// How the wait for a request's first byte ended.
enum First {
    Byte,
    Ended,
    Draining,
}

/// One connection: request heads read and answered until the client leaves, a 101 hands it to
/// the table's driver, or the drain closes it.
async fn serve_connection(mut pipe: Pipe, table: GatewayTable, shared: Arc<Shared>, live: Live) {
    let mut counted = false;
    loop {
        let mut byte = [0u8; 1];
        let woke = {
            let mut draining = pin!(shared.draining.wait_for(|draining| *draining));
            let mut read = pipe.read.read(&mut byte);
            poll_fn(|cx| {
                if let Poll::Ready(read) = Pin::new(&mut read).poll(cx) {
                    return Poll::Ready(if matches!(read, Ok(1)) { First::Byte } else { First::Ended });
                }
                if draining.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(First::Draining);
                }
                Poll::Pending
            })
            .await
        };
        if !matches!(woke, First::Byte) {
            return;
        }
        let first = byte[0];
        if !counted {
            counted = true;
            shared.read.fetch_add(1, Ordering::AcqRel);
        }
        let Some(head) = read_head(&mut pipe, first).await else { return };
        let Some((parts, length)) = parse(&head) else {
            let _ = pipe.write.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            return;
        };
        if length > 0 && skip(&mut pipe, length).await.is_err() {
            return;
        }
        let closes = parts.version == http::Version::HTTP_10
            || parts.headers.get(http::header::CONNECTION).is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"close"));
        match table.handshake(parts, None).await {
            Handshake::Switch(switch) => {
                let response = switch.response();
                if write_head(&mut pipe, response.status(), response.headers(), 0).await.is_err() {
                    return;
                }
                drop(live);
                switch.serve(async move { Ok::<_, BoxError>(Upgraded::from_futures(pipe)) });
                return;
            }
            Handshake::Refuse(refusal) => {
                let response = refusal.into_response();
                let body = response.body().as_bytes();
                if write_head(&mut pipe, response.status(), response.headers(), body.len()).await.is_err()
                    || pipe.write.write_all(body).await.is_err()
                {
                    return;
                }
            }
        }
        if closes || shared.draining.read(|draining| *draining) {
            return;
        }
    }
}

/// The rest of a request head whose first byte is `first`, through its blank line, a byte at a
/// time so nothing past it is read; `None` when the client leaves first or the head is too long.
async fn read_head(pipe: &mut Pipe, first: u8) -> Option<Vec<u8>> {
    let mut head = vec![first];
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= MAX_HEAD {
            return None;
        }
        match pipe.read.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            _ => return None,
        }
    }
    Some(head)
}

/// The head as `http`'s parts, and its `Content-Length`.
fn parse(head: &[u8]) -> Option<(http::request::Parts, usize)> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut request = httparse::Request::new(&mut headers);
    if !matches!(request.parse(head), Ok(httparse::Status::Complete(_))) {
        return None;
    }
    let version = if request.version == Some(0) { http::Version::HTTP_10 } else { http::Version::HTTP_11 };
    let mut builder = http::Request::builder().method(request.method?).uri(request.path?).version(version);
    let mut length = 0;
    for header in request.headers.iter() {
        if header.name.eq_ignore_ascii_case("content-length") {
            length = std::str::from_utf8(header.value).ok()?.trim().parse().ok()?;
        }
        builder = builder.header(header.name, header.value);
    }
    let (parts, ()) = builder.body(()).ok()?.into_parts();
    Some((parts, length))
}

/// Reads and drops a body of `length` bytes.
async fn skip(pipe: &mut Pipe, length: usize) -> io::Result<()> {
    let mut body = vec![0u8; length];
    pipe.read.read_exact(&mut body).await
}

/// A response head with `length` as its `Content-Length` unless the response is a 101.
async fn write_head(pipe: &mut Pipe, status: http::StatusCode, headers: &http::HeaderMap, length: usize) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), status.canonical_reason().unwrap_or(""));
    for (name, value) in headers {
        head.push_str(&format!("{name}: {}\r\n", String::from_utf8_lossy(value.as_bytes())));
    }
    if status != http::StatusCode::SWITCHING_PROTOCOLS && !headers.contains_key(http::header::CONTENT_LENGTH) {
        head.push_str(&format!("content-length: {length}\r\n"));
    }
    head.push_str("\r\n");
    pipe.write.write_all(head.as_bytes()).await?;
    pipe.write.flush().await
}

/// The pipe server, reached through the suite.
struct PipeHost {
    shared: Arc<Shared>,
}

impl Host for PipeHost {
    type Runtime = Smol;
    type Stream = Pipe;

    const PORT: Port = Port::Own;
    // The server closes a connection idle between requests at the drain, as hyper's graceful
    // shutdown does, and answers a request already arriving first.
    const CLOSES_IDLE_AT_DRAIN: bool = true;

    fn upgrades() -> bool {
        true
    }

    fn runtime() -> Smol {
        OnSmol::runtime()
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        OnSmol::block_on(fut)
    }

    fn bind(app: App<Connected>) -> (App<Connected>, Self) {
        let server = PipeServer::new();
        let shared = Arc::clone(&server.shared);
        (app.bind(server), PipeHost { shared })
    }

    fn connections_read(&self) -> Option<usize> {
        Some(self.shared.read.load(Ordering::Acquire))
    }

    async fn connect(addresses: &[BoundAddr]) -> io::Result<Pipe> {
        let addr = addresses.first().map(|bound| bound.addr);
        let sender = SERVERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(bound, _)| Some(*bound) == addr)
            .map(|(_, sender)| sender.clone());
        let Some(sender) = sender else {
            return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("no pipe server at {addr:?}")));
        };
        let (client, server) = connection();
        sender.unbounded_send(server).map_err(|_| io::Error::new(io::ErrorKind::ConnectionRefused, "the pipe server has closed"))?;
        Ok(client)
    }
}

ws_conformance_suite!(PipeHost);
