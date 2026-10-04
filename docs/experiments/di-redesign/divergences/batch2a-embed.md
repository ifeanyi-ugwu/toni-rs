# Divergences: race 2a, the embedding surface and the hyper rename

The race 2a part of the sixth response: `crates/ulo-http-axum` renamed to `crates/ulo-http-hyper`,
the `ulo_http::embed` module, `Host<T>`, `PreDispatch::adopt`, `Routing`, `Miss`,
`HttpCx::mount_prefix`, the panic catch around `ExecBody::poll_frame` (R12), and the two core
additions (R3, Q1). Each entry gives what was written, where the signed text left it open or could
not be built as written, and why.

Files changed: `Cargo.toml` (members), `crates/ulo-http-hyper/{Cargo.toml, src/lib.rs,
src/backend.rs, src/convert.rs}`, `crates/ulo-http/{Cargo.toml, src/lib.rs, src/embed.rs (new),
src/routing.rs (new), src/extract/host.rs (new), src/extract/mod.rs, src/__private.rs, src/cx.rs,
src/pre_dispatch.rs, src/render.rs, src/request.rs, src/router/mod.rs, src/server.rs,
src/service.rs}`, `crates/ulo-http-macros/src/route.rs`, `crates/ulo-transport/src/{extract.rs,
error.rs}`, `crates/ulo/src/{lib.rs, app/handle.rs, graph/scopes.rs, transport/server.rs}`, and
the manifests and `use` paths of nine excluded crates (entry 17).

## The signature

```rust
// ulo (core, additive)
impl<'a, T: Transport> Mounted<'a, T> {
    pub fn handlers_reading<I: Send + Sync + 'static>(&self) -> Vec<InputReader<'a, T>>;
}
pub struct InputReader<'a, T: Transport> { .. }
impl<'a, T: Transport> InputReader<'a, T> {
    pub fn handler(&self) -> &'a MountedHandler<T>;
    pub fn path(&self) -> &[String];
}
impl AppHandle { pub fn drain_timeout(&self) -> Duration; }

// ulo_transport
pub enum ExtractError { .., HostMissing { param: &'static str, type_name: &'static str } }

// ulo_http
pub struct Host<T>(pub T);                       // FromCall<Http> for T: Clone + Send + Sync + 'static
pub enum Routing { Matched { route: Arc<str>, handler: Arc<HandlerInfo> }, NotFound, MethodNotAllowed }
impl HttpCx { pub fn mount_prefix(&self) -> &str; }
impl PreDispatch { pub fn adopt<T: Clone + Send + Sync + 'static>(&mut self) -> &mut Self; }

// ulo_http::embed
pub trait Embed: Send + Sync + 'static { const NAME: &'static str; fn limits() -> EmbedLimits; }
pub struct EmbedLimits { pub peer_addr, pub upgrades, pub forward_miss, pub host_extensions,
                         pub tls_info: bool, pub request_body: RequestBody, pub disconnect: Disconnect }
pub enum RequestBody { Streamed, Buffered(u64) }
pub enum Disconnect { AtClose, AtNextWrite }
pub enum Miss { Final, Forward }
pub struct Forwardable { .. }
pub struct Embedded<A: Embed> { .. }              // impl ulo::Server, Transport = Http
impl<A: Embed> Embedded<A> {
    pub fn new() -> Self;  pub fn handle(&self) -> Handle;
    pub fn body_limit(self, u64) -> Self;  pub fn max_inflight(self, usize) -> Self;
    pub fn shed_retry_after(self, Duration) -> Self;  pub fn challenge(self, impl Into<Cow<'static, str>>) -> Self;
    pub fn timeout_grace(self, Bound) -> Self;
    pub fn nested_at(self, impl Into<Cow<'static, str>>) -> Self;  pub fn peer_addr(self, bool) -> Self;
    pub fn on_miss(self, Miss) -> Self;
}
pub struct Handle { .. }                          // Clone
impl Handle {
    pub fn service(&self) -> Service;
    pub fn host<F, E>(&self, server: F) -> Result<(), BoxError>;   // F: Future<Output = Result<(), E>> + Send + 'static
    pub fn stopping(&self) -> Stopping;           // Future<Output = ()>
    pub fn app(&self) -> Option<AppHandle>;
}
pub struct Service { .. }                         // Clone; tower::Service<http::Request<B>>, Error = Infallible
impl Service { pub fn respond(&self, req: Request) -> impl Future<Output = Response> + Send + use<>; }
```

## Decisions

### 1. The handle comes from `Embedded::handle()`

- **Written:** `Embedded::<A>::new()` returns the server, and `server.handle()` the
  `embed::Handle`, as `App::handle()` does. This is the spelling the folded DESIGN §3.8 uses.
- **Why:** the fifth response's `let (server, embedded) = fw_http::Embedded::new().body_limit(..)`
  has no valid shape: a builder method cannot chain on a tuple.

### 2. `Embed` carries `NAME` and `limits()` only; the connection comes from the request

- **Written:** `embed::Service` reads a `ConnInfo` from the request's `http::Extensions`, and builds
  one carrying only the HTTP version when none is there. An adapter whose host keeps the peer
  elsewhere (axum's `ConnectInfo`) inserts a `ConnInfo` from its own layer before the service.
  `Service::respond(ulo_http::Request)` takes a request a native adapter built itself, with its
  `conn` and `upgrade` filled; the tower `call` converts and calls it.
- **Why:** DESIGN §3.8 fixes the trait at two items. An `Embed::conn(&Parts)` hook was the
  alternative; a `ConnInfo` extension keeps `ulo-http` free of any host's types without one.
  `respond` exists because `OnUpgrade` is not `Clone` and cannot ride `http::Extensions`
  (`http` 1 requires `Clone` there), so a native adapter with its own upgrade mechanism needs an
  entry point that takes the app's `Request`.

### 3. The host slot and the stop signal on the handle

- **Written:** `Handle::host(fut)` installs the host's server future; `Embedded::serve` waits for
  it when the slot is empty and polls it; `Embedded::close` awaits the completion notice when
  `serve` holds the future, and drops a future installed and never polled. A second `host` call, or
  one after `close`, answers `Err` and drops the future. `Handle::stopping()` is a `'static` future
  `Embedded::drain` resolves, which `close` also resolves for an app closed without a drain.
  `Handle::app()` answers the `AppHandle` once `prepare` has run, for an adapter's `run` reading
  `drain_timeout()`.
- **Why:** R4 has `drain` "signal the host and return"; the signal needs a value the host's
  graceful shutdown can await. DESIGN §3.8 has `run` wire the core's `draining()` instead, which
  resolves at the same moment; both are available, and `stopping()` is the one that needs no
  `AppHandle` before `listen()` returns. `host` taking `&self` is what `run(app, &embedded, ..)`
  needs.

### 4. "Before the trigger" is `Phase::Running`

- **Written:** when the host future ends, `Embedded::serve` reads `AppHandle::phase()`: `Running`
  means the shutdown has not begun, and `Ok` is turned into `Err("the <name> host server stopped
  before the shutdown began")`. A host `Err` is `Err("the <name> host server failed: ..")` in any
  phase. The core then shuts down under `transport `Http` failed: ..`.
- **Why:** Q2. A host stopping during the before-shutdown stage (`Stopping`) is after the trigger.

### 5. After `close`, the handle answers 503 as during the drain

- **Written:** the handle's state moves `Unbound → Bound → Closed`. `Unbound` answers 503
  "the application is not yet listening" with `Retry-After` (R14), rendered from the builder's
  settings, which every builder method copies into the shared state. `Closed` answers the drain's
  503 with `Connection: close`. A `listen()` that fails after this server bound closes it, so a
  failed app does not keep serving through the handle.
- **Why:** the signed text covers before `listen()` and during the drain; after `close` the
  `AppService` would still open executions on a failed app.

### 6. `Routing` is absent where the app answered before routing

- **Written:** `Routing` is set when routing decides and copied onto the outgoing response if a
  pre-dispatch entry rebuilt it. It is absent from a load-shedding or drain refusal, the
  embedding's 503 before `listen()`, a response an unscoped pre-dispatch entry gave without calling
  `next`, and an upgrade request an `UpgradeHandler` took. The enum is `#[non_exhaustive]`.
- **Why:** Q6 says "every response"; no routing decision exists for those, and none of the three
  variants is true of them. A fourth variant (`Unrouted`) was the alternative; it is not in the
  signed enum. An upgrade handler exposes no `HandlerInfo`, so `Matched` has nothing to name.
- **Sign-off needed:** absence for these cases, or a fourth variant.

### 7. `OPTIONS` answered by the router is `Routing::MethodNotAllowed`

- **Written:** the 204 with `Allow` for an `OPTIONS` on a path with no `OPTIONS` handler carries
  `MethodNotAllowed`.
- **Why:** no handler answered it, so `Matched` is false, and the path matched, so `NotFound` is.
  The variant's doc names the 204 case. A host metric keyed on the variant counts these as 405s.
- **Sign-off needed.**

### 8. `Routing::Matched` fields

- **Written:** `route: Arc<str>` is the pattern as the host sees it, the `.nested_at` prefix and the
  controller's prefix applied (`/api/users/{id}`); `handler: Arc<HandlerInfo>`, the record
  `AppHandle::handlers()` lists, carrying the transport key, controller, method name and metadata.
  `Debug` is written by hand: `HandlerInfo` has none.
- **Why:** the fifth response's `Handled` carried route, handler name and transport key;
  `HandlerInfo` holds all three, already in an `Arc`.

### 9. The mount prefix: what each surface records

- **Written:** `.nested_at` is normalized to a leading `/` and no trailing one; `/` or empty means
  not nested; a prefix with a parameter, or one that does not parse as a route, is refused in
  `prepare`. `HttpCx::mount_prefix()` answers the normalized prefix, empty when not nested (on a
  backend too), so `format!("{}{path}", cx.mount_prefix())` never doubles a slash for an app path
  starting with `/`. The span's `http.route` and its `otel.name`, and `Routing::Matched::route`, use
  the joined route (`Pattern::join`, under which `/api` + `/` is `/api`). `HttpCx::route()` stays
  the in-app pattern, the one routing matched. The span's `url.path` stays the path the app
  received.
- **Why:** Q8 reads "`mount_prefix()` joins the prefix and the route", which a method with no
  argument cannot do for an arbitrary path; it answers the half the handler lacks. `url.path` is
  left as received because the host's original path is not recoverable from the stripped one: axum
  delivers both `/api` and `/api/` as `/`.
- **Sign-off needed:** `mount_prefix()` returning the prefix rather than a join helper, and
  `url.path` left stripped.

### 10. `Host<T>` is found by a probe on the handler's parameters

- **Written:** `#[ulo_http::get]` and its siblings emit one `HostProbe` per parameter beside the
  `PathProbe`, ranked by autoref: `Some(type_name)` for `Host<T>` and `Option<Host<T>>`. The probes
  fill `HttpHandler::host_reads`, which `Embedded::prepare` reads under `host_extensions: false`.
- **Why:** `Host<T>` is a `FromCall` extractor, not a container read, so the input walk does not see
  it, and `Dependencies` is opaque outside the core. The probe sees direct parameters only: a
  custom extractor that calls `Host::<T>::from_call` inside escapes the refusal and fails at the
  call with `HostMissing`.
- **Not built:** R10's `.forward::<T>(..)` exemption, race 2b. The refusal names the type; an
  exemption list keyed by `TypeId` is the place it goes.

### 11. `adopt::<T>()` is not refused under `host_extensions: false`

- **Written:** `PreDispatch::adopt::<T>()` is an unscoped entry that copies `T` from the request's
  `http::Extensions` into the execution's when present and passes the request on unchanged when not.
  It runs at its place in declaration order, so a tower layer before it can be its source. It is
  refused nowhere.
- **Why:** the signed text refuses `Host<T>` only. A pre-dispatch tower layer is a second writer of
  those extensions, on a backend as well as embedded, so `adopt` has a source on every host.
  The same reasoning would exempt `Host<T>` read after such a layer; the refusal follows the signed
  text and blocks it.
- **Sign-off needed:** whether the `Host<T>` refusal should stand when a pre-dispatch layer
  supplies the value.

### 12. `Option<P>` maps `HostMissing` to `None`

- **Written:** `impl FromCall<T> for Option<P>` now answers `None` for `ExtractError::HostMissing`
  as for `Missing`. `HostMissing`'s `param` is `"host"`; its `Display` is "`host`: the host supplied
  no `T` with the request", and its public message is the withheld "internal error"
  (`INTERNAL_MESSAGE`, now `pub(crate)` in `ulo-transport`).
- **Why:** Q5 makes `Option<Host<T>>` the optional spelling, and a separate impl for it would
  overlap the blanket one.

### 13. `Miss::Forward` on the app side

- **Written:** under `Miss::Forward` the service wraps the request body in a watcher that records
  the first `poll_frame`, and keeps the path as the client sent it. On a `NotFound` miss with the
  body never polled and the path unchanged, it answers the `NoRoute` 404 problem document with
  `embed::Forwardable` in its extensions and `Routing::NotFound`, skipping the error handlers. Any
  other miss is the app's own 404, logged at `warn`. A 405 is never forwarded. The watcher costs
  nothing when `Miss::Final` is set: no wrapper is built.
- **Why:** Q3's runtime rule, stated as what an adapter reads: `Forwardable` means the original
  request is still intact for the host. The error handlers are skipped because the host answers a
  forwarded request; a host ignoring the marker still sends a 404. A 405 means a route of the app
  matches the path.
- **Sign-off needed:** the 405 rule, and skipping the error handlers for a forwardable miss.

### 14. R12 without new core surface

- **Written:** `ExecBody::poll_frame` runs the inner poll under `catch_unwind`. On a panic it marks
  the body ended, reports `StreamOutcome::CutOff(exec.cancel_reason())` (first report wins, so a
  `Tracked` inside reporting the same on its drop changes nothing), drops the panicked body under a
  second catch, logs at `error`, and answers the error frame, after which the body is empty.
  `PanicRecovered` is built by resuming the payload inside `AppHandle::catch_panic` and polling
  that future once; resuming runs no panic hook, so the panic is reported once, where it happened.
- **Why:** `PanicRecovered` is `#[non_exhaustive]` with a crate-private constructor. A public
  `AppHandle::panic_recovered(stage, payload)` was the alternative, and is a third core addition
  the response does not list. The once-polled future completes on its first poll because nothing in
  it awaits.

### 15. `handlers_reading` returns the path with each handler

- **Written:** `Vec<InputReader<'a, T>>`, each with `handler()` and `path()`, one per handler and
  reading binding, for non-optional reads only. The input walk (`InputWalk`) now collects
  `InputRead { input, path }` for every non-optional input read a handler reaches; `check_inputs`
  filters those by seeder and reports as before, and `handlers_reading` filters by key. A handler
  is matched to its graph record by the identity of its `Arc<HandlerInfo>`.
- **Why:** R3 offers `Vec<&MountedHandler<T>>` "or an equivalent". The path lets the refusal name
  the service that reads the input: `Peer::peer (Http) → Audit (execution) → Dep<ClientAddr>
  (field `addr`) reads `ClientAddr`, and the test embedding supplies no peer address`.

### 16. `AppHandle::drain_timeout()` on an app without a `Timer`

- **Written:** zero, which is the window the drain uses there (`AppConfig::drain`); ten seconds
  unset.
- **Why:** the accessor reports the window in force, not the knob.

### 17. The rename beyond the crate

- **Written:** the backend type is `ulo_http_hyper::Hyper`, `NAME` `"hyper"`; responses go to hyper
  as `http::Response<HttpBody>` with no body wrapper. The `axum` workspace dependency entry stays
  for race 2b. In the nine excluded crates that depended on `../ulo-http-axum`, the manifest entry
  and every `ulo_http_axum::` path in `.rs` files now name `ulo-http-hyper`; the imported items
  (`AxumAdapter`, `TokioSender`) exist in neither crate and are left for those crates' ports. Left
  unchanged: the READMEs' install snippets and the `ulo-cli` template, which pin the published
  `ulo-http-axum = "0.1.0"`, and the rocket adapter's prose comparing itself with the axum adapter.
- **Note:** commit `70d3ac49` ("fold backends, embedding and cfg_attr into the designs") contains
  the directory rename with the old file contents, picked up from the index after `git mv`. The
  content changes are in the working tree.

### 18. `embed::Service` takes hyper's upgrade future only where the host declares `upgrades`

- **Written:** `hyper::upgrade::OnUpgrade` is removed from the request's extensions either way and
  converted only when `A::limits().upgrades`. `ulo-http` gains `hyper` (no features) and
  `hyper-util` (`tokio`) for it.
- **Why:** R1 has the service take the upgrade as the hyper backend's convert does; hyper's
  `upgrade` module is compiled without features.

### 19. `EmbedLimits` mirrors `BackendLimits`

- **Written:** `#[non_exhaustive]` with public fields, `EmbedLimits::NONE` and one `const fn` per
  row, as `BackendLimits` has. `request_body` and `disconnect` are enums. `prepare` reads
  `peer_addr`, `upgrades`, `forward_miss` and `host_extensions`; `tls_info`, `request_body` and
  `disconnect` are for the conformance suite.

## Not covered

- The five adapters, their `run` helpers and the conformance suite (race 2b and later).
- `Routing` on rocket, which has no response extensions: the adapter writes it to the local cache.
- A `Connection: close` mapped to actix's connection-type flag (R7): the actix adapter's.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass;
  the only warnings are the 17 in `crates/ulo/src` present before this change. `cargo test
  --workspace` passes.
- Scratch crate `embed` (own `[workspace]`, path dependencies on `ulo`, `ulo-http`,
  `ulo-http-hyper`, `ulo-tokio`, `#![deny(warnings)]`), two test hosts (`TestHost`: everything but
  the peer address; `NoExt`: no host extensions, no forward), driving `embed::Service` through tower
  `oneshot`. 25 checks pass, with identical output on rustc 1.98.1 and 1.88:

| Check | Result |
| --- | --- |
| request before `listen()` | 503, detail "the application is not yet listening", no `Routing` |
| `GET /users/7`, nested at `/api/` | 200, `Routing::Matched { route: "/api/users/{id}", handler: Api::user }` |
| `GET /nope` / `POST /users/7` | 404 `Routing::NotFound` / 405 `Routing::MethodNotAllowed` |
| `Host<User>` present / missing | 200 "alice" / 500, detail "internal error", no type name in the body |
| `Option<Host<User>>` | "some alice" / "none" |
| `adopt::<User>()` and a guard with an `Option<Ext<User>>` field | 200 with the extension, 403 without |
| `mount_prefix()` | "/api/users/1" |
| a body stream yielding one chunk then panicking | data "first", then the error frame "the handler panicked: the body broke", then the end; `on_stream_end` sees `CutOff(None)`; the `error` log line printed |
| shutdown with a host future that awaits `stopping()` and then lingers 300 ms | `stopping` and the destroy hook at 0 ms after the trigger, the host future's end and `serve`'s return at 301 ms: `drain` returned at once and `close` waited for the host |
| a second `host` call | `Err` |
| request after `close` | 503 "the server is shutting down" |
| a host future ending `Ok` before the trigger | the shutdown signal reads "transport `Http` failed: the test host server stopped before the shutdown began" |
| `Dep<Audit>` with `Audit` execution-scoped reading `Dep<ClientAddr>`, on `TestHost` | refused, naming `Peer::peer (Http) → Audit (execution) → Dep<ClientAddr> (field `addr`)`; the sibling handler reading `Option<Dep<ClientAddr>>` not named |
| the same with `.peer_addr(true)` | listens |
| `NoExt` with `Miss::Forward` | three failures: `Api::host` and `Api::maybe` reading `Host<embed::User>`, and `Miss::Forward` |
| `Miss::Forward` on `TestHost`: `/nope`, `/rewrite` (a middleware rewrites the path), `/read-body` (a middleware reads one frame) | forwardable 404 / app's 404 without `Forwardable`, `warn` logged / the same |
| the hyper backend on port 0, one raw HTTP/1.1 request | `HTTP/1.1 200 OK`, body "user 3"; shut down through `serve` |

- Against a known violation: the scratch crate run against `service.rs` with the `catch_unwind`
  in `ExecBody::poll_frame` removed exits 101, the body's panic unwinding into the test's task
  before the first check after it.
- The prose scan over the added lines for the banned register and em-dashes returns nothing, and
  returns its line for a planted "just" and em-dash.
