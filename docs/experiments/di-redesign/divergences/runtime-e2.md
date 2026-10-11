# Divergences: runtime neutrality, stage e2: the listener plug point in `ulo-hyper-serve`, `ulo-listen-tokio` and `ulo-listen-smol`, the hyper backend and the standalone WebSocket server on either, the HTTP suite's runtime half, and smol hosts of both suites

The thirty-eighth response, signed off 2026-10-10, has `ulo-hyper-serve` become runtime-neutral
rather than a second server being written beside it. This stage builds that. `ulo-hyper-serve`
takes a `Listener`, a trait whose `accept()` answers an `Incoming`: the peer and local addresses
and the handshake that yields a stream on hyper's I/O traits. The accept loop spawns through the
app's `Runtime`, times the handshake and its back-off through its `Timer`, and signals the drain
through `ulo-transport`'s runtime-free `Watch`; `RuntimeExecutor` and `RuntimeTimer` are hyper's
executor and timer over the same two, and `FuturesIo` carries a `futures-io` stream into hyper's
I/O traits and hyper's upgraded I/O out to `futures-io`'s. `ulo-listen-tokio` is the old adoption
extracted, tokio sockets and `tokio-rustls` under hyper-util's `TokioIo`; `ulo-listen-smol` is
`async-net` sockets and `futures-rustls` under `FuturesIo`. The hyper backend is `HyperOn<L>` and
the standalone WebSocket server `ServerOn<L>`, with `Hyper`, `Server` and `ulo_ws_hyper::Server`
the tokio aliases behind a default `tokio` feature; gRPC stays on the tokio listener. The HTTP
suite's `Host` names a `Harness`, its runtime half, and the suite's own code starts no tokio
runtime: its client is hyper's and `h2`'s client connections over the harness's `futures-io`
stream. On smol, the hyper backend passes the HTTP suite's 36 scenarios with the reference's one
declaration, and the standalone server, the HTTP port on `ulo-http-hyper` and an in-memory pipe
host with no hyper and no socket each pass the WebSocket suite's 31, three runs each. The tree
goal for `ulo-hyper-serve` and the smol listener is not met: hyper depends on tokio's `sync`
feature unconditionally, so both print tokio through hyper and through nothing else (F383, S1).

Files changed: the workspace `Cargo.toml` (the members `crates/ulo-listen-tokio` and
`crates/ulo-listen-smol`; `async-net`, `futures-rustls`, `httparse` and `piper` in
`[workspace.dependencies]`) and `Cargo.lock`; the new crates `crates/ulo-listen-tokio` and
`crates/ulo-listen-smol` (`Cargo.toml`, `src/lib.rs`, `tests/listener.rs`);
`crates/ulo-hyper-serve/{Cargo.toml, src/lib.rs, src/listener.rs, src/serve.rs, src/rt.rs (new),
tests/serve.rs (new), tests/rt.rs (new), tests/support/mod.rs (new)}`, `src/handshake.rs` removed;
`crates/ulo-http/src/{server.rs, service.rs, request.rs}`; `crates/ulo-http-hyper/{Cargo.toml,
src/lib.rs, src/backend.rs, src/convert.rs, tests/conformance.rs, tests/tls.rs,
tests/conformance_smol.rs (new), tests/tls_smol.rs (new), tests/listeners.rs (new)}`;
`crates/ulo-ws-hyper/{Cargo.toml, src/lib.rs, tests/runtime.rs, tests/conformance_smol.rs (new)}`;
`crates/ulo-ws/{Cargo.toml, tests/runtime.rs, tests/conformance_smol.rs (new),
tests/conformance_pipe.rs (new)}`; `crates/ulo-grpc/{Cargo.toml, src/server.rs}`;
`crates/ulo-http-conformance/{Cargo.toml, src/lib.rs, src/app.rs, src/count.rs, src/reference.rs,
src/wire.rs, src/cases/{disconnect.rs, drain.rs, lifecycle.rs, upgrade.rs}, src/tokio_harness.rs
(new), src/smol_harness.rs (new)}`; `crates/ulo-http-{axum,salvo,poem,rocket,actix}/Cargo.toml`
with each host's `tests/conformance.rs` (actix's `tests/common/mod.rs`);
`crates/ulo-graphql-ws/tests/runtime.rs`; in the workspace's `FRAMEWORK_GAPS.md`, F383 and F384
filed. The tree is `828e0f0d` plus this stage; `828e0f0d`, committed by another agent during the
stage, is `a1baa51c` with the two DESIGN files folded and nothing else.

## The signatures

```rust
// ulo_hyper_serve: the plug point, new
pub trait Listener: Sized + Send + Sync + 'static {
    type Io: hyper::rt::Read + hyper::rt::Write + Send + Unpin + 'static;
    fn adopt(listener: BoundListener, tls: Option<Arc<ServerConfig>>) -> io::Result<Self>;
    fn accept(&self) -> impl Future<Output = io::Result<Incoming<Self::Io>>> + Send;
}
pub struct Incoming<T> { /* private: peer, local, the handshake as a boxed future */ }
impl<T: Send + 'static> Incoming<T> {
    pub fn plain(io: T, peer: SocketAddr, local: Option<SocketAddr>) -> Incoming<T>;
    pub fn handshaking<F>(peer: SocketAddr, local: Option<SocketAddr>, handshake: F) -> Incoming<T>
    where
        F: Future<Output = io::Result<(T, TlsInfo)>> + Send + 'static;
    pub fn peer(&self) -> SocketAddr;
}
pub fn tls_info(session: &rustls::ServerConnection) -> TlsInfo;

// ulo_hyper_serve: hyper's runtime edges, new
#[derive(Clone)] pub struct RuntimeExecutor { /* Arc<dyn Runtime> */ }
impl RuntimeExecutor { pub fn new(runtime: Arc<dyn Runtime>) -> RuntimeExecutor; }
impl<F: Future + Send + 'static> hyper::rt::Executor<F> for RuntimeExecutor {}
#[derive(Clone)] pub struct RuntimeTimer { /* Arc<dyn ulo::Timer> */ }
impl RuntimeTimer { pub fn new(timer: Arc<dyn ulo::Timer>) -> RuntimeTimer; }
impl hyper::rt::Timer for RuntimeTimer {}               // sleep, sleep_until, now
#[derive(Debug)] pub struct FuturesIo<T> { /* private */ }
impl<T> FuturesIo<T> { pub fn new(inner: T) -> Self; pub fn get_ref(&self) -> &T; pub fn into_inner(self) -> T; }
impl<T: futures_io::AsyncRead + Unpin> hyper::rt::Read for FuturesIo<T> {}
impl<T: futures_io::AsyncWrite + Unpin> hyper::rt::Write for FuturesIo<T> {}
impl<T: hyper::rt::Read + Unpin> futures_io::AsyncRead for FuturesIo<T> {}
impl<T: hyper::rt::Write + Unpin> futures_io::AsyncWrite for FuturesIo<T> {}

// ulo_hyper_serve: changed
pub struct Serve<L: Listener>;                          // was `Serve`
impl<L: Listener> Serve<L> {
    pub fn new(listeners: Vec<BoundListener>, tls: Option<Arc<ServerConfig>>, config: &ServeConfig,
               runtime: Arc<dyn Runtime>) -> io::Result<Serve<L>>;   // `runtime` added
    pub async fn run<F, Fut>(&self, connection: F) -> Result<(), BoxError>
    where F: Fn(Accepted<L>) -> Fut + Send + Sync + 'static, Fut: Future<Output = ()> + Send + 'static;
    pub async fn drain(&self);                          // unchanged
    pub async fn close(&self);                          // unchanged
}
pub struct Accepted<L: Listener> { pub io: Io<L::Io>, pub conn: ConnInfo, pub draining: Draining }  // was `Accepted`
pub struct Io<T>;                                       // was `Io`, a tokio stream plain or TLS; now
impl<T> Io<T> { pub fn get_ref(&self) -> &T; }          // any listener's, on hyper's I/O traits
impl<T: hyper::rt::Read + Unpin> hyper::rt::Read for Io<T> {}
impl<T: hyper::rt::Write + Unpin> hyper::rt::Write for Io<T> {}
// removed: Io::is_tls (the listeners' streams answer it); tokio's AsyncRead and AsyncWrite on Io
impl Draining { pub async fn wait(&self); pub fn is_draining(&self) -> bool; }  // `wait` took `&mut self`
// unchanged: ServeConfig, ReadCount

// ulo_listen_tokio, new crate
pub struct TokioListener { /* tokio::net::TcpListener, Option<tokio_rustls::TlsAcceptor> */ }
impl Listener for TokioListener { type Io = hyper_util::rt::TokioIo<TokioStream>; }
pub struct TokioStream { /* plain or TLS */ }
impl TokioStream { pub fn is_tls(&self) -> bool; }     // and tokio's AsyncRead, AsyncWrite

// ulo_listen_smol, new crate
pub struct SmolListener { /* async_net::TcpListener, Option<futures_rustls::TlsAcceptor> */ }
impl Listener for SmolListener { type Io = FuturesIo<SmolStream>; }
pub struct SmolStream { /* plain or TLS */ }
impl SmolStream { pub fn is_tls(&self) -> bool; }      // and futures-io's AsyncRead, AsyncWrite

// ulo_http, added
impl AppService { pub fn runtime(&self) -> &Arc<dyn Runtime>; }

// ulo_http_hyper
pub struct HyperOn<L: Listener>;                        // was `pub struct Hyper`; Default, read_count()
impl<L: Listener> Backend for HyperOn<L> {}
#[cfg(feature = "tokio")] pub type Hyper = HyperOn<ulo_listen_tokio::TokioListener>;
#[cfg(feature = "tokio")] pub type Server = ulo_http::Server<Hyper>;   // unchanged meaning
pub type ServerOn<L> = ulo_http::Server<HyperOn<L>>;    // new
// features: `tokio` (default) = ulo-listen-tokio. No longer enables ulo-http's `tokio-io`, nor
// hyper-util's `tokio`.

// ulo_ws_hyper
pub struct ServerOn<L: Listener>;                       // was `pub struct Server`; same builder
#[cfg(feature = "tokio")] pub type Server = ServerOn<ulo_listen_tokio::TokioListener>;
// features: `tokio` (default) = ulo-listen-tokio. No longer enables ulo-http's `tokio-io`, nor
// hyper-util.

// ulo_grpc: no public change; `Server` accepts through `Serve<TokioListener>` and depends on
// ulo-listen-tokio.

// ulo_http_conformance
pub trait Harness: Send + Sync + 'static {              // new
    type Runtime: Runtime;
    type Listener: ulo_hyper_serve::Listener;
    type Stream: futures_io::AsyncRead + futures_io::AsyncWrite + Unpin + Send + 'static;
    fn runtime() -> Self::Runtime;
    fn block_on<F: Future>(fut: F) -> F::Output;
    fn connect(addr: SocketAddr) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}
pub trait Host { type Harness: Harness; /* the rest unchanged */ }
pub struct HyperHost<R: Harness>;                       // was `HyperHost`
pub async fn app(runtime: impl Runtime) -> App<Connected>;                     // was `app()`
pub async fn app_for(limits: EmbedLimits, runtime: impl Runtime) -> App<Connected>;  // `runtime` added
#[cfg(feature = "tokio")] pub struct OnTokio;           // new: Harness over tokio
#[cfg(feature = "smol")] pub struct OnSmol;             // new: Harness over smol
#[cfg(feature = "tokio")] pub struct Counted<S>;        // was unconditional
impl ReadCount { #[cfg(feature = "tokio")] pub fn wrap<S>(&self, stream: S) -> Counted<S>; }
// http_conformance_suite! stamps `#[test]`, each scenario through `Harness::block_on`; it stamped
// `#[tokio::test(flavor = "multi_thread")]`. `__private::Slots::hold` blocks the test thread (it
// awaited a tokio `Semaphore`).
```

Unchanged: `ulo_ws_conformance`'s `Host`, which already carried `runtime()`, `block_on` and a
`futures-io` stream; `ulo_http::Backend`; `ulo_http::Server`; `ReadCount`; `ServeConfig`.

Dependencies: `ulo-hyper-serve` drops tokio and `tokio-rustls` and gains `hyper` (no features),
`futures-io` and `ulo-transport`; `ulo-listen-tokio` depends on `ulo-hyper-serve`, `ulo-http`,
`ulo-net`, `hyper-util` with `tokio`, tokio with `net` and `rt`, and `tokio-rustls`;
`ulo-listen-smol` on `ulo-hyper-serve`, `ulo-http`, `ulo-net`, `async-net` 2.0, `futures-io` and
`futures-rustls` 0.26 with `ring`, `tls12` and `logging`. `ulo-http-hyper` and `ulo-ws-hyper` drop
tokio and hyper-util's `tokio` and take `ulo-listen-tokio` behind `tokio`; `ulo-grpc` gains
`ulo-listen-tokio`. `ulo-http-conformance` drops `reqwest` and its unconditional `ulo-tokio` and
tokio, and gains `ulo-hyper-serve`, hyper's client, `http-body-util`, `futures-io`,
`futures-channel` and `tokio-util` with `compat`, with `ulo-tokio`, `ulo-listen-tokio` and tokio
behind `tokio`, and `ulo-smol`, `ulo-listen-smol`, `async-io` and `async-net` behind `smol`. The
workspace gains `async-net`, `futures-rustls`, `httparse` and `piper` in
`[workspace.dependencies]`; `Cargo.lock` gains `async-net` 2.0.0, `ulo-listen-smol` and
`ulo-listen-tokio` (`piper`, `httparse`, `blocking` and `futures-rustls` were already in it), and
loses `reqwest`, `tower-http`, `wasm-bindgen-futures` and `web-sys`, which only the suite's
`reqwest` brought.

## Decisions

### 1. The tree goal, and where hyper sits

- **What hyper brings.** hyper 1.11.1 depends on tokio with `sync` and no `optional`
  (`hyper-1.11.1/Cargo.toml:171-173`), for the `oneshot` inside its upgrade future
  (`src/upgrade.rs:53`); 1.10.1 declares the same. h2 0.4.19, behind hyper's `http2`, depends on
  tokio with `io-util` and on `tokio-util` (`h2-0.4.19/Cargo.toml:102-110`). Any crate depending
  on hyper has tokio in its normal tree. The transports DESIGN's §8 table already says so (line
  1261); the brief's goal, an empty `cargo tree -p ulo-hyper-serve -e normal -i tokio`, and the
  same for the smol listener, did not account for it.
- **What was built.** `ulo-hyper-serve` depends on hyper with no features, for the I/O traits its
  `Listener::Io` is bounded by and for `RuntimeExecutor`, `RuntimeTimer` and `FuturesIo`, as the
  design places them. Both tree checks print `tokio v1.53.2 └── hyper v1.11.1 └── ..`, tokio's
  features `default` (empty) and `sync`, and no other path (verification). What tokio adds there is
  its synchronisation module; no reactor, executor or timer is compiled.
- **The alternative, not built.** A hyper-free `ulo-hyper-serve` whose listeners hand out streams
  on their runtime's own traits, the conversion into hyper's traits and the executor and timer
  adapters moved into each hyper-based server or a crate of their own. The two tree checks would
  then print nothing, but every crate that serves HTTP would still carry tokio through hyper, so
  the check would hold of two crates no server runs without; and the listener's stream would no
  longer be on hyper's traits, which the design names, or the tokio path would convert twice per
  read, which the fortieth response's S2 ruled out. F383 records the constraint; S1.

### 2. The plug point: `accept` answers an `Incoming` carrying its handshake

- **TLS on the listener side, timed by the loop.** `accept` answers the peer, the local address and
  the handshake as a boxed future that yields the I/O and what TLS settled; a plain listener
  answers it ready. `Serve` races that future against the handshake timeout, on the app's
  `Timer`, and against the drain, on the connection's own task, as it raced `tokio-rustls`'
  handshake before. A failure or timeout is logged at `debug` with the peer, as before.
- **Why not a `handshake(&self, stream)` on the listener.** A connection task calling back into
  the listener would hold it, and the socket, past the drain, where dropping the listener is what
  stops accepting. The accepted connection carries a clone of the acceptor instead, which is an
  `Arc` in both TLS crates; the cost is one allocation per connection for the boxed future.
- **The tokio listener refuses to adopt outside a tokio runtime,** with an `io::Error` naming the
  endpoint and `ulo_tokio::Tokio`, which `bind` reports, where `TcpListener::from_std` panicked. A
  gRPC server bound on a smol app now fails `listen()` that way.

### 3. The read count stays in the loop

`Io<T>` wraps any listener's I/O and counts at the first read that yields bytes, as `Io` did over
tokio's traits. hyper's `ReadBufCursor` is moved into the read it is handed and cannot report how
far it was filled, so until the first counted read the wrapper reads into a `ReadBuf` of its own
over the cursor's unfilled memory and advances the cursor by what it reports, the way hyper-util's
`TokioIo` reads through tokio's buffer; two `unsafe` calls, each with its reason. After the first
read the cursor passes straight through. The alternative was a count inside each listener crate,
which every listener, a third one included, would have to repeat.

### 4. hyper's executor, timer and I/O over the app's

- **`RuntimeExecutor`** spawns each future hyper hands it on the app's `Runtime` and detaches it,
  as hyper-util's executors do; HTTP/2's stream tasks therefore run on the app's runtime and a
  panic in one is redacted with the app's secrets.
- **`RuntimeTimer`** sleeps on the app's `Timer`. hyper's `Sleep` must be `Sync` and a `BoxFuture`
  is not, so each sleep sits in a `Mutex` reached only through `&mut`. `sleep_until` measures the
  deadline against the app clock's `now`, which hyper reads through `Timer::now` too, so a test's
  fake clock is the one both read.
- **`FuturesIo`** is both directions, as `TokioIo` is. Into hyper it zeroes the unfilled part of
  hyper's buffer before each read, `futures-io` reading into initialized memory; that cost falls on
  the smol path alone, the tokio listener handing hyper `TokioIo` as before. Out of hyper, an
  upgraded connection, it needs no zeroing, and both servers now build `Upgraded::from_futures`
  over it on either runtime, where they built `Upgraded::from_tokio(TokioIo::new(io))`, two
  conversions; so neither server enables `ulo-http`'s `tokio-io` any more.

### 5. Naming, and how a user picks a listener

- **Crates:** `ulo-listen-tokio` and `ulo-listen-smol`, role then library as the workspace names
  its integration crates; types `TokioListener` and `SmolListener`, beside `ulo_tokio::Tokio` and
  `ulo_smol::Smol`.
- **Generic types with tokio aliases.** `HyperOn<L>` and `ServerOn<L>` take the listener;
  `ulo_http_hyper::Hyper`, `ulo_http_hyper::Server` and `ulo_ws_hyper::Server` are aliases on
  `TokioListener`, so every existing line, `Server::new(..)` and `Hyper::default()` with
  `Server::with_backend` included, compiles unchanged. On smol:
  `ulo_http_hyper::ServerOn::<SmolListener>::new(..)` and
  `ulo_ws_hyper::ServerOn::<SmolListener>::new(..)`.
- **Why not a default type parameter.** A default does not drive inference in expression
  position: with `type Server<L = TokioListener>`, `Server::new("..")` is E0283, "type
  annotations needed" (probe, `probes/alias/`). A builder method would have to change the type
  after construction, which is the generic parameter again.
- **The `tokio` feature,** default on both crates, is what brings `ulo-listen-tokio`; a smol build
  turns it off and compiles no tokio runtime crate. S2.

### 6. The backend reaches the app's runtime through `AppService::runtime()`

`Backend::bind` hands the backend its `AppService`, which now carries the app's runtime, taken
from `Mounted::runtime()` in `prepare` beside the timer it already carried. The alternative was a
fifth parameter on `Backend::bind`, a change to the SPI's one method that both implementers, the
hyper backend and `ulo-http`'s test `Keeper`, would follow. S3.

### 7. Every task the servers start is the app's

The accept loop, each connection's task and, through `RuntimeExecutor`, HTTP/2's stream tasks now
spawn on the app's runtime. The two tests counting what the app's runtime is handed through a
WebSocket server (`crates/ulo-ws/tests/runtime.rs`, `crates/ulo-ws-hyper/tests/runtime.rs`) count
them: three once the server serves (broadcast delivery, `AfterInit`, the accept loop, the last
waited for since `serve` runs on its own task), five after the connection's 101 (its HTTP/1.1
task and its WebSocket task), six after one message. gRPC keeps hyper-util's `TokioExecutor` and
`TokioTimer`, tonic being tokio-only; its accept loop and connections go through the app's runtime
like the others'.

### 8. The HTTP suite's runtime half is a `Harness` the `Host` names

- **Shape.** `Host::Harness: Harness`, one line per host, carries `runtime()`, `block_on`, the
  listener the reference host accepts on and the client's `connect`. The WebSocket suite puts
  `runtime()` and `block_on` on `Host` itself; here the reference host compared against in
  `same_as_reference` must run on the host's runtime and on that runtime's listener, so the runtime
  half is a type both share: `HyperHost<H::Harness>`. `OnTokio` (feature `tokio`) is every tokio
  host's; `OnSmol` (feature `smol`) is an `async-executor` `Executor` per scenario run by the test's
  thread and three more, as `ulo-smol`'s own suite runs, the reference on `SmolListener` and the
  client on `async-net`. S4.
- **The client.** `reqwest` is gone. A request is hyper's HTTP/1.1 client connection over the
  harness's stream, one per request, with `Host` and, unless the exchange names one,
  `accept: */*`, which `reqwest` added; the whole exchange is bounded by `PATIENCE` on the app's
  clock, and a timeout is a `Failure::TimedOut` the scenarios reject as before. `drain_http2` holds
  an HTTP/2 connection through hyper's client with prior knowledge, its tasks on `RuntimeExecutor`;
  `drain_goaway` keeps `h2`, which reports the GOAWAY frame, over the stream seen through
  `tokio-util`'s `compat`, which converts traits and starts nothing. Every sleep, deadline and
  task in the suite is the app's `Timer` and `Runtime`; `Slots` blocks the test thread before
  `block_on`, as the WebSocket suite's does. S5.
- **What tokio remains.** At default features the suite's tree reaches tokio through hyper, h2 and
  `tokio-util`, with `sync`, `io-util` and the `compat` traits (F383); `OnTokio` brings the runtime
  only behind `tokio`.
- **The suite's app.** `/endless` sleeps on the app's `Timer`, injected into the controller as a
  `Dep<dyn Timer>` field: as a handler parameter, `Dep<dyn Timer>` fails to compile (F384). The
  echo upgrade handler spawns its task on the runtime it takes from `AppHandle::runtime()` in its
  `prepare`, where it called `tokio::spawn`.

### 9. The smol hosts

- **The HTTP suite** runs `HyperHost<OnSmol>` (`crates/ulo-http-hyper/tests/conformance_smol.rs`),
  with the reference's declaration, `routing_extension`, which holds of the hyper backend on any
  runtime.
- **The WebSocket suite** runs the standalone server on `SmolListener`
  (`crates/ulo-ws-hyper/tests/conformance_smol.rs`) and, beyond the brief, the HTTP port on
  `ServerOn<SmolListener>` (`crates/ulo-ws/tests/conformance_smol.rs`), the thirty-eighth
  response's "both hosts"; both reuse the HTTP suite's `OnSmol` through a dev-dependency with
  `smol`, rather than a third copy of the harness. S6.
- **The pipe host** (`crates/ulo-ws/tests/conformance_pipe.rs`) is `tests/table.rs`'s shape made a
  `Host`: a server that builds the `GatewayTable` in `prepare`, takes connections from an
  in-memory pipe (`piper`) handed to it by `connect` through a registry keyed by the address its
  `bound()` reports, which nothing binds, reads request heads a byte at a time with `httparse`,
  answers each with `GatewayTable::handshake`, and on a 101 calls `Switch::serve`. Its HTTP/1.1 is
  what the suite reaches: requests one after another, a body skipped by its `Content-Length`, a
  connection closed after an HTTP/1.0 or `Connection: close` request, and at the drain an idle
  connection closed and one whose head is arriving answered first, so it declares
  `CLOSES_IDLE_AT_DRAIN` as the hyper hosts do. No scenario is declared not applicable. S7.

### 10. Port 0 and socket activation, on both listeners

`crates/ulo-http-hyper/tests/listeners.rs` serves the suite's app on each listener at port 0, and
hands each listener a socket through the activation protocol: the test binds a port, starts the
test binary again as the ignored `child_serves_an_inherited_socket` through `/bin/sh -c
'LISTEN_PID=$$ exec "$0" "$@"'`, which sets `LISTEN_PID` to the process ID the binary then runs as,
with the socket placed at descriptor 3 by `dup2` in `pre_exec` (`fcntl` clearing `FD_CLOEXEC`
where it is already 3), `LISTEN_FDS=1` and `LISTEN_FDNAMES=web`; the child binds
`Endpoint::inherited("web")`, reports on stdout, and a request to the port is answered by its app.
Its stdin closing stops it; a guard kills it if the test fails first.

### 11. A feature unification the workspace check hid

With `tokio-io` dropped from `ulo-ws-hyper`'s dependencies, `cargo check --workspace
--all-targets` passed while `cargo test -p ulo-ws-hyper` failed to compile its tokio host's
`Upgraded::from_tokio`: another member enabled the feature for the whole build. `ulo-ws-hyper`
names `ulo-http` with `tokio-io` as a dev-dependency now. Each touched crate's tests were then run
alone, `-p` by `-p` (verification).

## The tests

- **`crates/ulo-hyper-serve/tests/serve.rs`**, the plug point through `Serve`, on smol, over two
  listeners of the test's own (`async-net`, `FuturesIo`): `a_connection_reaches_the_closure_with_its_addresses_and_is_served_through_hyper`
  (peer, local address, no TLS, HTTP/1.1, the read count at 1, an answer through hyper);
  `a_handshake_past_its_timeout_is_dropped_on_the_app_clock_and_never_served` (a handshake that
  never finishes dropped no sooner than its 150 ms timeout, the closure never called);
  `the_drain_resolves_draining_stops_accepting_and_waits_for_the_connections`;
  `the_accept_loop_and_every_connection_are_tasks_of_the_runtime_serve_was_given` (a counting
  runtime: one task for the loop, one per connection).
- **`crates/ulo-hyper-serve/tests/rt.rs`**: `http2_s_tasks_are_spawned_through_the_runtime_executor`;
  `the_header_read_timeout_is_timed_by_the_runtime_timer` (half a head, the connection closed);
  `a_deadline_is_measured_on_the_app_s_clock` (a clock an hour ahead asked for at most 50 ms).
- **`crates/ulo-listen-tokio/tests/listener.rs`**: `a_connection_on_port_0_is_reported_with_both_its_addresses`,
  `a_tls_connection_reports_the_alpn_and_sni_its_session_settled` (h2, `localhost`, HTTP/2),
  `adopting_outside_a_tokio_runtime_is_refused_naming_the_listener`.
- **`crates/ulo-listen-smol/tests/listener.rs`**: the first two of those on a smol executor, the TLS
  client `futures-rustls` over `async-net`.
- **`crates/ulo-http-hyper/tests/tls_smol.rs`**: `tls.rs`'s two on `SmolListener`, ALPN and an
  upgrade over TLS carrying bytes both ways.
- **`crates/ulo-http-hyper/tests/listeners.rs`**: port 0 and socket activation on each listener
  (decision 10), four tests and the ignored child.
- **Smol hosts:** `crates/ulo-http-hyper/tests/conformance_smol.rs` (the HTTP suite, 38 stamped,
  36 run, `routing_extension` declared in both modes as on the tokio reference);
  `crates/ulo-ws-hyper/tests/conformance_smol.rs`, `crates/ulo-ws/tests/conformance_smol.rs` and
  `crates/ulo-ws/tests/conformance_pipe.rs` (the WebSocket suite, 31 each, none declared).
- **Changed:** the three runtime-count tests (decision 7, `crates/ulo-graphql-ws/tests/runtime.rs`
  among them, found by the workspace run, two more tasks before the gateway's own: 7 and 8 where
  5 and 6); every HTTP suite host gains `type Harness = OnTokio`; `tls.rs` and salvo's
  `run_refuses_a_closing_from_another_handle` pass the runtime to `app` and `app_for`.

### Before and after

Each break went through `runtime-e2/scripts/brk.py`, which replaces each span (asserting it occurs
once), runs the target, writes every touched file and `Cargo.lock` back byte for byte, and
compares a hash over `git diff HEAD` of `crates`, `Cargo.toml` and `Cargo.lock` and every
untracked file under `crates`. Every restore reported `ok`, the hash the one recorded before the
first break (`a555ea9e..`), but the planted tree check, run after the final edits (`a104996f..`,
before and after). A span occurring nine times was refused before anything was written
(`broken/00-selftest.txt`). Specs in `runtime-e2/specs/`, output in `runtime-e2/broken/`.

| Break | Target | Result |
| --- | --- | --- |
| `Serve` never arming the handshake timeout | `serve.rs`'s timeout test | failed: "the hanging handshake's drop did not happen within 5s" |
| `drain` raising no draining signal | the drain test | failed: "the drain did not happen within 5s" |
| each connection awaited in the accept loop instead of spawned | the runtime test | failed: "both connections' tasks did not happen within 5s" |
| `Io<T>` not counting its first read | the first `serve.rs` test | failed: the read count 0 where 1 |
| `RuntimeExecutor` dropping each future | the HTTP/2 test | failed: "hyper::Error(User(DispatchGone), \"runtime dropped the dispatch task\")" |
| `RuntimeTimer`'s sleep never ready | the header-timeout test | failed: the connection not closed within 5s |
| `sleep_until` measured against `Instant::now()` | the deadline test | failed: "asked it for 3600.049985292s" |
| `FuturesIo` into hyper never advancing the cursor | the first `serve.rs` test | failed: `hyper::Error(IncompleteMessage)` |
| `FuturesIo` out of hyper reading end of stream | `tls_smol.rs`'s upgrade test | failed: the TLS client's unexpected EOF |
| the tokio listener reporting an empty `TlsInfo` | its TLS test | failed: "the protocol ALPN settled" |
| the tokio listener's runtime check removed | its adopt test | failed: tokio's panic, "there is no reactor running" |
| the smol listener ignoring its TLS configuration | its TLS test | failed: "the TLS handshake did not happen within 5s" |
| the smol listener reporting no local address | its port-0 test | failed: the local address `None` |
| the smol listener binding a socket of its own in place of the one it adopts | `listeners.rs`'s smol port-0 and activation tests | both failed: connection refused; no answer within 10 s (the read timeout, `WouldBlock`) |
| the same on the tokio listener | the tokio pair | both failed the same way |
| `OnSmol` naming `TokioListener` | `conformance_smol`'s `nested::routing_hit` | failed: the reference refused, "the tokio listener cannot adopt 127.0.0.1:0: no tokio runtime is running; .." |
| the standalone server's `Serve` on another runtime than the app's | `ulo-ws-hyper`'s runtime test | failed: "the accept loop's task did not happen within 5s" |
| tokio with `net` planted in `ulo-listen-smol`'s dependencies | the features tree | printed `tokio feature "net"` and `ulo-listen-smol` as a direct dependent beside hyper |

F384's probe, the handler parameter restored, failed `cargo check -p ulo-http-conformance` with
the error the entry quotes, and restored by hash (`probes/f384-dyn-timer-handler-param.txt`).

## Left for the transports DESIGN fold

Line numbers are those read on the working tree during this stage; another agent was editing the
file meanwhile.

- §0, principle 5 (line 13): a runtime enters `fw-hyper-serve`'s servers through their listener,
  `fw-listen-tokio` or `fw-listen-smol`; `fw-hyper-serve` spawns and times through the app's
  `Runtime` and `Timer`, and carries tokio only through hyper's `sync` (F383).
- §1's crate table: `fw-hyper-serve`'s row (line 27) becomes the accept loop over a `Listener`,
  the handshake an `Incoming` carries, the timeout on the app's `Timer`, one task per connection in
  a `TaskSet` on the app's `Runtime`, `RuntimeExecutor`, `RuntimeTimer` and `FuturesIo`, depending
  on hyper with no features; rows for `fw-listen-tokio` (tokio sockets, `tokio-rustls`, `TokioIo`)
  and `fw-listen-smol` (`async-net`, `futures-rustls`, `FuturesIo`) after it; `fw-http-hyper`'s
  (line 29) and `fw-ws-hyper`'s (line 33) gain `HyperOn<L>` / `ServerOn<L>`, the tokio aliases and
  the `tokio` feature; `fw-grpc`'s (line 38) accepts through `Serve<TokioListener>`.
- §2.7, TLS (line 348): the acceptor is built by the listener, `fw-listen-tokio` through
  `tokio-rustls` and `fw-listen-smol` through `futures-rustls`, not by `fw-hyper-serve`; the TCP
  link's is unchanged.
- §3.5, the standalone server (line 493): `ServerOn<L>` and `Server`; the 101 is served as
  `Upgraded::from_futures(FuturesIo::new(io))`; "what runs on tokio is the accept loop, TLS and the
  HTTP/1.1 exchange up to the 101" becomes what runs on the listener's sockets, every task on the
  app's runtime; the pipe host as the suite's third host beside `tests/table.rs`.
- §3.7, the accept loop (line 558): the paragraph's `Serve::new(listeners, tls, &ServeConfig)`
  gains `runtime`, the adoption moves to `Listener::adopt`, the handshake to `Incoming`,
  `JoinSet` to `TaskSet`, `select!` to the loop's own polls, `Io::poll_read` to `Io<T>`'s read
  over hyper's traits; "The loop depends on tokio and `tokio-rustls`" no longer holds;
  `AppService::runtime()`; `RuntimeExecutor` and `RuntimeTimer` in the backend's builders.
- §3.8, conformance (line 692): `Host::Harness`, `Harness`, `OnTokio` and `OnSmol` behind
  features, `HyperHost<R>`, `app(runtime)` and `app_for(limits, runtime)`, the client on hyper's
  and `h2`'s connections, `Counted` behind `tokio`, `Slots` blocking the test thread; the smol
  host of the hyper backend.
- §6.2, gRPC's engine (line 1096): on `fw-listen-tokio`, its accept loop and connections on the
  app's runtime, hyper-util's `TokioExecutor` and `TokioTimer` kept.
- §8's table (lines 1261-1263): `fw-hyper-serve` leaves the tokio-bound row for one of its own,
  tokio through hyper's `sync` alone; `fw-listen-tokio` is tokio-bound, `fw-listen-smol` is not;
  `fw-http-hyper` and `fw-ws-hyper` are tokio-bound only through their default `tokio` feature and
  hyper; `fw-ws-hyper` no longer uses `fw-http`'s `tokio-io`.
- §8's "Not built" (line 1266): the listener plug point, both listener crates, the suites taking a
  runtime and the smol hosts of both suites, the pipe host among them, are built; the smol TCP
  link, the CI job and the forbidden-dependency check are not. The paragraph's closing claim stays
  until the CI job passes, per the thirty-eighth response.
- The X table, X28 (line 1333): the TLS acceptor is the listener's; a row for the `Listener`
  plug point, `Incoming`, `tls_info`, `RuntimeExecutor`, `RuntimeTimer`, `FuturesIo`,
  `AppService::runtime`, `HyperOn`, `ServerOn`, the two listener crates and the suite's `Harness`.
- Decision 83 (line 1520): the count is `Io<T>`'s first read, over hyper's traits.
- A decision for F383: the tree test of §8 holds of the core and the hubs; a hyper-based crate
  reaches tokio through hyper and nothing else, which is what stage f checks of it.

## Needs sign-off

### S1. The tree goal stands unmet; `ulo-hyper-serve` depends on hyper

Decision 1 and F383. hyper depends on tokio's `sync` unconditionally, so `ulo-hyper-serve` and
`ulo-listen-smol` print tokio through hyper, and through nothing else. The alternative is a
hyper-free `ulo-hyper-serve` whose listeners hand out their runtime's own streams, the conversion
to hyper's traits, the executor and the timer moving to each hyper server or a crate of their own;
it empties those two trees and leaves every server's.

### S2. A generic server type with tokio aliases and a default `tokio` feature

Decision 5. `ulo_http_hyper::Server::new(..)` is unchanged on tokio; smol writes
`ulo_http_hyper::ServerOn::<SmolListener>::new(..)`. The alternatives are a defaulted type
parameter, which does not infer in expression position, or a builder method, which changes the
type after construction.

### S3. The app's runtime reaches a backend through `AppService::runtime()`

Decision 6. The alternative is a fifth parameter on `Backend::bind`.

### S4. The HTTP suite's runtime half is a type the `Host` names

Decision 8. One line per host; the reference host is `HyperHost<H::Harness>`, on the host's
runtime. The alternative is `runtime()`, `block_on` and `connect` on `Host` itself, as the
WebSocket suite has them, with the reference's listener named on the host too.

### S5. The suite's client is hyper's, and `h2` reads through `tokio-util`'s `compat`

Decision 8. `reqwest` is gone; `accept: */*` is sent as it sent it. The alternatives are a hand
written HTTP/2 client for the GOAWAY scenario, or a client per runtime.

### S6. The WebSocket suite's smol hosts reuse the HTTP suite's `OnSmol`, and the HTTP port runs on smol too

Decision 9. Two dev-dependencies on `ulo-http-conformance` with `smol`, rather than a copy of the
harness per crate; the HTTP-port host on smol is beyond the brief's two.

### S7. The pipe host speaks the HTTP/1.1 the suite reaches and drains as hyper does

Decision 9. It declares `CLOSES_IDLE_AT_DRAIN`, by closing an idle connection at the drain and
answering one whose head is arriving first. The alternative is keeping idle connections open, the
table answering their next request 503, which the scenario also accepts.

### S8. The handshake travels with the accepted connection

Decision 2. One boxed future per connection; the alternative, a `handshake` method on the
listener, keeps the listener alive past the drain.

### S9. The first-read count stays in the loop, over hyper's traits, with two `unsafe` calls

Decision 3. The alternative is a count in each listener crate.

### S10. The servers' accept loops and connections are the app runtime's tasks; gRPC keeps tokio's executor

Decision 7. Two runtime-count tests now count the server's own tasks. gRPC's hyper builder keeps
hyper-util's `TokioExecutor` and `TokioTimer`.

### S11. The tokio listener refuses to adopt outside a tokio runtime

Decision 2: an `io::Error` naming the endpoint and `ulo_tokio::Tokio`, where tokio panicked.

## Verification

Full, unfiltered output of every run is in the session scratchpad, `runtime-e2/`: `runs/` for the
runs during the build, `broken/` (with `summary.txt` and `baseline-hash.txt`), `specs/`,
`probe-specs/` and `probes/`, `suites/` (with `summary.txt`), `tree/` (with `summary.txt`),
`verify/` (with `summary.txt`) and `scripts/`.

- **The smol hosts and the tokio hosts beside them, three consecutive runs each,** one target at a
  time, `--locked`, libtest's time (`suites/summary.txt`): the HTTP suite on `HyperHost<OnSmol>` 36
  passed and 2 ignored (0.32 s, 0.32 s, 0.33 s); the WebSocket suite on the standalone server on smol
  31 passed (0.37 s, 0.33 s, 0.35 s), on the HTTP port on smol 31 (0.36 s, 0.33 s, 0.34 s), on the
  pipe host 31 (0.33 s, 0.32 s, 0.33 s); on tokio, the HTTP suite's reference 36 and 2 (0.31 s,
  0.32 s, 0.33 s), the standalone server 31 (0.32 s, 0.33 s, 0.32 s) and the HTTP port 31 (0.32 s,
  0.35 s, 0.32 s). Every scenario that passes on tokio passed on smol; none is declared not
  applicable on a smol host that is not on its tokio counterpart.
- **Each touched crate's tests alone,** `cargo test -p <crate> --locked` with the OpenSSL flags
  (`verify/crate-*.txt`), every run exiting 0: `ulo-hyper-serve` 7 passed and 1 ignored;
  `ulo-listen-tokio` 3 and 1; `ulo-listen-smol` 2 and 1; `ulo-http` 20 and 9, with `tokio-io` 22
  and 9; `ulo-http-hyper` 86 and 8 (the two suites 36 each, `listeners.rs` 4, `tls_smol.rs` 2);
  `ulo-ws-hyper` 107 and 1; `ulo-ws` 116 and 4; `ulo-http-conformance` 0 and 2;
  `ulo-ws-conformance` 1 and 3; `ulo-grpc` 0 and 5, its server's tests being
  `ulo-codegen-tests`', 53 passed; `ulo-http-axum` 69 and 1; `ulo-http-salvo` 70 and 2;
  `ulo-http-poem` 69 and 1; `ulo-http-rocket` 65 and 5; `ulo-http-actix` 35 and 35, and
  `conformance_http2` 38 with `conformance-http2`, its `drain_http2` now through hyper's client.
  Batch 24's counts for the adapters are the same. `ulo-graphql-ws`'s `runtime` target, after
  decision 7's count change, 2 passed in each of three runs (`verify/graphql-ws-runtime-*.txt`).
- `cargo check --workspace --all-targets --all-features` with
  `CFLAGS=-I/opt/homebrew/opt/openssl/include LDFLAGS=-L/opt/homebrew/opt/openssl/lib`, and with
  defaults: exit 0, the 17 known warnings in `crates/ulo/src` and no other location
  (`scripts/diag.py`, `verify/check-*.txt`).
- `cargo +1.88 check --workspace --all-targets --exclude ulo-http-salvo --exclude
  ulo-graphql-async-graphql`: exit 0, the same 17 and no other; the two listener crates,
  `async-net` 2.0 and `futures-rustls` 0.26 build on 1.88.
- `cargo test --workspace --no-fail-fast` with the OpenSSL flags, on the final tree: 942 passed, 0
  failed, 124 ignored across 184 test results (`verify/workspace-test-2.txt`). Batch 24's 795, plus
  `ulo-hyper-serve`'s 7, the listeners' 5, `ulo-http-hyper`'s 42 (the smol suite 36,
  `listeners.rs` 4, `tls_smol.rs` 2), `ulo-ws-hyper`'s 31 and `ulo-ws`'s 62, 147 in all; ignored
  rises by the smol HTTP suite's 2, the activation child and three `ignore` doc examples, 6; the 14
  results added are the new targets and the listener crates' unit and doc runs. The first workspace
  run on the tree before the last edit failed `ulo-graphql-ws`'s two runtime-count tests, 7 tasks
  where 5 (`verify/workspace-test.txt`), which decision 7 accounts for; 940 passed then.
- **The tree checks,** `cargo tree -p <crate> -e normal -i tokio --locked` (`tree/summary.txt`):
  stdout is empty for `ulo`, `ulo-transport`, `ulo-net`, `ulo-http`, `ulo-rpc`, `ulo-ws`,
  `ulo-graphql-ws`, `ulo-graphql-http`, `ulo-ws-conformance`, `ulo-runtime-conformance` and
  `ulo-smol`, each exiting 101 with "did not match any packages" or 0 with "nothing to print".
  `ulo-hyper-serve` prints three lines and `ulo-listen-smol` four: tokio, hyper, and the crate
  through `ulo-hyper-serve`; with `-e normal,features`, tokio's features are `default` and `sync`
  and its one direct dependent hyper, for both (F383). `ulo-ws-hyper` without its `tokio` feature
  reads the same; `ulo-http-hyper` without it, and `ulo-http-conformance` at its default features,
  reach tokio through hyper, h2 and `tokio-util` with `bytes`, `default`, `io-util` and `sync`;
  `ulo-http-hyper` at its defaults adds `net`, `rt`, `time`, `mio` and `libc` through
  `ulo-listen-tokio`, `hyper-util` and `tokio-rustls`, and `ulo-listen-tokio`'s tree is the positive
  control. The planted check (the breaks table) printed `net` and the crate as a direct dependent.
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --lib -p <crate>` for `ulo-hyper-serve`,
  `ulo-listen-tokio`, `ulo-listen-smol`, `ulo-http`, `ulo-http-hyper`, `ulo-ws-hyper`, `ulo-grpc` and
  `ulo-http-conformance`: each exits 0 (`verify/doc-*.txt`). `ulo-http-conformance` failed first, a
  link to `ReadCount::wrap`, which exists only behind `tokio`; the docs name it in code text, and the
  crate's docs build at default features and with `tokio,smol`.
- `cargo +1.98.1 clippy` over the fifteen touched crates (`ulo-graphql-ws` among them) with
  `--all-targets --no-deps --locked` and the OpenSSL flags: exit 0, 49 warning locations, 31 outside
  `crates/ulo/src`, none on a changed line or in a new file, by `scripts/changed_lines.py` reading
  `git diff -U0` and the untracked files; given an existing location as a planted changed line, it
  reported it (`verify/checker-selftest.txt`).
- **Containers:** none started. The user's four, seaweedfs, mailpit, postgres:18 and redis:7, were
  the only ones running at the end (`verify/docker.txt`). No activation child was left running. No
  permission check refused an action.
