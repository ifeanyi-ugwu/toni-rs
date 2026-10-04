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

# Second round: the seventh response

The changes the seventh response (signed off 2026-10-04) asks of this build: `Routing::Unrouted`
and `Routing::Options { route }` (decisions 6, 7), the original path in the span (9),
`.supplies::<T>()` and the one exemption set (11), the `stopping()` doc (3) and the comment at the
one-poll resume (14). Entries continue the numbering above.

Files changed: `crates/ulo-http/src/{routing.rs, service.rs, render.rs, embed.rs,
pre_dispatch.rs, router/mod.rs, extract/host.rs, extract/mod.rs, __private.rs}`. The macro crate
and the core are unchanged: `HostProbe` already carried the type parameter the set compares.

## The signature, added

```rust
// ulo_http
pub enum Routing {                                // #[non_exhaustive]
    Matched { route: Arc<str>, handler: Arc<HandlerInfo> },
    Options { route: Arc<str> },
    NotFound,
    MethodNotAllowed,
    Unrouted,
}
impl PreDispatch { pub fn supplies<T: Clone + Send + Sync + 'static>(&mut self) -> &mut Self; }

// ulo_http::embed
pub struct OriginalPath { .. }                    // Clone, Debug, PartialEq, Eq
impl OriginalPath { pub fn new(impl Into<String>) -> Self; pub fn as_str(&self) -> &str; }
impl From<&http::Uri> for OriginalPath;           // the URI's path, its query left out
impl<A: Embed> Embedded<A> { pub fn supplies<T: Clone + Send + Sync + 'static>(self) -> Self; }
```

## Decisions

### 20. Where `Routing::Unrouted` is set

- **Written:** two places. `render::refusal`, which builds the load-shedding refusal, the drain's
  503, the embedding's 503 before `listen()` and the one after `close`, inserts it. Every other
  response gains `Unrouted` on the way out of `ServiceInner::respond` when routing never recorded a
  decision: an unscoped pre-dispatch entry answering without `next`, one failing or panicking (its
  error rendered through the global error handlers), and an upgrade request an `UpgradeHandler`
  took. A response already carrying a `Routing` keeps it, as before.
- **Why:** the response names "an unscoped pre-dispatch entry answering early"; a failing entry is
  answered by the app before routing in the same way, and no other variant is true of it.

### 21. `Routing::Options { route }` carries the route as the host sees it

- **Written:** `route` is the matched pattern with the `.nested_at` prefix applied, the same value
  `Matched` carries, kept on the router's `RouteEntry` so the `OPTIONS` path builds no string.
  `MethodNotAllowed` now covers only the 405. A `HEAD` answered by a `GET` handler stays `Matched`.

### 22. The original path is an `embed::OriginalPath` extension

- **Written:** an adapter inserts `OriginalPath` into the request's `http::Extensions`; the axum
  adapter will build it with `OriginalPath::from(&original_uri.0)` in race 2b. The span's `url.path`
  records it when present and the path the app received otherwise. It holds the path only:
  `url.path` carries no query, and `From<&http::Uri>` drops it. It is read on a backend as well,
  where nothing outside the app can put it in the request. Only the span reads it:
  `Routing::Matched::route`, `HttpCx::route()` and `HttpCx::mount_prefix()` are as decision 9 left
  them, and the value stays in the request's extensions, where `Host<OriginalPath>` reads it.
- **Why:** a type `ulo_http` defines keeps axum's `OriginalUri` out of the crate, as `ConnInfo`
  does for the connection. It lives in `embed` because only an embedding strips a prefix.

### 23. The exemption set is checked as a set

- **Written:** under `host_extensions: false`, `prepare` collects one set of `TypeId`s from
  `Embedded::supplies::<T>()` and every module's `PreDispatch::supplies::<T>()`, and refuses each
  `Host<T>` a handler reads and each `adopt::<T>()` entry whose `T` is not in it. Neither the scope
  of the entry a `supplies` follows nor its position relative to an `adopt` is checked. The
  refusals name the remedy: "a pre-dispatch entry inserting it is declared with
  `.supplies::<T>()` after it". An `adopt` refusal names the entry's source location.
- **Why:** the response specifies one set with two writers, and the refusal applying "only to types
  nothing declares".
- **Consequence:** two configurations pass that fail per request, as every configuration did before
  the refusal existed. A `.supplies::<T>()` after a `layer_for(["/admin/*"], ..)` lifts `Host<T>`
  on routes outside `/admin`, which then answer `HostMissing` (500). A `.supplies::<T>()` after an
  entry written after `adopt::<T>()` lifts the `adopt`, which runs first and finds nothing, so
  `Ext<T>` fails with `LookupError::NotFound`. A precise check is buildable without new surface: a
  scoped supply counts for the routes its entry's `ScopedStage` covers (the router already holds
  each route's stage), and an `adopt` counts only unscoped supplies earlier in stage order.
- **Sign-off needed:** the set as written, or the precise check.

### 24. A `supplies` written before any entry is refused

- **Written:** `PreDispatch::supplies` records its call site when the module's `PreDispatch` has no
  entry yet, and `prepare` reports it as it reports a stray `exclude`: "`supplies` at {location}
  follows no pre-dispatch entry; it declares what the entry before it inserts, and a value the host
  inserts is declared on the embedding with `Embedded::supplies`". The check runs in
  `Stage::build`, on a backend as well as embedded.
- **Why:** "written after the layer that inserts `T`" names a position; a `supplies` with no entry
  before it declares a value no entry of its module inserts, which on an actix or rocket host is a
  value the adapter's `.forward::<T>()` owes.
- **Sign-off needed:** this refusal is not in the response.

### 25. The adapter's writer is a public `Embedded::supplies::<T>()`

- **Written:** `Embedded<A>` keeps its own list, which `check_host_values` joins with the
  pre-dispatch declarations. An adapter's `.forward::<T>(..)` (race 2b) calls it when it registers
  the copy from the host's store; the method needs no host type. It is public and documented, so
  an app author can call it too; on a host declaring `host_extensions: true` the set is never read.
- **Why:** the adapter crates write `.forward` on `ulo_http::embed::Embedded<A>`, a foreign type to
  them, through an extension trait or a wrapper; either needs a public method on `Embedded` to
  record the type. A `#[doc(hidden)]` method was the alternative.
- **Sign-off needed:** public, or `#[doc(hidden)]` for adapters only.

### 26. `adopt::<T>()` is refused under `host_extensions: false`

- **Written:** entry 11's "refused nowhere" no longer holds. `Step::Adopt` carries its type, and
  the check in entry 23 refuses an `adopt::<T>()` nothing supplies. On a backend and on a host
  declaring `host_extensions: true`, nothing changes.

### 27. `stopping()` and the one-poll resume

- **Written:** `Handle::stopping()` documents itself as the signal an adapter's `run` wires,
  resolving with the core's `AppHandle::draining()` and available before `listen()` returns; the
  module doc says the same. `panic_recovered` carries a comment: one poll completes the caught
  future because `catch_panic` polls the inner future under `catch_unwind` with no await before
  it, and the inner future resumes the panic on its first poll; an await added in either place
  returns `Pending` there, and the fallback then reports the panic without its stage or redaction.

## Not covered

- The axum adapter inserting `OriginalPath` from `OriginalUri`, and `.forward::<T>()` on actix and
  rocket (race 2b).
- The transports DESIGN's `Routing` enum, `.supplies` and `OriginalPath`: the design document is
  not this build's to edit.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass;
  the only warnings are the 17 in `crates/ulo/src` present before this change. `cargo test
  --workspace` passes. `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo-http --no-deps` passes.
- Scratch crate `embed`: the 25 checks of the first round, the one before `listen()` amended to
  expect `Unrouted`, plus 12 new ones, 37 in all, pass with identical output on rustc 1.98.1 and
  1.88 under `#![deny(warnings)]`. `check` now records a failure and the run exits 1 at the end,
  so one run reports every failing check. A `tracing_subscriber` layer captures each `url.path` a
  span records.

| Check | Result |
| --- | --- |
| request before `listen()` | 503, `Routing::Unrouted` |
| `OPTIONS /users/7`, nested at `/api/` | 204, `Routing::Options { route: "/api/users/{id}" }` |
| an unscoped entry answering without `next` / returning `Err` | 200 / 500, `Routing::Unrouted` |
| `OriginalPath::from(&uri)`, `uri` `/api/users/7?x=1`, on a request for `/users/7` / none | `url.path` "/api/users/7" / "/users/7" |
| request after `close` | 503, `Routing::Unrouted` |
| `adopt::<User>()` on `NoExt` | refused: "`adopt::<embed::User>()` at src/main.rs:216:33 copies a host value, and the noext embedding declares `host_extensions: false`: .. `.supplies::<embed::User>()` after it", beside the two `Host<User>` refusals |
| `NoExt`, `.apply_value(InsertUser).supplies::<User>().adopt::<User>()` | listens; `Host<User>` answers "bob", the guard reading the adopted `Ext<User>` admits |
| `NoExt` with `Embedded::supplies::<User>()`, no pre-dispatch declaration | listens |
| `.supplies::<User>()` with no entry before it | refused: "`supplies` at src/main.rs:196:33 follows no pre-dispatch entry; .." |
| `max_inflight(1)`, one response held, a second request | 503, `Routing::Unrouted` |

- Against a known violation: one run with each change mutated out of the source, the crate still
  compiling: `render::refusal` not inserting `Unrouted`, `respond` inserting only a recorded
  decision, the `OPTIONS` miss recorded as `MethodNotAllowed`, `url.path` recording the received
  path, the supplied set empty, the `adopt` refusal disabled, and the stray-`supplies` call site
  not recorded. 11 checks fail, each a check of a changed behaviour: the 10 new ones and the
  amended check before `listen()`. Of the other two new checks, the fallback path passes, being
  the old behaviour, and the one reading `Host<User>` through the supplying entry does not run,
  since its app is refused. The remaining 25 pass. The sources were restored by `cp` from copies
  taken before the mutation, compared byte for byte, and the clean run repeated.
- The prose scan over the added source lines for the banned register and em-dashes returns
  nothing, and returns its line for a planted "just" and em-dash.

# Third round: the eighth response

The changes the eighth response (signed off 2026-10-04, item 3 removing `Embedded::supplies`) asks
of this build: the precise supplied check (item 1), the stray-`supplies` refusal kept with its text
pointing to `Embedded::forward` (item 2), and `Embed::HostRequest` with `Embedded::forward`
replacing `Embedded::supplies` (item 3). Entries continue the numbering above.

Files changed: `crates/ulo-http/src/{embed.rs, pre_dispatch.rs, server.rs, extract/host.rs}`.
`__private.rs`, `extract/mod.rs` and `router/` are unchanged: the check reads the router's
existing `RouteTarget::stage`.

## The signature, changed

```rust
// ulo_http::embed
pub trait Embed: Send + Sync + 'static {
    const NAME: &'static str;
    type HostRequest;                              // new
    fn limits() -> EmbedLimits;
}
impl<A: Embed> Embedded<A> {
    pub fn forward<T: Clone + Send + Sync + 'static>(
        self,
        copy: impl Fn(&A::HostRequest) -> Option<T> + Send + Sync + 'static,
    ) -> Self;                                     // replaces `supplies::<T>()`
    pub fn handle(&self) -> Handle<A>;             // was `Handle`
}
pub struct Handle<A: Embed> { .. }                 // Clone; was `Handle`
impl<A: Embed> Handle<A> { pub fn service(&self) -> Service<A>; /* host, stopping, app unchanged */ }
pub struct Service<A: Embed> { .. }                // Clone; was `Service`
impl<A: Embed> Service<A> {
    pub fn respond(&self, host: &A::HostRequest, req: Request) -> impl Future<Output = Response> + Send + use<A>;
}
impl<A, B> tower::Service<http::Request<B>> for Service<A>
where A: Embed<HostRequest = http::request::Parts>, ..;
```

## Decisions

### 28. A supply belongs to the entry it follows, and the check reads it where that entry runs

- **Written:** `PreDispatch::supplies::<T>()` pushes `{ type, call site }` onto the last entry's
  `supplies`; `PreDispatch::supplied` is gone. Under `host_extensions: false`, `prepare` walks the
  unscoped entries in the order the unscoped sub-step runs them (modules in collection order,
  entries as written). A `Host<T>` read is checked per route, over `ServiceInner::router`'s
  targets: it is supplied by a `forward::<T>`, by any unscoped entry's supply, or by a supply on a
  scoped entry that the route's `ScopedStage::steps` holds, compared by the declaring
  `Arc<PreDispatch>` and the entry's index, as `ScopedStage::holds` compares them. An
  `adopt::<T>()` is supplied by a `forward::<T>` or by a supply on an unscoped entry at an earlier
  position.
- **The refusal for a route:** "`Api::maybe` on `GET /maybe` reads `Host<embed::User>`, and the
  noext embedding declares `host_extensions: false`: the `.supplies::<embed::User>()` at
  src/main.rs:227:83 follows a scoped entry whose scope does not cover `/maybe`, so it does not
  reach this route". The route is the app's pattern, the one scopes are matched against, not the
  mounted one. With no supply of `T` anywhere: "nothing supplies it. A value the host keeps is
  copied in with `Embedded::forward::<T>(..)`, and a pre-dispatch entry inserting it is declared
  with `.supplies::<T>()` after it". Refusals for routes come in the router's precedence order
  rather than the handlers' mount order.

### 29. An `adopt` refusal names each supply that misses it

- **Written:** an `adopt::<T>()` nothing reaches lists every reason that applies, joined by `;`:
  a supply on an unscoped entry at or after its position "follows an entry that runs after this
  `adopt`, which then finds nothing; write the `adopt` after it"; a supply on a scoped entry
  "follows a scoped entry, which runs after routing and so after every `adopt`". With neither, the
  "nothing supplies it" text of entry 28.

### 30. An unscoped entry's `exclude` is not consulted

- **Written:** a supply after an unscoped entry counts for every route, whatever that entry's
  `exclude`.
- **Why:** an unscoped exclusion matches the request path as that entry receives it, and an entry
  after it may still rewrite the path before routing. Comparing the exclusion with route patterns
  would refuse a route that every request reaches through a rewrite, a false refusal; the
  opposite case passes either way. Item 1 asks for the scope of scoped entries, whose exclusions
  `Stage::scoped_for` has already applied to each route's stage.
- **Consequence:** `.apply_value(Auth).exclude(["/public/*"]).supplies::<User>()` lifts `Host<User>`
  on `/public/*` routes, which answer `HostMissing` (500) per request.
- **Sign-off needed:** not consulting it, or comparing it with route patterns and accepting the
  false refusal behind a rewrite.

### 31. A failed stage leaves the `Host<T>` reads unchecked

- **Written:** `prepare_app` answers no service when `Stage::build` failed, as its doc already
  claimed; before this change it answered one built over an empty stage. Without a service the
  `Host<T>` check does not run, since every route's scoped stage in it would read as empty and
  refuse reads a scoped supply covers. The `adopt` check runs regardless: stage order needs only
  the metadata. A backend's `prepare` is unaffected, since a failed stage is a failure there too.
- **Consequence:** a `Host<T>` refusal appears only once the stage's own failures are fixed.

### 32. `HostRequest` has no bounds; a tower host names `http::request::Parts`

- **Written:** `type HostRequest;` with no bounds. `Service<A>` is a `tower::Service` only for
  `A: Embed<HostRequest = http::request::Parts>`, and its `call` runs every copy on the request's
  head before converting it, the copies' values added to the head's extensions through
  `Extensions::extend`. The axum adapter (race 2b) names `Parts`.
- **Why:** the tower impl has to run the copies, or a `forward` on a tower host would declare `T`
  supplied and copy nothing, the false declaration item 3 removes. `Parts` is what a tower host
  hands over besides the body, and it is the only request a copy on such a host can read. A host
  with a request type of its own calls `respond`, so the restriction costs it nothing.
- **Sign-off needed:** the tower impl restricted to `HostRequest = http::request::Parts`.

### 33. `Handle` and `Service` take the host as a type parameter

- **Written:** `Handle<A>` and `Service<A>`, each holding the copies as
  `Arc<OnceLock<Box<[HostCopy<A>]>>>` beside the untyped shared state. `Stopping` stays
  unparameterised. `Embedded::forward` collects into the builder, and `prepare` moves the list into
  the `OnceLock` once the app is accepted, so a handle taken before `.forward(..)` runs the copies
  too. Before `prepare` the list is empty, and a request then answers 503 regardless.
- **Why:** a copy is typed over `A::HostRequest`, and running it needs that type at the call.
  Erasing it behind `Any` would need `HostRequest: 'static` and check the host type at runtime.
- **Consequence:** a user naming the handle's type writes `Handle<ulo_http_axum::Host>`, or the
  adapter's alias.
- **Sign-off needed:** the type parameter on both public types.

### 34. The adapters' API is `Service::respond(host, req)`

- **Written:** `respond` takes the host's request beside the app's, runs every copy on `host` into
  `req.head.extensions`, then answers as before. There is no separate method to run the copies.
- **Why:** an adapter reaches the app through `respond` or the tower impl, and both run the copies,
  so no adapter can answer a request without them.
- **Sign-off needed:** the copies inside `respond` rather than a method an adapter calls before it.

### 35. A copy that panics inserts nothing

- **Written:** each copy runs under `catch_unwind`; a panic inserts nothing for that copy, is
  logged at `error` with the host and the type, and the request goes on. A handler reading the
  value then answers `HostMissing`.
- **Why:** the copy is code the app author writes, and it runs in the host's request task before
  an execution exists, so no error handler can be offered the panic. Letting it unwind would hand
  the panic to the host.

### 36. `type HostRequest;` cannot be rocket's request

- **Found:** rocket's handler receives `&'r Request<'_>`, an arbitrary lifetime, and
  `HostRequest = rocket::Request<'static>` cannot accept it. A probe crate against rocket 0.5.1
  with the trait as written, a rocket `Handler` passing its request to a function over
  `&A::HostRequest`, fails with E0521, "argument requires that `'life1` must outlive `'static`".
  The same probe with `type HostRequest<'r>;`, `type HostRequest<'r> = rocket::Request<'r>` and
  copies stored as `dyn for<'r> Fn(&A::HostRequest<'r>) -> Option<T>` compiles.
- **Written:** the trait as signed, with no lifetime parameter. Actix's `HttpRequest` and `Parts`
  carry none, so race 2b's actix and axum work is unaffected.
- **Sign-off needed before race 2b's rocket adapter:** a generic associated type
  `type HostRequest<'r>;`, which changes `forward`'s bound to `for<'r> Fn(&A::HostRequest<'r>)`,
  or a rocket adapter that names something other than its request.

## Not covered

- An `.adopt::<U>().supplies::<T>()` declares that the `adopt` entry inserts `T` into the request's
  extensions, which an `adopt` never does, and is accepted. Refusing a `supplies` after an `adopt`
  entry would follow the stray-`supplies` rule; it is not in the response.
- The axum adapter naming `Parts`, actix's adapter calling `respond` with its `HttpRequest`, and
  rocket's, which waits on entry 36 (race 2b).
- The transports DESIGN's `Embedded::supplies`, `Handle`, `Service` and `respond`: the design
  document is not this build's to edit.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass;
  the warning list on stable is identical to the one taken before the change, the 17 in
  `crates/ulo/src`, and on 1.88 no warning points outside `crates/ulo/src`. `cargo test
  --workspace` passes. `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo-http --no-deps` passes.
- Scratch crate `embed`: the 37 checks of the second round, less the one on `Embedded::supplies`
  and with two amended (the `Host<T>` refusal now names the route; the stray refusal must name
  `Embedded::forward`), plus 9 new ones, 45 in all, pass with identical output on rustc 1.98.1 and
  1.88 under `#![deny(warnings)]`. A third host, `FakeHost`, declares
  `type HostRequest = FakeRequest` (a struct carrying an optional user) and
  `host_extensions: false`, and is driven through `Service::respond`; `TestHost` and `NoExt` name
  `Parts` and are driven through the tower impl.

| Check | Result |
| --- | --- |
| `.apply_for::<ScopedInsert>(["/host"]).supplies::<User>()` on `NoExt` | refused: "`Api::maybe` on `GET /maybe` reads `Host<embed::User>` .. follows a scoped entry whose scope does not cover `/maybe`, so it does not reach this route"; `Api::host` not named |
| the same scoped to `["/host", "/maybe"]` | listens; `Host<User>` answers "dave", the scoped entry's value |
| `.adopt::<User>().apply_value(InsertUser).supplies::<User>()` on `NoExt` | refused: "`adopt::<embed::User>()` at src/main.rs:242:33 .. the `.supplies::<embed::User>()` at src/main.rs:242:73 follows an entry that runs after this `adopt`, which then finds nothing; write the `adopt` after it"; no `Host<User>` refusal |
| `.apply_value(InsertUser).supplies::<User>().adopt::<User>()` (second round) | listens; "bob" and 200 |
| `.apply_value(InsertUser).exclude(["/host"]).supplies::<User>()` on `NoExt` | listens (entry 30) |
| `Embedded::<FakeHost>::new().forward(\|req: &FakeRequest\| req.user.map(User))` with `AppModule` | listens |
| `respond(&FakeRequest { user: Some("carol") }, ..)` on `/host`, `/guarded` | "carol"; 200, the adopted `Ext<User>` admitting |
| the same with `user: None` | 500 (`HostMissing`); 403 |
| a copy that panics | 500, the panic not reaching the caller |
| `.supplies::<User>()` with no entry before it | refused: ".. and a value the host keeps is copied in by the embedding with `Embedded::forward`" |

- Against a known violation: three runs, each with mutations applied to the restored sources, the
  crate still compiling. Run 1: scoped supplies counted for every route, the `adopt` position test
  answering `true`, the copies not run, and the stray text naming `Embedded::supplies`; 5 checks
  fail: the stray refusal, the scoped refusal, the one reading the scoped refusal for `Api::host`
  (no refusal is produced), the late-`adopt` refusal, and the copy reaching `Host<User>`. Run 2:
  the scope test answering `false`, the `adopt` position test answering `false`, and `forward`'s
  types left out of the check; 5 checks fail: the scope covering both routes, `Api::host` accepted
  inside its scope, the supply before the `adopt`, the exclusion check (through its `adopt`), and
  `forward` lifting the refusals (the three `respond` checks after it do not run). Run 3: the copy
  called outside `catch_unwind`; the run exits 101 at the copy's panic, 38 checks passed before
  it. The `None` check asserts an absence and passes under every mutation; the exclusion check
  pins entry 30, and no mutation consults the exclusion. The sources were restored by `cp` from copies taken before the mutations, compared
  byte for byte, and the clean run repeated.
- The rocket probe of entry 36 is at the scratchpad's `rocket_probe/` (`main_plain.rs` fails,
  `main_gat.rs` compiles).

# Fourth round: the ninth response

The changes the ninth response (signed off 2026-10-04) asks of this build: `type HostRequest<'r>;`
with `forward`'s copy bound over every `'r`, the cost of entry 30 stated in the `supplies` docs
(item 1), and a `supplies` written after an `adopt` refused (the open question). The adapter
aliases of item 3 arrive with the adapters. Entries continue the numbering above.

Files changed: `crates/ulo-http/src/{embed.rs, pre_dispatch.rs}`. `server.rs` is unchanged: both
servers build the stage through `prepare_app`, whose `Stage::build` raises the new refusal.
`extract/host.rs` is unchanged: `Host<T>`'s doc names no host request type.

## The signature, changed

```rust
// ulo_http::embed
pub trait Embed: Send + Sync + 'static {
    const NAME: &'static str;
    type HostRequest<'r>;                          // was `type HostRequest;`
    fn limits() -> EmbedLimits;
}
impl<A: Embed> Embedded<A> {
    pub fn forward<T: Clone + Send + Sync + 'static>(
        self,
        copy: impl for<'r> Fn(&A::HostRequest<'r>) -> Option<T> + Send + Sync + 'static,
    ) -> Self;
}
impl<A: Embed> Service<A> {
    pub fn respond(&self, host: &A::HostRequest<'_>, req: Request) -> impl Future<Output = Response> + Send + use<A>;
}
impl<A, B> tower::Service<http::Request<B>> for Service<A>
where A: for<'r> Embed<HostRequest<'r> = http::request::Parts>, ..;
```

## Decisions

### 37. The tower impl's bound is `A: for<'r> Embed<HostRequest<'r> = http::request::Parts>`

- **Written:** the higher-ranked projection bound above, on the impl's `where` clause.
- **Found:** it compiles on rustc 1.98.1 and 1.88, and normalizes for a host outside the crate:
  the scratch crate's `TestHost` and `NoExt` write `type HostRequest<'r> = http::request::Parts;`
  and are driven through `ServiceExt::oneshot`, and a helper bounded the same way accepts them.
  `Service<FakeHost>`, whose request is a struct of its own, has no `oneshot` (E0599 on both
  toolchains).

### 38. `respond`'s future captures neither lifetime of the host's request

- **Written:** `host: &A::HostRequest<'_>`, both lifetimes elided, and the return type keeps
  `use<A>`. The copies run before `respond` returns, so nothing in the future borrows `host`.
- **Consequence:** a rocket handler passes its `&'r Request<'_>` unchanged, and the answer can be
  awaited after the host's request is gone: the scratch crate drops a borrowed request and the
  `String` it borrows from before awaiting the answer, which then carries the copied value.

### 39. A `supplies` after an `adopt` is kept off the entry

- **Written:** `PreDispatch::supplies` records the call site in `PreDispatch::adopt_supplies` when
  the last entry is an `adopt`, and pushes nothing onto that entry. `Stage::build` reports each:
  "`supplies` at src/main.rs:259:92 follows an `adopt` entry: `adopt` copies a value out of the
  request and inserts none; declare the `supplies` after the entry that inserts it". The refusal
  holds on a backend and on every embedding, whatever its `host_extensions`, and whatever the two
  types: `.adopt::<U>().supplies::<T>()` is refused for any `T`.
- **Why off the entry:** the declaration supplies nothing, and the `host_extensions: false` check
  then counts only what does.
- **Consequence:** under `host_extensions: false`, an `adopt::<T>()` that only such a declaration
  would have reached reports "nothing supplies it" beside the refusal. The `Host<T>` reads go
  unchecked until the refusal is fixed, by entry 31, since the stage failed.
- **Sign-off needed:** kept off the entry, or counted on it, which reports the refusal alone.

### 40. The `supplies` docs name two placements for a supplying entry

- **Written:** the paragraph on entry 30 states that an unscoped entry's `exclude` is not
  consulted, that a supply after an excluding entry therefore lifts the refusal for the routes it
  excludes, which then answer 500 (`HostMissing`) per request, and the advice: put the supplying
  entry where its exclusion matches the routes that read the value, "an `exclude` covering none of
  them, or a scoped entry, whose exclusions are applied to routes when the server prepares".
- **Why the second placement:** a scoped entry's exclusions are applied per route in
  `Stage::scoped_for`, and the check reads a scoped supply only on the routes its stage holds. It
  reaches no `adopt`, which the paragraph before it states.
- **Sign-off needed:** the scoped placement kept, or the advice cut to the response's sentence.

## Not covered

- The adapter aliases (`ulo_http_axum::Handle`, `ulo_http_axum::Service`), with the adapters.
- The transports DESIGN's `Embed`, `forward` and `respond` signatures: the design document is not
  this build's to edit.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning list, taken as file, line and message from cargo's JSON output, is identical before
  and after the change on both toolchains: the 17 in `crates/ulo/src`. The comparison reports a
  planted `fn probe_unused` in `pre_dispatch.rs` as one added line; the file was restored by `cp`
  and compared byte for byte. `cargo test --workspace` and `RUSTDOCFLAGS="-D warnings" cargo doc
  -p ulo-http --no-deps` exit 0.
- Scratch crate `embed`: the 45 checks of the third round, plus 4 new ones, 49 in all, pass with
  identical output on rustc 1.98.1 and 1.88 under `#![deny(warnings)]`. `TestHost`, `NoExt` and
  `FakeHost` name `HostRequest<'r>`; a fourth host, `BorrowHost`, declares
  `type HostRequest<'r> = BorrowedRequest<'r>`, a struct holding `Option<&'r str>`.

| Check | Result |
| --- | --- |
| `Embedded::<BorrowHost>::new().forward(\|req: &BorrowedRequest<'_>\| req.name.map(\|n\| Name(n.to_owned())))`, `respond` on a request borrowing a local `String`, both dropped before the answer is awaited | 200, "erin" |
| `.apply_value(InsertUser).supplies::<User>().adopt::<User>().supplies::<User>()` on `TestHost` | refused, naming src/main.rs:259:92 and not the first `supplies` at 259:55 |
| the same on `NoExt` | the same refusal alone: the `supplies` before the `adopt` still counts for it |
| the same on the hyper backend | the same refusal |

- The borrowed check is shown to need the lifetime: with `type HostRequest<'r> =
  BorrowedRequest<'static>` the crate fails to compile, E0597 (`owned` does not live long enough)
  and E0505.
- Against a known violation, two runs, each a mutation of the restored `pre_dispatch.rs`. Run 1:
  the `adopt` arm in `supplies` removed, so the declaration goes onto the `adopt` entry; the 3
  refusal checks fail, the other 46 pass. Run 2: the arm kept and the loop reporting
  `adopt_supplies` iterating nothing; the same 3 fail. The source was restored by `cp` from a copy
  taken before the mutations and compared byte for byte.
- The rocket probe at the scratchpad's `rocket_probe/` (`main_real.rs`, rocket 0.5.1, edition
  2024) depends on `ulo`, `ulo-http` and `ulo-tokio` by path and implements the real
  `ulo_http::embed::Embed` with `type HostRequest<'r> = rocket::Request<'r>` and
  `host_extensions: false`. Its rocket `Handler` calls `Service::<RocketHost>::respond(&service,
  req, app_req)` with the `&'r Request<'_>` it receives; the app has `Embedded::forward` copying the
  `x-user` header into a `User` that a handler reads as `Host<User>`, and it listens. Driven through
  rocket's local client, `GET /who` with `x-user: frank` answers 200 "frank" and without the header
  500, on rustc 1.98.1 and 1.88. The copy written without a parameter annotation,
  `.forward(|req| req.headers().get_one("x-user") ..)`, compiles on both (`main_unannotated.rs`).
  The round-three files (`main_plain.rs`, `main_gat.rs`, `Cargo.toml.round3`) are kept beside it.

# Fifth round: the tenth response

The change the tenth response (signed off 2026-10-04, section "The refusal text") asks of this
build: the embedding refusals print type names through the formatter the core's wiring reports
use, cut to the last path segment, with full paths only where two different types in one report
would print alike. Entries continue the numbering above.

Files changed: `crates/ulo/src/{key.rs, error/wiring.rs}`, `crates/ulo-http/src/{embed.rs,
server.rs, router/mod.rs, pre_dispatch.rs, __private.rs, extract/host.rs, extract/mod.rs,
extract/path.rs}`, and one doc comment in `crates/ulo/src/transport/mod.rs` (entry 46).

## The signature, added

```rust
// ulo
impl Key {
    /// The keys among `keys` whose display another, different key among them shares.
    pub fn colliding(keys: impl IntoIterator<Item = Key>) -> HashSet<Key>;   // new
}

// ulo_http::__private (doc-hidden)
impl PathCheck {
    pub fn of<T: DeserializeOwned + 'static>() -> PathCheck;                  // was `T: DeserializeOwned`
}
```

## Decisions

### 41. The formatter is `Key`'s `Display`; the one new core item is `Key::colliding`

- **Found:** the core's formatter is `key::short_type_name`, `pub(crate)`. It reaches a caller
  outside the crate through `Key`'s public `Display` (short) and `{:#}` (full paths), which apply
  it after `role_spelling`. The collision rule is `error::wiring::colliding`, private, over
  `KeyName`s.
- **Written:** `ulo-http` names every type in a report as `Key::of::<T, ()>()` and writes it with
  `{}` or `{:#}`. The rule moved into a public, documented `Key::colliding(keys) -> HashSet<Key>`,
  and the wiring report's private `colliding` now delegates to it, so the two reports share one
  implementation. `Key`'s type doc states that a transport's startup report names a type through
  a `Key`, bound or not.
- **Not chosen:** `ulo::__private`, whose module doc limits it to code `ulo-macros` generates; a
  public free `short_type_name(&str)`, which would add a second public spelling of what `Key`'s
  `Display` already does and leave the collision rule duplicated in each transport.
- **Sign-off needed:** `Key` naming a type that is never bound (`Host<T>`'s `T`, `Path<T>`'s
  `T`), or a dedicated public type, such as a `TypeName`, carrying the same `Display`/`{:#}` pair.

### 42. A failure naming types is written when the report is

- **Written:** `server.rs` holds `Failures` (the list both servers' `prepare` fill), `Failure`
  (`Plain(String)`, or `Naming { keys, text }` with `text: Box<dyn Fn(&Names) -> String>`) and
  `Names`, whose `of(key)` writes `{key:#}` for a key in the report's collision set and `{key}`
  otherwise. `PrepareError`'s `Display` runs `Key::colliding` over every failure's keys, then
  writes each failure. `Failures::push` takes `impl Into<Failure>`, so a plain `format!(..)` push
  is unchanged.
- **Scope of a report:** one `prepare` error, which `listen()` prints as one
  `StartupError::Configure` entry. Two transports' entries are separate reports, as each core
  wiring report is.
- **`HostType` removed:** a host value's type is a `Key` in `HostRead`, `Supply`, `Step::Adopt`
  and the `forward` copies. `Key` compares on `TypeId`, as `HostType::id` did.

### 43. Controller keys count among a report's types, and the route table's failures use them

- **Written:** a `Host<T>` refusal puts its controller's key beside `T`, and the route-table
  failures (`Router::build`, which both servers run) name each handler through a `Who { controller,
  name }` written with `Names`. Two controllers whose names print alike then print in full:
  "`embed::Api::user` and `embed::other::Api::user` both answer `GET /users/{id}`".
  `RouteError` is gone; `Router::build` returns `Vec<Failure>`.
- **Why:** the collision rule is stated over every type a report names, and a controller is one.
  Counting only the `Host<T>` types would print `a::Users` and `b::Users` alike in one report.
- **Sign-off needed:** the route-table failures sit outside the embedding refusals the response
  names; kept, or reverted to their earlier always-short controller names.

### 44. `Path<T>`'s refusal goes through the same formatter

- **Written:** `PathCheck` holds a `Key` in place of a `type_name`, and "`Path<T>` does not fit
  the route" writes it with `Names`. `PathCheck::of` and the `ViaPath` probe impl gain
  `T: 'static`, which `Path<T>: FromCall<Http>` already requires, so no handler that compiled
  before is refused.

### 45. What prints a type and is left as it was

- The `peer_addr` refusal prints `InputReader::path()`, strings the core renders short before the
  report exists, which core DESIGN §10.1 keeps short as steps of a dependency path, and the literal
  `ClientAddr`. No change.
- The `error` log of a `forward` copy that panics writes `type=embed::User`, the full path, now
  through `{:#}`: a log line stands alone, with no report to collide within.
- `ExtractError::HostMissing` carries `type_name::<T>()` at request time; its message is withheld
  from the 500 and is not a startup report.
- `Stage::build`'s failures name no type.

### 46. A rustdoc failure in `ulo` fixed

- **Found:** `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo --no-deps` fails at HEAD (`293d7d34`):
  `Transport::KEY`'s doc splits the code span `` `const __ULO_KEY_<name>: &'static str = <Tr as
  Transport>::KEY;` `` across two lines, and rustdoc reports `<name>` and `<Tr` as unclosed HTML
  tags. The fourth round documented `ulo-http` alone and did not reach it.
- **Written:** the span on one line, the paragraph reflowed; no word changed.

## Not covered

- The aliases and adapter crates, with race 2b, as before.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning list, taken as file, line and message from cargo's JSON output, is identical to the
  one at HEAD (`293d7d34`, extracted by `git archive` into the scratchpad) on both toolchains: the
  17 in `crates/ulo/src`. The comparison reports a planted `fn probe_unused` in `server.rs` as one
  added line; the file was restored by `cp` and compared byte for byte. `cargo test --workspace`
  exits 0. `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo-http -p ulo --no-deps` exits 0 after
  entry 46, and exits 101 at HEAD.
- Scratch crate `embed` (crate name `embed`, so a full path reads `embed::User`): the 49 checks of
  the fourth round, with the refusal-text ones now matching `Host<User>`, `adopt::<User>()`,
  `Embedded::forward::<User>(..)` and `.supplies::<User>()`, the first also asserting no `embed::`
  in its report, plus 2 new ones, 51 in all. They pass under `#![deny(warnings)]` on rustc 1.98.1
  and 1.88 with identical verdicts and identical refusal texts.

| Check | Result |
| --- | --- |
| `Twins` reading `Host<User>`, `Host<other::User>` and `Host<Name>` on `NoExt`, nothing supplied | refused: `Host<embed::User>`, `Host<embed::other::User>`, `Embedded::forward::<embed::other::User>(..)`; `Host<Name>` and `Twins::mine` stay short |
| `Api` and `other::Api` both answering `GET /users/{id}` on `TestHost` | refused: "`embed::Api::user` and `embed::other::Api::user` both answer `GET /users/{id}`" |

- Against a known violation, two runs, each a mutation of `Names::of` in the restored
  `server.rs`. Run 1, the formatter bypassed (always `{key:#}`): the 4 short-name refusal checks
  and the collision check's `Host<Name>` assertion fail, 5 in all, the other 46 pass. Run 2, the
  collision set ignored (always `{key}`): the 2 collision checks fail, the other 49 pass. The
  source was restored by `cp` from a copy taken before the mutations and compared byte for byte,
  and a rerun passes 51.

# Sixth round: the eleventh response

The change the eleventh response (signed off 2026-10-04) asks of this build: a public
`ulo::TypeName` carrying the shortening and the collision rule, `Key::colliding` removed, the
wiring reports and `ulo-http` calling `TypeName::colliding`, `ulo-http` holding a `TypeName` where
it held a `Key` only to print a type, and the startup report of `StartupError::Configure`
colliding names across every transport's failures. Entries continue the numbering above.

Files changed: `crates/ulo/src/{type_name.rs (new), key.rs, lib.rs, redact.rs, app/mod.rs,
error/configure.rs, error/mod.rs, error/wiring.rs, graph/mod.rs, graph/scopes.rs,
graph/visibility.rs, graph/wire.rs, hooks.rs, module/mod.rs}`, `crates/ulo-http/src/{server.rs,
embed.rs, pre_dispatch.rs, __private.rs, router/mod.rs, extract/path.rs}`. `extract/host.rs` and
`extract/mod.rs` are unchanged: neither holds a `Key`.

## The signature, changed

```rust
// ulo
pub struct TypeName { .. }                       // new; Clone, Copy, Eq and Hash on the TypeId
impl TypeName {
    pub fn of<T: ?Sized + 'static>() -> TypeName;
    pub fn colliding(names: impl IntoIterator<Item = TypeName>) -> HashSet<TypeName>;
}
impl fmt::Display for TypeName { .. }            // `User`; `{:#}` writes `my_app::User`
impl fmt::Debug for TypeName { .. }              // as `Display`

impl Key {
    pub fn type_name(&self) -> TypeName;         // new
    // `pub fn colliding(..)` removed
}

pub struct PrepareFailure { .. }                 // new; Display, Debug, Error
impl PrepareFailure {
    pub fn new(
        names: impl IntoIterator<Item = TypeName>,
        text: impl Fn(&HashSet<TypeName>) -> String + Send + Sync + 'static,
    ) -> Self;
    pub fn names(&self) -> &[TypeName];
    pub fn text(&self, full: &HashSet<TypeName>) -> String;
}
```

## Decisions

### 47. `TypeName` compares on its `TypeId`, and `Key` is two of them

- **Written:** `TypeName` holds the `TypeId` and the `type_name` string, and compares and hashes on
  the `TypeId`, as `Key` did and as `ulo-http`'s `HostType` did before entry 42. `Display` is the
  core's shortener; `{:#}` writes the string whole. `Key`'s fields are now a `TypeName` for the
  type and one for the qualifier, and its `{}` and `{:#}` write each through them, role spelling
  (`AnyGuard<Http>`) applied on top as before.
- **The shortener's home:** `type_name.rs`, beside `key.rs`, holding `short_type_name` and its two
  helpers, `pub(crate)`. The reports that hold a bare `type_name` string with no `TypeId`
  (transport names, metadata names, hook refusals, module names) still call it directly.
- **`Key::type_name()` made public:** `ulo-http` names a handler's controller from the `KeyName`
  a `MountedHandler` carries, and needs that key's type as a `TypeName` to enter it into the
  report's collision pass. It replaces a `pub(crate)` accessor of the same name that answered the
  string; the core's own callers now write `.type_name().full()`.
- **A stray line removed:** `key.rs` carried a doc fragment, "The diagnostic spelling of a
  `type_name`: module paths stripped from every segment, so", attached to the top of
  `role_spelling`'s doc. It is gone with the move; `short_type_name`'s doc in `type_name.rs` says
  what it does.
- **Sign-off needed:** `TypeName` carrying the `TypeId` (equality by type, not by string), and the
  public `Key::type_name()`.

### 48. The wiring report collides a key's type and its qualifier separately

- **Written:** the report runs `TypeName::colliding` over every key's type and, when qualified,
  its qualifier, across all its entries, and writes each part of a key full or short on its own
  (`Key::text_in`, `KeyName::text_in`, `pub(crate)`). Before, it collided whole-key texts.
- **Consequence:** two outputs change. `Store @ a::Replica` beside `Store @ b::Replica` prints
  `Store @ embed::cfg_a::Replica` and `Store @ embed::cfg_b::Replica`, where it printed
  `embed::Store @ embed::cfg_a::Replica`: `Store` names one type. `a::Config` beside
  `b::Config @ Q` now prints both `Config`s full, where both printed short, since the whole-key
  texts `Config` and `Config @ Q` differed. A key's collection kind still plays no part.
- **Why:** `TypeName::colliding` is the one entry point the response names, and it takes types,
  so the rule the report applies becomes the one stated for every report: two different types
  printing alike print in full.
- **Sign-off needed:** the per-type granularity, or a `Key`-level pass kept inside the core that
  reads `TypeName::colliding`'s answer back onto whole keys.

### 49. The seam is a core error type, `PrepareFailure`

- **Written:** a server whose `prepare` failure names types answers a boxed `PrepareFailure`: the
  names, and a closure writing the text given the set of names to write in full.
  `ConfigureErrors::collect` (`pub(crate)`, called by `listen()`) downcasts each server's error,
  runs `TypeName::colliding` over the names of every `PrepareFailure` among them, and writes each
  against that one set. Any other error keeps its own text and contributes no names. A
  `PrepareFailure` displayed alone collides its own names.
- **`ulo-http`:** `PrepareError` is gone. Both servers' `prepare` answer
  `Failures::into_error()`, a `PrepareFailure` naming every type its failures name, whose text is
  the format `PrepareError` wrote: one failure alone, or "N problems:" and a bullet each.
  `ulo-http`'s `Names` stays, borrowing the set the closure receives, and `Names::of` takes a
  `TypeName`.
- **Not chosen:** a method on an error trait. `prepare` answers a `BoxError`, and a
  `Box<dyn Error>` downcasts only to a concrete type, so the core would need a concrete wrapper
  around such a trait object anyway, or a change to `Server::prepare`'s return type. A core
  `Names` type handed to the closure: the closure receives the `HashSet` that
  `TypeName::colliding` answers, and the one-line `contains` test each name needs is left to it
  rather than given a third public item.
- **Sign-off needed:** the name `PrepareFailure` and its closure signature, and leaving a
  plain-error failure out of the pass.

### 50. The cross-report pass runs when the report is built, not in `Display`

- **Written:** `ConfigureErrors::collect` writes every entry's text against the whole report's
  set, then redacts it (`redact::redact_as`, new, `pub(crate)`: the scrub applied to a text the
  core wrote, the original error kept for `downcast_ref`). `ConfigureErrors`'s `Display` writes
  the stored entries.
- **Why not in `Display`:** an entry's text is redacted against the graph's secrets when it is
  stored, and `Display` has no registry. Writing it there would mean keeping the secrets inside
  the returned error, or redacting a template with placeholders for the names. The report the
  user reads is the same either way, since `listen()` holds every server's failure before it
  builds `ConfigureErrors`.
- **Consequence:** each `ConfigureError::source` prints the cross-report text, the same as its
  entry in the report. `source.downcast_ref::<PrepareFailure>()` reaches the original, whose own
  `Display` collides its names alone, as a `WiringError` displayed alone does. The
  `pub(crate)` constructors `ConfigureErrors::new` and `ConfigureError::new` are gone.
- **Sign-off needed:** build time, or `Display` with the secrets held in the error.

### 51. `ulo-http` holds a `TypeName` wherever it held a `Key`

- **Written:** `Supply::ty`, `Step::Adopt`'s type, the `forward` copies' type, `HostRead::ty`,
  `PathCheck::ty` and the route table's `Who::controller`. `ulo-http` no longer imports `Key`.
  Each host value type was printed and compared by `TypeId`, never looked up as a binding.
- **The controller:** a controller is bound, but the route-table failures print only its type, in
  `` `Type::method` ``, so `Who` and the `Host<T>` refusal hold the controller key's
  `type_name()`. A controller key's qualifier would no longer print; `ModuleDef::controller::<C>()`
  registers every controller unqualified, so no output changes.
- **The `forward` panic log:** `type=` writes the `TypeName` with `{:#}`, the same full path.
- **Sign-off needed:** the controller held as a `TypeName`, or as the `Key` with its type entered
  into the pass.

## Not covered

- Transport names. ``transport `Http` `` is `transport_name`'s string, cut by its own rule (the last
  segment, or the whole name when generic), and is not entered into the pass: two transports whose
  markers share a last segment print alike. `ConfigureError::transport` is a public
  `&'static str`, and `StartupError::Bind` and the shutdown reports name transports the same way.
- Module names. `module::colliding_names` stays: a `ModuleName` carries a label and an instance
  number beside its type.
- The aliases and adapter crates, with race 2b, as before.
- The DESIGN documents: neither names `TypeName` or `PrepareFailure` in its surface list.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning list, taken as file, line and message from cargo's JSON output, matches the one at
  HEAD (`1174cdc7`, extracted by `git archive` into the scratchpad) on both toolchains, the 17 in
  `crates/ulo/src`, but for one line: `graph/mod.rs`'s `exports is never read` moves from line 71
  to 72, its `use` of `short_type_name` now on a line of its own. That moved line is the
  comparison reporting a one-line difference. `cargo test --workspace` exits 0, and runs the new
  `PrepareFailure` doctest. `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo -p ulo-http --no-deps`
  exits 0.
- Scratch crate `embed`: the 51 checks of the fifth round, unchanged, plus 2 new ones, 53 in all,
  pass under `#![deny(warnings)]` on rustc 1.98.1 and 1.88 with identical verdicts and texts. The
  new imports sit at the bottom of `main.rs`, so the source locations the refusal checks assert
  stay those of round five.
- The second report source: `ulo-http`'s `Http` is the workspace's only transport, and a wiring
  failure cannot share a report with a `prepare` failure, since `wire()` returns
  `StartupError::Wiring` before `listen()` prepares anything. The scratch crate declares a second
  transport, `Queue` (`Cx = ()`, `Reply = ()`), and a `QueueServer` whose `prepare` answers a
  `PrepareFailure` naming `other::User` and `Ticket`; the app binds it beside an
  `Embedded::<NoExt>`.

| Check | Result |
| --- | --- |
| `Solo` reading `Host<User>` on `NoExt`, nothing supplied, bound with `QueueServer` | one report, 2 errors: `Host<embed::User>`, `Embedded::forward::<embed::User>(..)` in the `Http` entry, `embed::other::User` in the `Queue` entry; `Solo` and `Ticket` stay short |
| `NeedsConfigs` reading `Config` from `cfg_a` and `cfg_b`, `Store` under `cfg_a::Replica` and `cfg_b::Replica`, and `Ticket`, none bound | wiring refused: `embed::cfg_a::Config`, `embed::cfg_b::Config`, `Store @ embed::cfg_a::Replica`, `Store @ embed::cfg_b::Replica`, `Ticket` |

- Against a known violation, two runs, each a mutation of a restored source. Run 1, the
  cross-report pass skipped (`ConfigureErrors::collect` writing each `PrepareFailure` against its
  own names): the cross-report check fails, `Host<User>` in the `Http` entry, and the other 52
  pass, the single-transport collision checks among them. Run 2, the wiring pass counting a key's
  type and not its qualifier: the wiring check fails, `Replica` short, and the other 52 pass. Each
  source was restored by `cp` from a copy taken before the mutations and compared byte for byte,
  and a rerun passes 53.

# Seventh round: the twelfth response

The change the twelfth response (signed off 2026-10-04) asks of this build: the core's
`PrepareFailure` renamed `PrepareError`, and every transport name the core stores for a report
held as a `TypeName` of the transport's marker type and entered into the collision pass of the
report that prints it. Entries continue the numbering above.

Files changed: `crates/ulo/src/{type_name.rs, lib.rs, redact.rs, error/configure.rs, error/mod.rs,
error/wiring.rs, transport/mod.rs, transport/controller.rs, transport/server.rs, binding/alias.rs,
module/def.rs, graph/mod.rs, graph/wire.rs, graph/scopes.rs}`, `crates/ulo-http/src/server.rs`.
`app/mod.rs` and `lifecycle/shutdown.rs` are unchanged: they pass `ErasedServer::transport_name()`
through, and its type changed under them.

## The signature, changed

```rust
// ulo
pub struct PrepareError { .. }                   // was `PrepareFailure`; same API

pub struct ConfigureError { pub transport: TypeName, pub source: Redacted, .. }  // was `&'static str`
pub enum StartupError { .., Bind { transport: TypeName, source: Redacted } }
pub struct TimerMissing { pub transport: TypeName }
pub enum ShutdownFailure { .., Close { transport: TypeName, reason: FailureReason } }
pub enum WiringError {
    ..,
    InputNotSeeded { handler: String, transport: TypeName, input: KeyName, seeder: TypeName, path: Vec<String> },
    ..
}
pub enum InputOrigin {
    Transport { name: TypeName, at: &'static Location<'static> },
    Module { module: ModuleName, seeders: Vec<TypeName>, at: &'static Location<'static> },
    Binding { .. },                              // unchanged
}
```

## Decisions

### 52. `PrepareFailure` is `PrepareError`

- **Written:** the type, its export, every doc link to it in `ulo` (`type_name.rs`, `redact.rs`,
  `error/configure.rs`) and `ulo-http`'s `Failures::into_error`. The doctest's binding and
  `ConfigureErrors::collect`'s closure argument, both `failure`, are now `error` and `prepared`.
- **Not touched:** the earlier rounds of this log, which record the name as built then.

### 53. A transport name is a `TypeName` wherever the core stores one for a report

- **Written:** the three fields the response names, and four more the search found:
  `TimerMissing::transport`, which `StartupError::Bind` carries in its `source`;
  `WiringError::InputNotSeeded`'s `transport` and `seeder`; `InputOrigin::Transport::name`; and
  `InputOrigin::Module::seeders`, each a `seeded_by::<Tr>()` transport.
- **Inside the core:** `HandlerDecl` held a `TypeId` and a `&'static str` for its transport, and
  `InputRecord` and `InputDecl` the same pair for a seeder. Each pair is now one `TypeName`, which
  compares on the `TypeId` the pair compared on. `ErasedServer::transport_name` answers a
  `TypeName`. `transport_name::<T>()` is gone.
- **Left as text:** `HandlerInfo::transport()` and `BoundAddr::transport`, both `Transport::KEY`,
  a lookup key, and `HandlerDecl::key`. Strings the core renders ahead of the report keep the
  short form, as other pre-rendered text does: the path steps `UsersController::get (Http)`,
  `ClosureScopeViolation::closure`, and the signal text ``transport `Http` failed: ..`` that ends
  `serve` when a server's `serve` fails.
- **Consequence, a generic marker:** `transport_name` kept a generic marker's full name
  (DIVERGENCES.md, F5). `Bind`, `Configure`, `Close` and `InputNotSeeded` applied
  `short_type_name` to it before printing and wrote `Q<X>`. `TimerMissing`, the
  `InputOrigin::Transport` line, the pre-rendered strings and the signal wrote it whole,
  `a::Q<b::X>`. All of them now write `TypeName`'s short form, `Q<X>`, and the full path on a
  collision. A non-generic marker prints as before everywhere.
- **Sign-off needed:** the four fields beyond the three named, the seeder counted as a transport
  name, and F5's generic-marker exception dropped.

### 54. The startup report enters each failing transport into its pass

- **Written:** `ConfigureErrors::collect` runs `TypeName::colliding` over every failing transport
  together with every `PrepareError`'s names. A transport whose short form another transport
  shares, or a type a `PrepareError` names, prints in full; `PrepareError` texts are written
  against the same set, so the transport `a::Http` beside a failure naming `b::Http` prints both
  in full.
- **The flag:** `ConfigureError` gains a private `transport_full: bool`, set when the report is
  built, and `Display` follows it, so an entry displayed alone prints its transport as the report
  does. This matches entry 50, where `source` carries the report's text.
- **A plain error:** a server answering an error that is not a `PrepareError` contributes no names
  from its text, but its transport enters the pass.
- **Sign-off needed:** the private flag, or the written transport text stored in its place; and a
  plain error's transport entering the pass.

### 55. The shutdown report collides every name it prints

- **Why it qualifies:** a shutdown report prints one transport per failed `close` and one key per
  failed hook, so two names can print alike in it.
- **Written:** `ShutdownError`'s `Display` runs `TypeName::colliding` over every failure's names,
  each hook key's type and qualifier and each transport, and writes each failure against that set.
  `ShutdownFailure` displayed alone collides its own names, as `WiringError` does.
- **Why in `Display`:** `ShutdownError` holds no text written ahead of time. Its reasons are
  redacted when recorded, and the names carry no secret, so the pass runs where the report is
  printed, unlike entry 50's build-time pass.
- **Consequence:** hook keys join the pass with the transports. Before, a key always printed short
  in this report, so `a::Pool` and `b::Pool` failing their destroy hooks in one shutdown printed
  alike. Now both print in full.
- **Sign-off needed:** the keys included in the pass along with the transports.

### 56. The bind error prints one name and runs no pass

- **Written:** `StartupError::Bind` writes its transport's short form. The error names one
  transport. A `TimerMissing` source names that same transport, and a type cannot collide with
  itself. A transport's own error is opaque text. With one name, there is nothing to
  collide.
- **Sign-off needed:** no pass on `Bind`.

### 57. The wiring report enters the transports it names into its pass

- **Written:** `WiringError::type_names` replaces the `colliding` helper over keys: each key's type
  and qualifier, `InputNotSeeded`'s transport and seeder, and the transports of `InputConflict`'s
  two origins. `WiringErrors` and a `WiringError` displayed alone collide them with the keys'
  types. `InputOrigin` displayed alone collides its own names; `{:#}` still governs only the
  module name.
- **The match:** `type_names` names the two variants that hold a transport and gives every other
  variant no transports through a wildcard arm, where `key_names` and `module_names` list every
  variant.
- **Sign-off needed:** the wildcard arm, or an exhaustive list kept in step with `key_names`.

## Not covered

- An `InputNotSeeded` whose path opens at the handler prints the route line from the path, so
  under a collision its headline writes the transport in full and the route line writes it short.
  The path is rendered at wiring time, before the report's set exists.
- The scratch crate checks the startup and shutdown reports. Its transports declare no inputs, so
  `InputNotSeeded` and `InputConflict` under a transport collision are untested.
- `DESIGN.md` §10.2 still shows `&'static str` for `Bind`, `TimerMissing`, `Close`,
  `InputOrigin::Transport` and `InputOrigin::Module`'s seeders, and neither DESIGN document lists
  `PrepareError`. `DIVERGENCES.md` F5 records the generic-marker exception entry 53 drops.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning list, taken as file, line and message from cargo's JSON output, matches the one at
  HEAD (`b463566c`, extracted by `git archive` into the scratchpad) on both toolchains, the 17 in
  `crates/ulo/src`, except one line: `module/def.rs`'s `location is never read` moves from line 537
  to 538, below the added `use` of `TypeName`. `cargo test --workspace` exits 0, and runs the
  `PrepareError` doctest. `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo -p ulo-http --no-deps`
  exits 0.
- Scratch crate `embed`: the 53 checks of the sixth round, renamed to `PrepareError`, print the
  same lines as in round six. Two new checks bring the total to 55, all passing under
  `#![deny(warnings)]` on rustc 1.98.1 and 1.88 with identical verdicts and texts. The new
  transports, `Faulty<T>` server and `EmptyModule` sit below `main`. The new checks sit in `main`
  after round six's, so the source locations the refusal checks assert are unchanged.
  `Faulty<T>` fails with a plain error, which names no type, so only the transports can make a
  name print in full.

| Check | Result |
| --- | --- |
| `SoloModule` bound with `Embedded::<NoExt>`, `Faulty<embed::Queue>` and `Faulty<embed::alt::Queue>`, each failing `prepare` | one report, 3 errors: ``transport `embed::Queue` ``, ``transport `embed::alt::Queue` ``, ``transport `Http` `` with `Host<User>`; no `ulo_http::Http` |
| `EmptyModule` bound with `Faulty<embed::Queue>`, `Faulty<embed::alt::Queue>` and `Faulty<embed::Topic>`, each failing `close` | 3 failures: ``transport `embed::Queue` failed to close``, ``transport `embed::alt::Queue` failed to close``, ``transport `Topic` failed to close``; no `embed::Topic` |

- Against a known violation, two runs, each a mutation of a source restored afterwards. Run 1, the
  startup report's transports left out of the pass (`ConfigureErrors::collect` filtering them out
  before `TypeName::colliding`): the startup check fails, both queues printing ``transport `Queue` ``,
  and the other 54 pass. Run 2, `ShutdownFailure::Close` contributing no names: the shutdown check
  fails, both queues printing ``transport `Queue` failed to close``, and the other 54 pass. Each
  source was restored by `cp` from a copy taken before the mutations and compared byte for byte,
  and a rerun passes 55.

# Eighth round: the thirteenth response

The change the thirteenth response (signed off 2026-10-04) asks of this build:
`WiringError::type_names` lists every variant, and `InputNotSeeded` writes the transport at the
head of its path from the `TypeName` it holds, when the report is formatted. Entries continue the
numbering above.

Files changed: `crates/ulo/src/{error/wiring.rs, graph/scopes.rs, transport/server.rs}`.

## The signature, changed

```rust
// ulo
pub enum WiringError {
    ..,
    /// `path`: the steps after the handler; the handler step is written from `handler` and `transport`
    InputNotSeeded { handler: String, transport: TypeName, input: KeyName, seeder: TypeName, path: Vec<String> },
    ..
}
```

The field types are unchanged. `path` no longer opens with the handler step `AltApi::call (Rpc)`.

## Decisions

### 58. `type_names` lists every variant

- **Written:** the two variants that hold a transport, then the other thirty in one arm answering
  no transports, in declaration order. `key_names` and `module_names` keep their own lists.
- **Consequence:** a variant added to `WiringError` fails to compile at `type_names` until it is
  placed. With `KnobWithoutTimer` removed from the arm, `cargo check -p ulo` fails with E0004
  naming it.

### 59. `InputNotSeeded` writes its handler step when the report is formatted

- **Written:** `path` holds the steps after the handler: each binding between it and the read,
  then the injection point. The render writes `{handler} ({transport})` ahead of them, the
  transport through `TypeName::written` against the report's set, the call the headline makes.
  The match on `path.first()` that recognised a handler step written ahead is gone.
- **No new field:** the entry already holds the handler's name and the transport's `TypeName`,
  and the two make the step.
- **Inside the core:** `InputRead::path` is `InputRead::steps`, without the handler, and
  `InputWalk` drops its `head` field. `scopes::handler_step` writes the step short for the one
  caller that wants it written ahead, `Mounted::handlers_reading`, so `InputReader::path()` still
  opens with it.
- **Sign-off needed:** the public `path` field changes meaning with its type unchanged. A caller
  reading `path[0]` as the handler gets the first binding.

### 60. `InputConflict` needed no change

- **Why:** `InputOrigin::line` writes both of its lines from the `TypeName`s each origin holds,
  against the report's set. No transport in the entry is written ahead. Check (b) below shows it
  printing both in full.

### 61. The scratch transport

- **Written:** `Rpc` declares `CallerId` in `inputs()`, and `alt::Rpc` declares none. Each
  controller is an `#[injectable]` with a hand-written `impl Controller` mounting one
  `HandlerSpec`. `RpcApi` reads nothing; its handler mounts `Rpc`, which declares `Rpc`'s
  inputs. `AltApi` and `TopicApi` read `Dep<CallerId>` as a field, so each is an
  execution-scoped controller whose handler, of `alt::Rpc` or `Topic`, reaches the input.
- **Check (b)'s second source:** a module's `m.input::<CallerId>().seeded_by::<alt::Rpc>()`,
  which keeps `alt::Rpc` the one second marker for both collision checks. A third marker
  declaring `CallerId` in its own `inputs()` would test two `Transport` lines in place of a
  `Transport` and a `Module` line.
- **Sign-off needed:** the module as check (b)'s second source.

## Not covered

- `InputReader::path()` opens with the handler step written short, and `ulo-http`'s `ClientAddr`
  refusal joins it into a plain failure text, which names no type. In a startup report whose pass
  prints the HTTP transport in full, the entry's heading writes ``transport `a::Http` `` and the
  path in its text writes `(Http)`, the inconsistency entry 59 removes from the wiring report.
- `ClosureScopeViolation::closure` writes its handler's transport short inside the description,
  `method-level guard #2 of UsersController::get (Http)`, and that transport enters no pass.

## Verification

- `cargo check --workspace --all-targets` and `cargo +1.88 check --workspace --all-targets` pass.
  The warning list, taken as file, line and message from cargo's JSON output, matches the one at
  HEAD (`fcabd78a`, taken from the clean tree before the edits) on both toolchains: the 17 in
  `crates/ulo/src`, no line moved. `cargo test --workspace` exits 0, the `PrepareError` doctest
  among its tests. `RUSTDOCFLAGS="-D warnings" cargo doc -p ulo -p ulo-http --no-deps` exits 0.
- Scratch crate `embed`: the 55 checks of the seventh round print the same lines as in round
  seven. Three new checks bring the total to 58, all passing under `#![deny(warnings)]` on rustc
  1.98.1 and 1.88 with identical verdicts and texts. The checks sit in `main` after round seven's,
  and the new types below the existing ones, so the source locations the refusal checks assert are
  unchanged. Each new check reads the report `wire()` returns.

| Check | Result |
| --- | --- |
| (a) `UnseededModule`: `RpcApi` and `AltApi` | ``input `CallerId` is seeded by embed::Rpc, read on a path from the embed::alt::Rpc handler AltApi::call``, the path ``AltApi::call (embed::alt::Rpc) → AltApi (execution) → Dep<CallerId> (field `id`)``; no `(Rpc)`, no `embed::AltApi`, no `embed::CallerId` |
| (b) `ConflictModule`: `RpcApi`, and `CallerId` seeded by `alt::Rpc` | ``declared by transport `embed::Rpc` at ..`` and ``declared in ConflictModule with seeder `embed::alt::Rpc` at ..``; no `` `Rpc` `` |
| (c) `ShortModule`: `RpcApi` and `TopicApi` | ``input `CallerId` is seeded by Rpc, read on a path from the Topic handler TopicApi::call``, the path ``TopicApi::call (Topic) → TopicApi (execution) → Dep<CallerId> (field `id`)``; no `embed::` |

- Against a known violation, three runs on both toolchains, each a mutation of the core restored
  afterwards. Run 1, the handler step written ahead again (`check_inputs` putting
  `handler_step` at the head of `path`, the render joining `path` alone): (a) fails with the
  headline writing `embed::alt::Rpc` and the path `AltApi::call (Rpc)`, and the other 57 pass.
  Run 2, `InputNotSeeded` moved into the arm answering no transports: (a) fails, both transports
  printing `Rpc`, and the other 57 pass. Run 3, `InputConflict` moved there: (b) fails, both
  lines printing `` `Rpc` ``, and the other 57 pass. Each source was restored by `cp` from a copy
  taken before the mutations and compared byte for byte, and a rerun passes 58 on both
  toolchains.
