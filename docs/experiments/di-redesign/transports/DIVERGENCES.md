# Race 2a: divergences for sign-off

Where the code written for race 2a departs from `transports/DESIGN.md` or `DESIGN.md`, or fills
what they leave open, gathered from the eight logs under `divergences/`: `race2a-spine.md` and one
per area (CX core extensions, MX macros and handler codegen, T `ulo-transport`, N `ulo-net` and
`ulo-tokio`, H `ulo-http` requests, routing and replies, P `ulo-http` pipeline and server, HM
`ulo-http-macros` and the axum backend). Nothing has compiled. Each item cites its entry:
`[spine 12]`, `[P 2]`, `[CX R1]` for a request. A point two logs cover appears once with both
citations.

The `fw` → `ulo` rename is applied throughout without an entry each: crate names,
`__ULO_KEY_<name>`, `__ULO_KEYS_CHECK_<key>`, and the span attributes `ulo.transport`,
`ulo.handler` and `rpc.system = "ulo"` [spine, preamble].

## 1. Decisions for you

### T1. How an error handler recognises a 405 [P 3]

- **Design:** §3.2: a matching path with the wrong method answers 405 with `Allow`; §3.3: on a
  miss only the global error handlers apply. No `ErrorKind` row is 405.
- **Built:** a 404 reaches the global error handlers as
  `CallError::new(NotFound, "no route matches this path")`. A 405 reaches them as a private
  `MethodNotAllowed` error carrying `Allow`: an error handler sees an opaque error, and
  `CallError::from_boxed` on it answers `Internal`. Unclaimed, `service.rs` renders problem details
  with status 405 and `Allow`. `OPTIONS` with no handler answers 204 with `Allow` directly and is
  offered to nobody.
- **Options:** (a) a public `ulo_http::MethodNotAllowed` type with `allow()` that a handler
  downcasts; (b) an `ErrorKind::MethodNotAllowed`, though every other kind maps on all four
  transports and 405 is HTTP's alone; (c) leave it opaque.
- **Recommendation:** (a); the kind table stays transport-neutral and the 405 stays reshapeable.

### T2. The axum backend serves through hyper; does the crate keep its name? [HM 1, spine 47, spine 63]

- **Design:** §1 lists `fw-http-axum` as one `fw_http::Backend`; DESIGN §9.4 writes
  `fw_http::Server::new("0.0.0.0:8080")`, naming no backend.
- **Built:** `axum::serve` is not used. Each connection is served by
  `hyper_util::server::conn::auto::Builder` or `hyper::server::conn::http1::Builder`; axum supplies
  `axum::body::Body` and the request and response aliases, nothing else. Four reasons: `axum::serve`
  accepts the HTTP/2 preface on every plain connection, which `h2c = false` (the default) cannot
  survive; it cannot set `SETTINGS_MAX_CONCURRENT_STREAMS`; it spawns connection tasks detached, which
  `Backend::close` cannot close; it awaits the TLS handshake inline in the accept loop, stalling every
  connection behind a slow peer. The server is spelled `ulo_http_axum::Server::new(..)`, an alias for
  `ulo_http::Server<Axum>`, because `ulo-http` cannot default to a backend that depends on it.
- **Options:** keep `ulo-http-axum` (the ecosystem's types and `tower-http` layers fit); rename to
  `ulo-http-hyper` (names what runs); or the hyper crate now with an axum-named wrapper later.
- **Recommendation:** rename to `ulo-http-hyper` before the first publish. The four reasons are
  permanent, and a crate named for a dependency it barely uses misleads; the alias then reads
  `ulo_http_hyper::Server::new(..)`.

### T3. The 30-second TLS handshake and header-read timeouts [HM 3, HM 4]

- **Design:** silent on both. `axum::serve` sets no timer, and hyper skips its default
  header-read timeout without one.
- **Built:** both HTTP/1.1 builders get `TokioTimer`, which turns on hyper's default: a request head
  must arrive within 30 seconds of hyper starting to read it, a clock that also runs while a
  keep-alive connection waits for its next request. A TLS handshake is bounded by 30 seconds to
  match. A failed or timed-out handshake is logged at `debug` with the peer address and the
  connection dropped. Without the timeout a peer holds a connection open indefinitely and the drain
  waits on it until its deadline.
- **Options:** keep both fixed at 30 s; expose `header_timeout` and `handshake_timeout` on the
  server builder; turn both off to match `axum::serve`.
- **Recommendation:** keep 30 s for the first compile; add the two knobs to `HttpConfig` as
  `Option<Duration>` afterwards.

### T4. A generic handler is refused [MX 1]

- **Design:** §2.3 mentions a handler with a type or const parameter only to exempt its opaque
  returns from `+ use<>`. The spine's `HandlerSig::is_generic` doc said its checks are "read from
  the mount function".
- **Built:** `compile_error!` at the handler's name: "`get` declares a type or const parameter; a
  handler is not generic, since the generated call names each parameter's type outside the method".
  The three owed items are still written, with an empty mount function and `__ULO_CHECKS_<name>: ()
  = ()`, so the refusal is the one error. The spine's mechanism cannot exist: the call closure, the
  dependency reads and the checks constant name each parameter's type where the method's own type
  parameters are out of scope, and nothing at the call could infer them. A generic *controller*
  works (MX 2).
- **Options:** accept; or design generic handlers later (a turbofish on the attribute).
- **Recommendation:** accept; a handler has no caller to infer its parameters from.

### T5. A handler behind `#[cfg]` is not handled, and the fix changes a public signature [MX 7]

- **Design:** silent.
- **Built (unchanged from the spine):** an attribute macro receives `#[cfg(feature = "x")]`
  unevaluated, so `#[routes]` treats the method as a handler and writes references to its
  `__ULO_KEY_*`, `__ULO_CHECKS_*` and `__ulo_mount_*`. With the feature off, the method is stripped
  and those references fail to resolve.
- **Proposed fix:** `#[routes]` copies each handler's `cfg` attributes onto its checks read and its
  mount call, and the key assertion becomes a `const` block of `#[cfg]`-gated statements instead of
  one `key_in` over an array literal. Both need the handlers' `cfg` attributes in
  `keys::assertions`, whose public signature takes bare identifiers.
- **Options:** apply before the first compile (a public signature in `ulo-handler-codegen`, which
  nothing outside the workspace calls); defer and document.
- **Recommendation:** apply before the first compile; a handler under a feature flag is ordinary
  Rust.

### T6. Parameters read through `Param<T, _>`; the core's container types implement no `FromCall` [spine 28]

- **Design:** §2.2: autoref ranks `P: FromCall<T>` first and `S: FromContainer` second as
  `Injected<S>`; `fw-transport` implements `FromCall<T>` for `Dep`, `Many`, `Ext`, `ModuleRef` and
  `ExecutionRef`; the pairwise assertion reads `<P as FromCall<T>>::CONSUMES_BODY`.
- **Built:** `ulo_transport::__private::Param<T, M>`, implemented for every `FromCall<T>` type
  (`M = ViaCall`) and every `FromContainer` type (`M = ViaContainer`); generated code names
  `<P as Param<T, _>>` for the dependency reads, the extraction and the assertion. The core's
  injection points reach a handler through `FromContainer` alone; `Injected<S>: FromCall<T>` stays
  for generic code. The assertion sits in a constant, where autoref ranking does not run, and
  `<Dep<U> as FromCall<T>>` does not compile for a bare container type. `Option<Dep<U>>` behaves as
  designed through the core's `Option<S>`.
- **Options:** accept; or also implement `FromCall<T>` for the five core types, after which a type
  implementing both traits is ambiguous under `Param<T, _>`.
- **Recommendation:** accept; the handler's spelling is unchanged and the `on_unimplemented` note
  still points at the parameter's type.

### T7. `Shared<(Arc<V0>, ..)>` instead of a generated struct [spine 20]

- **Design:** X2: a generated `__FwShared<V0, V1, ..>` struct.
- **Built:** a core `pub struct Shared<T>(pub T)` in `ulo::__private`, `T` a tuple of `Arc<Vi>`,
  `Shared(())` with no value; the per-handler mount function takes `&Shared<(Arc<__UloV0>, ..)>`. A
  struct `#[routes]` generates needs a name unique per module, and the transport attribute on a
  method cannot learn the controller's type name to spell it.
- **Options:** accept; or pass the controller's name through `__handler`, changing the frozen
  grammar.
- **Recommendation:** accept.

### T8. A route timeout's 504 bypasses the error handlers [P 2]

- **Design:** §3.6: `#[meta(Timeout(..))]` fires `Deadline` and renders 504; §2.4: the deadline is
  not in the error, the reason lives on the execution.
- **Built:** the timer is armed after routing, on the app's `Timer`, around the scoped sub-step and
  `dispatch`; the unscoped entries run before it. If the sleep wins, the pipeline is dropped at its
  current await, the execution is cancelled with `Deadline`, and `render::timed_out()` renders
  problem details with no error handler run. If the pipeline wins, the pending sleep moves into the
  response body; when it passes, the execution is cancelled and a streaming body observes it through
  `cancelled()`.
- **Options:** accept (an error handler run after the deadline is unbounded by it); run the error
  handlers with a `Timeout` `CallError` under a second, short bound.
- **Recommendation:** accept; the design keeps the deadline out of the error already, and a
  `#[catch]` on a timeout was never promised.

### T9. A tower layer calling its inner service twice gets a 500 [P 1]

- **Design:** §3.4: layers are composed once in `prepare` and run inside the stage; silent on how
  the request carries its continuation.
- **Built:** the continuation (the rest of the chain, the `ConnInfo` and the upgrade future, which an
  `http::Request` cannot carry) rides the request's extensions in a take-once slot every clone
  shares. A layer that calls its inner service twice for one request (a retry layer), or builds a
  new request without the original's extensions, gets a bare 500 from `ulo_http::Service` and an
  `error` log line. The chain is one-shot regardless: the body has been read and the middleware
  consumed.
- **Options:** accept and state in `layer`'s doc that retries belong in a client; make the slot
  clonable, which cannot replay a read body.
- **Recommendation:** accept, with the limitation in `layer`'s doc.

### T10. SSE keep-alive requires a tokio runtime [H 17]

- **Design:** `Sse::keep_alive(Duration)` writes `: keepalive` comments; the core depends on no
  runtime; the clock's source is unnamed.
- **Built:** the timer is `tokio::time::Sleep`, created on the body's first poll, so the body must be
  polled on a tokio runtime with time enabled. The app's `Timer` is not reachable from `HttpCx`. A
  comment is written when the period passes with nothing written; every event and every comment
  restarts the clock, so none is written while events flow. `ulo-http` already depends on `tokio`
  with `time` for its body I/O.
- **Options:** accept; thread the app's `Timer` into `HttpCx` so SSE uses the configured clock; drop
  keep-alive from the reply and offer a `Timer`-based helper.
- **Recommendation:** accept now and file threading `Timer` through `HttpCx` as a follow-up; it is
  the one place `ulo-http` picks a runtime the app did not.

### T11. A `LISTEN_PID` mismatch is an error where systemd ignores it [N 1]

- **Design:** §2.7: an inherited socket is accepted only when `LISTEN_PID` equals the process ID;
  §12: a mismatch fails startup at `prepare`.
- **Built:** as §12: `PidMismatch`, where `sd_listen_fds` answers zero sockets. `Activation::get()`
  is read only when an endpoint is inherited (P 10), so an address-only server under a stale
  `LISTEN_PID` is unaffected. Two further departures from `sd_listen_fds`: `LISTEN_FDS=0` is no
  sockets (systemd: `EINVAL`), and a missing `LISTEN_PID` or `LISTEN_FDS` is no sockets, not an
  error.
- **Options:** keep the error; follow systemd and treat the mismatch as no sockets, which then fails
  as `Missing` naming the endpoint.
- **Recommendation:** keep the error; `Missing` hides the cause.

### T12. Wiring reports number a handler's parameters, and naming them changes a contract [CX 5, CX R1]

- **Built:** step 3 reports a missing or ambiguous key a parameter reads as
  ``UsersController::get (param #1)``; step 5 reports an unseeded input as
  ``UsersController::get_rpc (Rpc) → Dep<RequestHead> (param #1)``. `#n` counts container-read
  parameters, not signature position: `get(&self, id: Path<u64>, svc: Dep<Svc>)` reports `svc` as
  `param #1`, because `Injected::dependencies` writes `Dependencies::add` and `Param::dependencies`
  has no name to pass.
- **Fix (unapplied):** `Param::dependencies_named(d, name)`, answered by the `ViaContainer` impl with
  `d.param::<S>(name)` and the `ViaCall` impl with `P::dependencies(d)`, called by the generated code
  with the parameter's identifier; the report then prints ``param `svc` ``. A change to a frozen
  T/MX contract; reports only, nothing goes unchecked without it.
- **Recommendation:** apply after the first compile.

### T13. Two workspace-manifest requests, both unapplied [N R1, H req 5, F306]

- **`libc = "0.2"`, optional [N R1]:** socket2 reads `SO_ACCEPTCONN` on Linux, Android, FreeBSD,
  Fuchsia and AIX only. On macOS and the other BSDs a nonzero bound port stands in, so a bound
  socket that never called `listen` passes `prepare` and fails at its first accept. `libc` is in the
  lockfile already through socket2. Coupled to F306's candidate fix.
- **Drop `serde_urlencoded = "0.7"` [H req 5]:** `ulo-http` no longer uses it (H 9); the root
  `Cargo.toml` still lists it.
- **Recommendation:** add `libc` (the development machine is the BSD case) and drop
  `serde_urlencoded`, both in the compile commit.

### T14. An optional 405 renderer in `render.rs` [P H2]

- **Built:** the 405 problem document is written in `service.rs`, apart from the other problem
  documents in `render.rs`. P's optional request: a crate-private
  `render::method_not_allowed(allow, config)` or an exposed `document(error, status, config)`, so
  `service.rs` drops its own. Left for after the first compile.
- **Recommendation:** apply once T1 is decided; the renderer's shape follows the 405's
  representation.

### T15. One input, two transports: WebSocket [CX 8, F308]

- **Design:** an input has one seeder (`seeded_by::<Tr>()`, `InputDecl::seeder`); §2.10 lists
  `ConnectionInfo`, `UpgradeHead` and `SessionHandle` under WebSocket, which is two transports, `Ws`
  (messages) and `WsConnect` (the connect phase).
- **Built:** two transports declaring one input is `WiringError::InputConflict` naming both. Before,
  the first transport to mount won without a report and the second's handlers failed as
  `InputNotSeeded`. If both WS transports declare the inputs, wiring fails; if one does, the other's
  handlers read them unseeded.
- **Options (F308):** `seeded_by` taking a set of seeders; accepting a second declaration of a key
  from a transport whose crate also declared it.
- **Recommendation:** a set of seeders, decided before `ulo-ws` declares its inputs in 2b; one
  declaration per key still says who seeds it.

### T16. `Tls::from_pem_files` and endpoint text fail at `prepare`, not where written [spine 42, spine 43]

- **Design:** §0.6 names `Tls::from_pem_files` and `Endpoint::parse` as fallible conversions
  returning `Result`; §2.7 and §12 parse both in `prepare`, so a bad certificate or endpoint fails
  startup as `Configure`. The two clauses conflict.
- **Built:** `Tls::from_pem_files(cert, key) -> Tls` and `Tls::from_pem(..) -> Tls` record their
  source; `Tls::load(alpn) -> Result<TlsAcceptor, TlsError>` reads and parses in `prepare`. Server
  builders take `impl Into<EndpointSpec>` (text, an `Endpoint`, a `SocketAddr`); text resolves in
  `prepare`. `main` writes no `?` on either.
- **Options:** accept §2.7's behaviour; or keep `Result` constructors and parse again in `prepare`.
- **Recommendation:** accept and amend §0.6's example; a `Result` in `main` contradicts
  one-report-per-startup.

### T17. `EndStream` is not `#[non_exhaustive]` [spine 26]

- **Design:** DESIGN §10.2 makes every public error type `#[non_exhaustive]`.
- **Built:** `pub struct EndStream;` with `Default`. An error handler constructs it
  (`Err(EndStream.into())`), which `#[non_exhaustive]` forbids outside the core.
- **Options:** accept the exception; a `#[non_exhaustive]` struct with `EndStream::new()`.
- **Recommendation:** accept; a unit marker has nothing to add later.

### T18. `length` counts characters for text [T 5]

- **Spine:** `Rule::Length` "checked against `.len()`".
- **Built:** text (anything `AsRef<str>`) is counted in characters; a collection whose borrowing
  iterator is an `ExactSizeIterator` is counted in items, picked by autoref through
  `__private::validate::LengthProbe`. A byte count makes `length(max = 64)` refuse a 40-character
  name in a non-Latin script; the `validator` crate counts characters too. Consequence: a user type
  with an inherent `len()` and no borrowing `ExactSizeIterator` does not compile under `length`.
- **Recommendation:** accept; the refused case is rare and fails at compile time.

### T19. No blanket `Validate` for `validator::Validate` [T 7, spine 36]

- **Design:** "a bridge for the `validator` crate sits behind a feature flag".
- **Built:** the `validator` feature exposes `validator_bridge::violations(&ValidationErrors) ->
  Vec<FieldViolation>`; the user writes a three-line `Validate` impl calling it, shown in its docs.
  `impl<T: validator::Validate> Validate for T` is not written: a blanket impl behind a feature
  changes coherence for every downstream crate when it turns on, and features must be additive.
- **Recommendation:** accept.

### T20. A constructor's 401 challenge is lost through a user's own `?` [T 4]

- **Built:** an execution-scoped constructor answering `CallError::unauthorized(challenge)` keeps
  the challenge through `from_boxed` and `Param::extract` (a crate-private
  `CallError::from_extract`), but loses it through a user's own `?` on the blanket
  `From<ExtractError>`, since `Classify` has no method to carry one.
- **Options:** accept; add `Classify::challenge(&self) -> Option<..>` defaulting to `None`.
- **Recommendation:** accept for 2a; add the method when a constructor-raised challenge appears.

### T21. `WiringError::InputConflict` carries pre-rendered text [CX 7]

- **Design:** a transport-declared input's origin prints "declared by transport `Http`"; no
  variant carried a transport origin, every existing report naming a module.
- **Built:** `InputConflict { key: KeyName, first: String, second: String }`, each side a line:
  ``declared by transport `Http` at <file>:<line>``, ``declared in AppModule with seeder `Rpc` at
  <file>:<line>``, ``bound in AppModule at <file>:<line>``. One variant covers three conflicts
  without a new public origin type. Cost: a test matches the variant and the key, not the origin's
  parts, and a module name inside the text is not lengthened when two modules print alike.
- **Options:** accept; a public `InputOrigin` in the variant.
- **Recommendation:** accept; `Missing::consumer` is rendered text already.

### T22. Invalid UTF-8 in a path parameter is captured with U+FFFD, not answered 404 [H 1]

- **Plan point, H decides.** Each request segment is percent-decoded as a path: `+` stays `+`,
  `%2F` decodes to `/` inside its segment without splitting it, a `{*rest}` value is the raw
  remainder decoded the same way. A decoded sequence that is not UTF-8 is captured with U+FFFD per
  invalid sequence, because `PathParams` holds `String`s (frozen) and `Routed` has no error variant.
  The alternative, a miss, answers 404 for a path that does name a route.
- **Recommendation:** accept; a typed `Path<T>` then fails as `Malformed` naming the field.

### T23. Route precedence is path-first, and a less specific pattern is not consulted for the method [H 4]

- **Design:** silent on which of two matching patterns answers.
- **Built:** routes are ordered once in `prepare` by a key compared left to right: static before
  parameter before rest, so `/users/me` answers before `/users/{id}` whatever the declaration order.
  The first matching pattern decides the method check: its handler, else `GET` for `HEAD`, else 204
  for `OPTIONS`, else 405. With only `GET /users/me` and `DELETE /users/{id}` declared,
  `DELETE /users/me` answers 405. RFC 9110: the URI names the resource and 405 means that resource
  lacks the method.
- **Options:** accept; fall through to the next matching pattern on a method miss, which lets
  `/users/{id}` answer `/users/me` for some methods and not others.
- **Recommendation:** accept.

### T24. `max_concurrent_streams` unset keeps hyper's default [HM 5]

- **Design:** `HttpConfig` holds `Option<u32>` with no stated meaning for `None`.
- **Built:** `None` leaves hyper's default (200, which hyper places outside its stability
  guarantee); `Some(n)` sets `n`. Passing `None` through to hyper would remove the limit.
- **Recommendation:** accept, documenting "hyper's default" rather than the number.

### T25. The `Send` bounds H assumes of CX are unconfirmed [H req 4]

- **Request:** the SSE body boxes `ulo::dispatch_late(..)` as a `BoxFuture<'static, LateOutcome>`
  from owned clones of the handler, the execution and the `HttpCx`, so `recover` and
  `dispatch_late` must return `Send` futures for `T = Http` and `MountedHandler<Http>` must be
  `Send + Sync`; P's `AppService::call` assumes the same of `dispatch`. CX's log does not mention
  it. In the code, both are plain `async fn` with no declared bound on the future, and
  `MountedHandler<T>` holds `ModuleRef`, two `EnhancerSpec<T>`, `Arc<HandlerInfo>` and
  `Arc<dyn Any + Send + Sync>`, so its auto-traits follow `EnhancerSpec<Http>` and `ModuleRef`. The
  compile decides.
- **Recommendation:** no decision before the compile; if it fails, bound `Transport::Cx` and the
  spec types rather than make the SSE body non-`Send`.

### T26. HTTP/2 extended CONNECT is not advertised [HM 6]

- **Design:** §3.5 hands `Upgrade: websocket` to `ulo-ws`, an HTTP/1.1 mechanism; RFC 8441 is not
  mentioned.
- **Built:** the HTTP/2 builder does not send `SETTINGS_ENABLE_CONNECT_PROTOCOL`; `axum::serve`
  does. Nothing in 2a answers an extended CONNECT.
- **Recommendation:** accept; revisit when `ulo-ws` decides whether to speak WebSocket over HTTP/2.

## 2. What you will write differently

- **The server names its backend** [spine 47, spine 63; T2 for the name]:
  before `fw_http::Server::new("0.0.0.0:8080")`
  after `ulo_http_axum::Server::new("0.0.0.0:8080")`, an alias for `ulo_http::Server<Axum>`;
  `Server::with_backend(endpoint, backend)` for a backend without `Default`.
- **TLS and endpoints carry no `?`** [spine 42, spine 43; T16]:
  before `.tls(Tls::from_pem_files("cert.pem", "key.pem")?)`
  after `.tls(Tls::from_pem_files("cert.pem", "key.pem"))`, read in `prepare`. Endpoint text, an
  `Endpoint` or a `SocketAddr` go straight into `Server::new`.
- **Server builder** [spine 48, spine 49]: `endpoint`, `tls`, `body_limit`, `max_inflight`,
  `max_concurrent_streams`, `shed_retry_after`, `challenge`, `h2c`; `HttpConfig` is
  `#[non_exhaustive]` with those fields, the body limit 2 MiB unset. `KB` and `MB` are `u64`
  constants; `#[meta(BodyLimit(2 * MB))]` and `#[meta(Timeout(..))]` as the design writes them.
- **A sync-bodied closure in a providers list compiles in every position** [spine 24]:
  before only an `into` entry's closure was wrapped; `with = |db: Dep<Db>| Svc::new(db)` failed.
  after `with`, `with(scope)`, `K: with` and the bare closure all wrap. `K: with = ..` with `K` a
  role key is refused; write `into AnyGuard<Http>: [with = ..]` [spine 25].
- **Receivers** [MX 5]: `&self`, `&'a self`, `self: &Self`, `self: Arc<Self>` and
  `mut self: Arc<Self>` are accepted; `&mut self` and `self` are a span error; an alias of `Arc` is
  refused, since the generated call passes `std::sync::Arc<Self>`.
- **Container reads in a handler keep their spelling** (`Dep<Svc>`, `Many<K>`, `Ext<T>`,
  `Option<Dep<U>>`); the mechanism is `Param<T, _>` (T6) [spine 28]. Generic code over a
  `T: Transport` writes `Injected<S>` for the `FromCall` form.
- **Validation** [spine 35, 36, 37, 38; T 7, 10, 11]: `Valid<Json<NewUser>>` as designed;
  `Valid<P>` needs `P: Validate`, which `Json`, `Form`, `Query` and `Path` implement by delegating,
  so a custom extractor under `Valid` writes its own impl. `#[derive(Validate)]` and
  `#[derive(Classify)]` emit `::ulo_transport::..`, so a crate deriving either depends on
  `ulo-transport`. Rules: `length(min, max)` (either bound optional; characters for text, T18),
  `range(min, max)`, `email` (a shape check, not RFC 5322); several rules may share one attribute.
  `#[classify(kind)]` goes per variant or on the enum as a default. A violation's `field` is the
  name serde gives it (`rename`, `rename_all`, `r#` stripped); `flatten` and nested structs are not
  followed. The `validator` bridge is the `validator` feature plus a three-line `Validate` impl
  calling `validator_bridge::violations` (T19).
- **Error values** [spine 31, 32, 33]: `CallError` adds `with_source`, `message`, `details`,
  `challenge` and `summary()`; `ErrorKind::as_str()` gives `"bad_request"` and kin;
  `Details::{new, push, with, iter, len, is_empty, retry_after}`, `FieldViolation { field,
  description }`, `Link { description, url }`. `IntoReplyError::new(impl Into<BoxError>)`.
  `CallError::grpc_code` waits for 2b.
- **Stream callbacks** [spine 10, spine 26, CX 1]: an `on_stream_end` callback matches
  `StreamOutcome::{Completed, CutOff(Option<CancelReason>)}`, `None` for a stream cut by an error
  item or dropped with nothing recorded. An error handler ending a stream writes
  `Err(EndStream.into())`.
- **`HttpCx`** [spine 51]: `cx.ext::<T>()` returns `Result<Ext<T>, LookupError>` (write `?`);
  `response_headers()` returns the `MutexGuard`; also `param`, `params`, `extensions`,
  `take_upgrade`, `conn`, `app`.
- **Replies** [spine 50, spine 59]: `Created::at(location, body)`, `NoContent`,
  `WithHeaders::new(reply).header(name, value)`; `String` and `&'static str` are
  `text/plain; charset=utf-8`, `Bytes` is `application/octet-stream`. One body type `HttpBody` with
  `Body` as an alias; `Response` is `http::Response<HttpBody>`.
- **SSE** [spine 60, H 17]: `Sse::end_event("end")` takes a `&'static str`, validated when the
  reply converts; `Event::json(&v)` and `Event::comment(text)` exist; `EventId::new(..)?` and
  `EventName` fail with `EventFieldError`; items are `Event` or `Result<Event, E: Into<CallError>>`
  through `SseItem`. `.keep_alive(d)` needs tokio (T10).
- **Pre-dispatch scopes** [spine 55, H 8, P 11]: a scope is a route pattern whose last segment may
  be `*` or `{*name}`, matching one or more segments: `/admin/*` covers `/admin/users` and
  `/admin/{id}/notes`, not `/admin`. A scope's parameter covers a route's parameter or static
  segment whatever the names. `exclude` applies to the entry written before it; `apply_for` and
  `layer_for` with an empty pattern list are refused at `prepare`.
- **`Cors`** [spine 57]: `new()` allows nothing; `allow_origin` (repeatable), `allow_any_origin`,
  `allow_methods`, `allow_headers`, `allow_any_header`, `expose_headers`, `allow_credentials`,
  `max_age`. Any origin with credentials is refused in `prepare` as `CorsError`.
- **Middleware** [spine 54, spine 52, H 18]: `Middleware::handle` answers a `Response` and has no
  `Err`; a middleware fails only by panicking, and an `Err` comes from a tower layer.
  `Request::set_path(p)` returns `Result<(), http::Error>` and adds a missing leading `/`.
- **`ulo-tokio`** [spine 46]: `Timer` is a unit struct; `shutdown_signal()` resolves to
  `Signal::new("SIGTERM")`, `"SIGINT"`, `"CTRL_C"` or `"CTRL_CLOSE"`; `spawn_in` returns
  `JoinHandle<Option<T>>`, `None` when the execution was cancelled first.
- **`AppHandle`** [spine 16, spine 18]: `root() -> ModuleRef`; `phase()` answers
  `#[non_exhaustive] enum Phase { Running, Stopping, Draining, Destroying, Closed }`, ordered,
  `Running` before a handle exists.
- **`Metadata`** [spine 1, spine 2]: in `ulo`, re-exported by `ulo-transport`;
  `Metadata::controller(v)` / `Metadata::method(v)`; `get` answers the most specific (method over
  controller, first written within a tier), `get_all` every declaration method first, `entries()`
  gives `(type name, MetaTier)`.

## 3. Behaviour that differs or was filled in

### Wiring and startup

- `listen()` refuses a missing `Timer` first (`StartupError::Bind`), then runs every `prepare`,
  then every `bind`; `prepare` would otherwise receive no clock [spine 9].
  `StartupError::Configure(ConfigureErrors)` holds one `ConfigureError { transport, source }` per
  failing `prepare`, with `iter`, `len`, `is_empty` and a report `Display` [spine 8].
  `Server::prepare` has a default `Ok(())`; `bind(&mut self, Mounted<'_, T>)` is unchanged
  [spine 6].
- A handler parameter's container reads are checked in step 3 against the controller module's
  table and in step 5 as roots of the input walk. They take no part in the cycle check or the
  needs-execution pass, so a parameter reading `Ext<T>` makes nothing per-execution [CX 5].
- A handler parameter reading `Many<K>` blocks a lazily loaded module contributing to `K`
  (`LoadRefusal::Contribution`) [CX 6].
- Two transports declaring one input, a transport's declaration beside a module's with another
  seeder, or beside a single binding under the key, is `InputConflict`; inputs only modules declare
  keep `DuplicateBinding` [CX 7, CX 8; T15, T21].
- `.at(prefix)` is recorded on each `HandlerInfo` (`prefix()` beside `route()`); the transport joins
  the two by its own path rules, and an RPC pattern takes none [spine 4].
- A transport's `inputs` record `InputOrigin::Transport { name, at }` [spine 19].
- HTTP `prepare` [P 10]: one `PrepareError` listing every failure, wrapped by `listen()` as the
  transport's `ConfigureError`. A failed pre-dispatch stage still builds the route table, so route
  conflicts are reported beside it. Each `BackendLimits` entry is checked: `upgrades`, `h2c`
  against `.h2c(true)`, `tls` against `.tls(..)`, `inherited_sockets`, `port_zero`. An address
  listed twice is refused, except port 0. `Activation::get()` is read only when an endpoint is
  inherited, once per `prepare`. Each inherited name is checked against `Activation::count`: a name
  no untaken descriptor answers to fails as `Missing`; a name listed more often than descriptors
  answer fails naming the name and both counts. TLS loads with ALPN `h2` then `http/1.1`. `bind`
  without a prior `prepare` answers an error.
- Upgrade paths are parsed as route patterns in `prepare`; two that match the same requests are a
  `Configure` failure, as is any upgrade path on a backend declaring `upgrades: false` [P 9].
- Two patterns differing only in parameter names are refused whatever their methods, naming both
  handlers as `Controller::method`; the spine's doc said "same method", the design names none, and
  `HttpCx::param` would otherwise answer differently by method [H 5]. The grammar also refuses an
  empty segment (`/a//b`); a name is identifier-like, Unicode included [H 3, HM 11].
- `Path<T>`'s startup check is exact both ways: a struct's fields must be exactly the route's
  parameters; a newtype is checked as what it wraps (`UserId(u64)` takes one parameter,
  `Wrapper(Params)` is checked as `Params`); `Option<T>` as `T`; a unit, map, sequence and
  `deserialize_any` are unchecked. An `Option` field the route never provides is refused at startup
  though it would extract as `None` [H 10].

### Dispatch and the error handlers

- An extraction failure reaches the error handlers as `CallError::from(extract_error)` with the
  `ExtractError` as source (the spine boxed it bare); a reply-conversion failure as an `Internal`
  `CallError` holding the `IntoReplyError` [T 1].
- `CallError::from_boxed` maps a constructor's `Errored(r)` by the whole table (`CallError`,
  `ExtractError`, `GuardRejected`, `PanicRecovered`, a nested `LookupError`, `Closed`, another
  `Redacted`), so a constructor returning a dependency's `LookupError` keeps that dependency's 404
  [T 2]. It looks inside a `Redacted` that is the error itself or the `Errored` reason, no deeper,
  and walks no `source()` chain: an outside error wrapping a `CallError` is `Internal`; a bare
  `CallError` wins over a panic found deeper; `ulo::is_panic` is tested after `CallError` and
  `ExtractError` and before every other row [T 9].
- `ExtractError::Dependency { param, source: LookupError }` is a sixth variant; its public message
  and details are what its lookup error maps to, so a key name never reaches the caller
  [spine 29, T 4].
- `recover(Some(handler), ..)` sets the handler's `HandlerInfo` on the execution first, as
  `dispatch` does (a second set changes nothing). `recover(None, ..)` resolves the global handlers
  at the execution's current module: the one `route_to` set, otherwise the one it opened in, the
  root for HTTP [spine 14, CX 2, CX 3].
- A pre-dispatch entry's panic reaches the error handlers as `PanicRecovered { stage: PreDispatch,
  .. }`, message redacted [spine 15]. Each entry runs under its own `catch_panic`, `next` included,
  so what reaches an entry's catch is its own panic. The code between the stage and `dispatch`
  (upgrade matching, routing, context building, rendering) is not an entry: a panic there is
  attributed to the innermost unscoped entry, or reaches the backend when none exists; an
  `UpgradeHandler::upgrade` panic takes the same path [P 5].
- The call extracts every parameter in order, then resolves the controller, then runs the method;
  a failed extraction builds no per-execution controller [MX 6].
- `route_to` returns nothing: the first call wins; a later call, or a module of another app,
  changes nothing [spine 17].
- `dispatch_late` sets `is_late` before the first error handler and never clears it. `End` is
  answered when the error the walk ends with is an `EndStream` at its top level, an unclaimed
  `EndStream` item error included; one wrapped inside another error renders as that error [CX 1,
  spine 12]. After a late handler's `Ok`, the transport logs `Ignored` and renders
  `CallError::summary()`, a copy without the source [spine 13, spine 31].
- A panicking `on_stream_end` callback is caught and dropped; the callbacks after it still run;
  the panic hook has already printed it [CX 4]. A callback registered after the end runs at once
  with the recorded outcome; a later report changes nothing [spine 11].
- HTTP misses [P 3; T1]: 404 is offered to the global error handlers as
  `CallError::new(NotFound, "no route matches this path")`; 405 as a private error; `OPTIONS` with
  no handler answers 204 with `Allow` directly.
- 413 and 415 render from the `ExtractError` (bare or as a `CallError`'s source) only while the
  `CallError` is still `BadRequest`; reshaped to another kind, that kind renders. A 415 carries
  `Accept` naming the media type the extractor expected [H 16].
- The route timeout: T8. The deadline's `Timeout` rendering also applies to an SSE error written
  on an execution cancelled with `Deadline` [H 17].

### Routing

- Precedence: T23. `Allow` is the pattern's methods in declaration order, then `HEAD` after `GET`
  when only `GET` is declared, then `OPTIONS` when no handler declares it, computed once in
  `prepare` [H 6].
- A parameter or rest never captures an empty segment: `/u//x` does not match `/u/{id}/x`, and
  `/files//` does not match `/files/{*rest}` [H 2]. Percent-decoding: T22. A static segment matches
  the request segment as sent or once decoded, so `/caf%C3%A9` matches `/café` [H 1].
- `HEAD` from `GET` [H 7, P 8]: after `dispatch`, a `Routed::Found { head_from_get: true }`
  response has its body replaced with an empty one, unread. When the response carries no
  `Content-Length` and the dropped body's size hint is exact, that size is set, except on 1xx, 204
  and 304; a `Content-Length` the handler set is left. A streaming `GET` body is dropped before its
  first item and `Tracked` reports `CutOff(None)`. H and P agree after H's request 1.
- Upgrades at runtime [P 9]: after the unscoped entries, any request carrying an `Upgrade` header
  whose path matches a registered upgrade path goes to that `UpgradeHandler` before routing,
  whatever the header's value; the handler decides what it accepts. No `route_to` runs (the
  gateway's module is unknown to HTTP) and the scoped entries do not run.

### Extraction

- `Path`, `Query` and `Form` share one deserializer over name-value pairs. A missing field is
  `Missing { param: "<field>" }`, a value that does not parse `Malformed { param: "<field>" }`, a
  field given twice `Malformed`; `"path"`, `"query"` or `"body"` only when serde names no field.
  Consequence: `Option<Query<T>>` is `None` when a required field is absent [H 9]. Query and form
  grammar is WHATWG's (`+` as space, empty pieces skipped, U+FFFD for invalid UTF-8); a struct,
  map, `Option`, unit enum and `Vec<(String, String)>` deserialize; an empty value for an
  `Option<u32>` field is `Malformed`, as with `serde_urlencoded`.
- An empty body (`is_end_stream` or `Content-Length: 0`) is `Missing { param: "body" }` for
  `Json`, `Form` and `Multipart`, whatever the media type, so `Option<Json<T>>` is `None` on a
  bodiless request. `Bytes` extracts empty bytes and `BodyStream` an empty stream. A body already
  taken (a guard's `take_body`) is `Missing` for all five [H 11].
- Media types [H 12]: `Json` takes `application/json` and any `+json` suffix, essence compared
  case-insensitively; `Form` `application/x-www-form-urlencoded`; `Multipart` `multipart/form-data`
  with a boundary; no `Content-Type` is 415. Order: media type, then `Content-Length` against the
  route's limit (413 before a byte is read), then the bytes as they arrive (413 the moment they
  pass, whatever `Content-Length` said); a failed read is `Malformed { param: "body" }`.
  `BodyStream` and `Multipart` passing the limit mid-read end with a boxed `TooLarge` then `None`;
  `Multipart` surfaces it from `next_field` as multer's `StreamReadFailed` [spine 61].
- `Header<H>` reads every value of `H::name()`; none is `Missing` naming it, so
  `Option<Header<H>>` is `None`; a failed `decode` is `Malformed`. `LastEventId` reads the header's
  bytes as UTF-8, admitting non-ASCII ids; bytes that are not UTF-8 are `Malformed` [H 13,
  spine 61]. `Option<P>` is `None` exactly on `ExtractError::Missing` [spine 41].
- `Valid<P>`'s `Invalid.param` is `P`'s outermost type without generics (`"Json"` for
  `Valid<Json<NewUser>>`), sliced from `type_name`, so crate paths stay out of the client-visible
  detail; the violations name the fields [T 6].

### Replies, rendering and streams

- Only a body of unknown length (`size_hint().exact()` is `None`, as `Body::stream(s)`) is wrapped
  in `Tracked`, in the `IntoReply` impls of `HttpBody` and `Response`; a known-length body is
  written whole and an `on_stream_end` callback registered for it never runs. Wrapping would drop
  `Content-Length` on every response [H 14]. `Tracked` reports `Completed` when the inner stream
  returned `None` before the wrapper drops; the transport drops it after writing the stream's end
  [spine 41].
- Problem documents [H 15]: `title` is the RFC 9110 phrase ("Content Too Large", "Unprocessable
  Content", where the `http` crate still has the older ones); `details` omitted when empty;
  `Retry-After` in whole seconds, a fraction rounded up, agreeing with the `seconds` in `Details`'
  JSON [T 14, T's advisory]; a 401 challenge that is not a valid header value is logged at `warn`
  and `Bearer` sent; `draining` and `shed` answer `Unavailable` with
  `Retry-After: <shed_retry_after>`, `draining` adding `Connection: close`; a `Details` that fails
  to serialize is dropped with a `warn`.
- `Details`' JSON members [T 14]: `{"type": "field_violations", "violations": [{"field",
  "description"}]}`, `{"type": "error_info", "reason", "domain", "metadata"}`,
  `{"type": "retry_after", "seconds"}`, `{"type": "help", "links": [{"description", "url"}]}`,
  `{"type": "json", "value"}`.
- Messages `from_boxed` writes for a withheld one: `"internal error"`, `"forbidden"` for
  `GuardRejected` (whose `Display` names the guard), `"the application is shutting down"` for
  `Closed` (whose `Display` names a key) [T 3].
- A reply probe converted twice answers an `Internal` `CallError` rather than panicking; generated
  code converts each once [T 16].

### SSE [H 17, spine 60, HM 12]

- Headers: `Content-Type: text/event-stream; charset=utf-8` and `Cache-Control: no-cache`, the
  latter not in the design.
- Encoding: `name: value`; comments first, then `event`, `id`, `retry` (milliseconds), then data.
  Data and comment text split at CR, LF and CRLF; the piece after a trailing terminator is kept, so
  `"a\n"` round-trips. Empty data is one `data: ` line, dispatched as `""`; an event without data
  has no `data` line. A keep-alive frame is `: keepalive` and a blank line (T10).
- The late path: an `Err` item runs `dispatch_late` with the matched handler, boxed in the body.
  `Render(e)` writes `e`; `Ignored` logs at `warn` and writes the original's `summary()`; `End`,
  like the stream returning `None`, writes the end event if one is set and completes. The
  mid-stream error form, which the design leaves unstated, is an event named `error` carrying the
  envelope, after which the stream ends; its envelope always carries `details`, an empty array when
  there are none. A written error event reports `CutOff(None)` first; `Tracked`'s later
  `Completed` changes nothing.
- An `Sse` an error handler returns on a miss has no matched handler: its item errors are written
  as they are, no error handlers run.
- HTTP handlers declare no `Shape`; an SSE handler is `Shape::Unary`, since telling it apart would
  read the return type's spelling, which the design rules out, and nothing in HTTP reads the shape.

### The pipeline [P]

- Two heads [P 4]: the `RequestHead` input is seeded at `Execution::open` with the head as the
  client sent it. The `HttpCx` that `dispatch` receives is built from the request as the stage
  leaves it: a rewritten path, a header a middleware set, and a header a tower layer changed reach
  the extractors. An entry's failure is offered with a context built from the head as that sub-step
  received it, no body, no upgrade. In the scoped sub-step it carries the matched route and the
  handler's tiers run before the global ones; in the unscoped sub-step only the global ones run,
  even when a route matches later.
- The continuation through a tower layer: T9. `ulo_http::Service` accepts any `Bytes` body with
  error `Infallible`; a layer's service may change the response body type and its error converts to
  `BoxError`, which a body-wrapping layer such as `RequestBodyLimitLayer` needs [spine 56]. tower's
  `util` feature is not needed: each request drives its own clone of the layered service through
  `poll_fn` over `poll_ready` then `call` [P 1].
- The in-flight `Permit` and an `ExecutionRef` sit in the outermost response body and drop when
  the backend drops it: a streaming response, SSE included, counts against `max_inflight` and
  keeps its execution open for as long as it streams [P 6].
- `Disconnected` fires when the outermost response body drops before its end: a `None` frame, a
  frame after which `is_end_stream()` holds, or an error frame (the server cutting the body). A body
  the backend never writes (a `HEAD` answer, a 1xx, 204 or 304) counts as ended from the start
  [P 7].
- CORS [P 14]: a preflight is `OPTIONS` carrying `Origin` and `Access-Control-Request-Method`; from
  an origin not allowed it is still 204 with `Vary` and no allow headers. A preflight's `Vary`
  lists `Origin`, `Access-Control-Request-Method` and `Access-Control-Request-Headers`; every other
  response passing through gets `Vary: Origin`, requests without `Origin` included; a `Vary`
  already naming it, or `*`, is left alone. Origins compare byte for byte.
  `Access-Control-Allow-Methods` is sent only when methods are configured; `allow_any_header`
  reflects `Access-Control-Request-Headers`. A `Cors` allowing any origin with credentials that
  escaped `prepare` sends no allow headers at all.
- The call span [P 13, spine 40, T 15]: created at `Execution::open` with the method alone as
  `otel.name` (the `tracing` span's static name is `"call"`, level INFO, every transport's fields
  declared `Empty`). After routing `otel.name` is recorded again as `GET /users/{id}`,
  `http.route` as the pattern, `ulo.handler` as `Controller::method`; `url.scheme` is `https` under
  TLS; `url.path` is the path as sent; the status is recorded after the pipeline answers.
- `exclude` on an unscoped entry is matched against the request path as that entry receives it,
  through `ScopePattern::covers_path`, since no route is known yet [P 11].

### Sockets, TLS and the server [N, HM]

- Socket activation [N 1–5, spine 44]: `Activation::get()` reads the environment once per process.
  No `LISTEN_PID` or no `LISTEN_FDS` is no sockets; `LISTEN_FDS=0` is no sockets; a count above
  `i32::MAX - 3`, a `LISTEN_FDNAMES` that is set without one name per descriptor, or a variable
  that is not UTF-8 is `Malformed`; a mismatch is `PidMismatch` (T11). `FD_CLOEXEC` is set on every
  descriptor first. A descriptor passes when `SO_TYPE` is `SOCK_STREAM`, its local address is IPv4
  or IPv6, and `SO_ACCEPTCONN` is set (BSD fallback: a nonzero bound port, T13). Every descriptor
  is checked through a `BorrowedFd` before any is adopted, so a `LISTEN_FDS` that overstates the
  count and reaches the runtime's own descriptor is refused without closing it. One failing
  descriptor fails the whole activation as `NotListening { index }`, adopting none; `prepare` asks
  only `contains`, a `bool` that cannot carry the reason. Filed as F306: a UDP socket beside TCP
  listeners fails every server, and 2b's UDP link needs slots of either kind. A name answers for
  every descriptor carrying it and `take` hands over the first untaken, so a systemd unit
  listening on IPv4 and IPv6 passes two descriptors named alike and a server lists the endpoint
  twice. Off Unix `Activation::get()` is `Unsupported`, and an inherited endpoint fails `prepare`
  with "socket activation is supported on Unix only"; `Endpoint::Addr` is unaffected.
- TLS [N 6, spine 45]: rustls with the `ring` provider passed explicitly (the process default
  panics when both backends are enabled and none installed), TLS 1.2 and 1.3, one certificate
  chain, no client authentication. Filed as F307: gRPC's mTLS guard in 2b needs client
  certificates. The key match is rustls's own; a PEM error from a file names the file. `Tls::load`
  takes `&[&[u8]]`, so literals of different lengths need `.as_slice()` [N note P3].
- Connections [HM 1, 2, 3, 4, 5, 9, 10]: one accept loop per listener, each a spawned task, one
  task per connection in that loop's `JoinSet`. TLS connections, and plain ones with h2c on, go
  through `auto::Builder`; plain ones with h2c off through `http1::Builder`; upgrades enabled. A
  failed handshake is logged at `debug` with the peer and the connection dropped; the accept loop
  is unaffected. `ConnInfo::peer` is always set; `ConnInfo::local` is the accepted socket's own
  address (the interface address under a `0.0.0.0` listener), unset only if the OS refuses it;
  `ConnInfo::tls` carries the negotiated ALPN protocol and the SNI name. `Request::upgrade` is
  `Some` exactly when hyper stored an upgrade future, which it does for an HTTP/1.1 request asking
  to upgrade, and `None` on every other request although the backend declares `upgrades: true`.
- Shutdown [HM 7, HM 8, P 15]: `drain` raises the signal and resolves once every connection's
  graceful shutdown has completed; GOAWAY, the idle close and `Connection: close` come from hyper's
  `graceful_shutdown`. The core drops a drain still running at its deadline, and `close` aborts
  what is left through `JoinSet::shutdown`, so `close` does not cut a response still being written
  inside the window. `serve` before `bind` is `Err`; `drain` and `close` before `bind` do nothing;
  `drain` or `close` before `serve` closes the listeners and a later `serve` answers `Ok(())`.
- `shutdown_signal()` installs its handlers at the future's first poll, inside the runtime, so
  calling it outside one does not panic; a signal between the call and the first poll takes the OS
  default; a handler that fails to install is skipped, and with none installed, or off Unix and
  Windows, the future never resolves. On Unix tokio keeps the handler for the rest of the process,
  so a second SIGINT during shutdown does not end it [N 7].
- `Admission::new(Option<usize>).retry_after(d)`, `try_admit() -> Option<Permit>`,
  `connection(Option<usize>)`: over a limit the call is refused at once [spine 39].

## 4. Diagnostics and error text

- Key assertion: "`htpp` is not the key of any handler's transport in this impl (handlers: get,
  get_rpc)"; "(no handlers)" for an impl with none; names printed without `r#` [MX, message texts].
- Body consumer: "`user` and `login` both consume the body; a handler reads the body once"; a
  destructuring pattern is named by its tokens (`Path (id)`) with `{` and `}` doubled [MX, message
  texts].
- Receiver: "a handler takes `&self`, or `self: Arc<Self>` for a reply that outlives the call"
  [MX 5]. Generic handler: T4's text [MX 1].
- `K: with = ..` with a role key: "a role key takes contributions, and `K: with = ..` binds a
  single instance; a global enhancer is written `into AnyGuard<Http>: [with = ..]`" [spine 25].
- The HTTP attribute [HM 13]: no argument, "`#[get]` takes the route pattern, as in
  `#[get("/users/{id}")]`"; no `__handler` attribute, "`#[get]` goes on a method of a `#[routes]`
  impl, which hands it the handler's enhancers"; a bad literal, "invalid route pattern: {reason}"
  on the literal. The macro and the router check the same grammar with the same reason texts in
  the same order, so a literal refused at compile time and a prefix-joined pattern refused at
  `prepare` read alike; one trailing slash is dropped before the checks, so `/files/{*rest}/`
  passes [H 3, HM 11, spine 62].
- Wiring reports: ``UsersController::get (param #1)`` and ``UsersController::get_rpc (Rpc) →
  Dep<RequestHead> (param #1)`` (T12) [CX 5]; `InputConflict`'s three line forms, a binding
  reported once per key (T21) [CX 7].
- `#[derive(Validate)]` [T 10, T 11]: `length` or `range` with neither bound, a bound written
  twice, `email` with arguments, and an unknown rule are span errors; a bound on a type without
  `Display` does not compile (descriptions: "length must be between 1 and 64", "must be at least
  13", "must be an email address"); `range` is `!(value >= min)`, so NaN fails both bounds; an
  enum, union, tuple or unit struct is refused on the type's name; a serde attribute form the
  derive does not follow counts as absent, and serde reports it.
- `#[derive(Classify)]` [T 12, spine 38]: a second `#[classify]` on one item, an unknown kind
  word (the message lists the eleven), a union, and a struct with no kind are span errors; every
  variant left without a kind is reported in one compile, each on its name.
- `email` [T 8]: exactly one `@`; a local part of 1 to 64 bytes with no whitespace or control
  character; a domain of two or more labels of letters, digits and `-`, none starting or ending
  with `-`; 254 bytes in all; non-ASCII letters pass.
- `validator` bridge descriptions [T 7]: the rule's message when set, otherwise ``failed the
  `<code>` rule (k = v, ..)``; the `value` parameter is omitted since it may be a secret; nested
  paths join with `.`, list items are `items[0]`, sorted by field.
- Route errors: `Router::build` returns every failure as a `Vec<RouteError>`, each a sentence
  through `Display` naming handlers as `Controller::method`; `PatternError` and `RouteError`
  implement `Display` [H 19, H req 3]. A `Path<T>` mismatch lists both sides [H 10].
- Pre-dispatch refusals [P 11]: a stray `exclude` records its caller (`#[track_caller]`) and
  `prepare` reports it; `apply_for` or `layer_for` with an empty pattern list; a scope or exclusion
  that does not parse, naming the entry's location.
- Inherited-socket shortfall: the name, how often it is listed, how many descriptors answer
  [P 10]. Off Unix: "socket activation is supported on Unix only" [N 5].
- Log levels: a failed TLS handshake at `debug` [HM 3]; a tower layer's second call at `error`
  [P 1]; an `Ignored` late outcome, a bad 401 challenge and a `Details` that fails to serialize at
  `warn` [H 15, H 17].
- A dependency lookup's text, which names keys, never reaches the client [T 4].

## 5. Internal only

### Core SPI, placed where the core's signatures name it [spine 1, 3, 5, 7, 11, 12, 14, 15, 19, 27]

- `Metadata`, `MetaTier`, `Shape`, `HandlerInfo` and `BoundAddr` live in `ulo`; `ulo-transport`
  and `ulo-net` re-export. `Shape` is `#[non_exhaustive] { Unary, ServerStreaming,
  ClientStreaming, Bidi }` with no binary flag, which is RPC's and 2b's.
  `BoundAddr::new(transport, addr).tls(..)`, `#[non_exhaustive]`.
- `Mount::once<K: ?Sized + 'static>(&mut self, f: impl FnOnce(&mut Mount<'_>))`, keyed by
  `TypeId` within one controller's mount.
- `ExecutionRef::report_stream_end(outcome)` is public, for `Tracked`; `dispatch_late(handler,
  exec, cx, err)` takes the execution as `dispatch` does; `recover(..)` and
  `AppHandle::catch_panic(stage, fut)` are new; `DispatchStage::PreDispatch`.
- `pub const fn key_in(key: &str, keys: &[&str]) -> bool`. `Inputs::input::<T>() -> &mut Self`
  records the call's location.

### The macro protocol [spine 21, 22, 23, 24; MX 2, 3, 4, spans, generic form, review]

- The transport attribute emits `const __ULO_CHECKS_<name>: () = { .. };` and `#[routes]` forces
  it with `const _: () = <Ctrl>::__ULO_CHECKS_<name>;`, since an attribute on an impl item can emit
  only impl items. A controller with any generic parameter (a lifetime included) takes the generic
  form: associated consts, read at the top of `Controller::mount` as
  `let () = Self::__ULO_KEYS_CHECK_<key>;` and `let () = Self::__ULO_CHECKS_<name>;`, evaluated
  when `mount` instantiates.
- `__enhancer_specs!(<Transport>, "<key>", <shared ident>, <__handler tokens>)`; a controller-tier
  `value` becomes `spec.<role>_arc(Arc::clone(&shared.0.N))`, both fields as `Index` tokens (the
  template's `#shared.0.#index` lexed `0.` as a float). `value` positions are counted alike in
  `shared::shared_values` and `__enhancer_specs!`: every `value` across the impl's three
  attributes in order, other transports' entries included.
- `ulo-macros` depends on `ulo-handler-codegen`; the `__handler` grammar (`EnhancerAttr`, `Entry`,
  `Form`, `Role`, `ScopeArg`, `HandlerTokens`) moved there. Every entry form round-trips; `==` and
  `=>` are excluded before a `key = ..` form; `meta(controller(..), method(..))` round-trips with
  either list empty; raw identifiers round-trip through `Ident::parse_any` and generated names use
  the unraw text.
- Providers-list closures lower through `factory::{FallibleBinding, PlainBinding}` with
  `wrap_async`; `ProviderScope` replaces `Fallible`/`Plain::register_singleton`.
- On a generic impl, `#[routes]` rewrites each handler's opaque returns to `+ use<T, N, ..>` (type
  and const parameters, lifetimes omitted) through the new
  `reply::rewrite_opaque_returns_in(sig, impl_generics)`; the attribute's `rewrite_opaque_returns`
  then leaves them. An opaque type is left as written when a bound names a `use<..>`, a lifetime
  other than `'static` (`'a`, `'_`, `Item = &'a str`), or an elided reference (`Item = &str`);
  lifetimes under `for<..>` and in `Fn(&str)` sugar are late-bound and do not count; `'static` is
  no capture; nested opaque types are judged separately.
- `emit::mount_param_ident()` names the mount function's `&mut Mount<'_>` parameter (`__ulo_m`)
  for 2b's `Mount::once::<Self>(..)` call in a WebSocket `handler_value`; nothing in 2a calls it.
- Spans: `__ULO_KEY_<name>` and the names of `__ULO_CHECKS_<name>` and `__ulo_mount_<name>` at the
  handler's name; each pairwise `assert!` at the pair's second parameter's type; a shared value's
  role bound at the handler's name (a value lacking the role fails E0277 at `Controller::mount`'s
  call); extraction and dependency reads at the parameter's type, where `Param`'s
  `on_unimplemented` note is placed; the method call at its name; `#[meta]` at each expression; an
  impl-level `value` at its expression. The locals (`__ulo_cx`, `__ulo_argN`, `__ulo_this`,
  `__ulo_out`, `__ulo_meta`, `__ulo_dependencies`, `__ulo_m`, `__ulo_shared`, `__ulo_call`) are
  mixed-site.

### `ulo-transport` internals [spine 30, 34, 37, 40; T 13, 15, 16]

- The reply probe is `(&&&::ulo_transport::__private::IntoReplyProbe::<Http, _>::new(out))
  .into_reply(&cx)`, reached through the transport crate's re-export: the arms need
  `V: IntoReply<T>` in where-clauses, which lookup checks where it never checks a method's, and
  they name `CallError`, which the core cannot. Its value sits in a `Cell<Option<_>>`.
- The impls bound `T::Cx: AsRef<ExecutionRef>`; `HttpCx` implements it beside `exec()`.
- Generated handler code reaches `ulo_transport` through the transport crate's
  `__private::transport` re-export; the derives name `::ulo_transport` directly.
- `__private::validate::{LengthProbe, ViaChars, ViaItems, is_email}`, named by
  `#[derive(Validate)]` rather than repeated per derived impl.
- `span::call(transport, name, handler) -> Span`: INFO, static name `"call"`, `otel.name` as the
  display name, every transport's fields declared `Empty`, `ulo.handler` recorded when `Some`.

### HTTP internals [spine 53, 56, 58; H 8, H 19; P 1, P 12; HM 2, HM 10]

- `BackendLimits { upgrades, h2c, inherited_sockets, port_zero, tls }`, `BackendLimits::NONE` and
  const setters. `Axum` is `pub struct Axum` (`Default`) with no documented limit [spine 63].
- `trait UpgradeHandler { paths(&self, &AppHandle); upgrade(&self, Request) }` and `Upgrades`
  module metadata are the 2b hand-off point.
- `Stage::unscoped` is removed; `ScopedStage::steps` kept; `scoped_for` compares by `Arc::ptr_eq`
  on the `PreDispatch` and the entry index, so routes with the same entries share one stage. `Entry`
  gains a crate-private `scoped` flag. `middleware::Next::new` is crate-private; the `Continuation`
  lives in `http::Extensions` as a take-once slot.
- Crate-private additions: `Display` on `PatternError` and `RouteError`; `pattern::{normalize,
  split}`, `Pattern::{match_split, precedence}`, `RouteEntry::allow`, `HttpBody::tracked`,
  `render::timed_out`, `EventName::error`. `ScopePattern` has two private fields and is built by
  `ScopePattern::parse`, as `pre_dispatch.rs` already does [H req 2].
- The pattern grammar is parsed twice, in `ulo-http-macros/src/pattern.rs` and
  `ulo-http/src/router/pattern.rs`, since the macro crate cannot depend on the crate that
  re-exports it; a doc comment on each names the other [spine 62, HM req].

### Dependencies [spine 45, spine 64; HM 1; H 9]

- Workspace entries added: `async-lock`, `axum`, `futures-core`, `headers`, `http-body`, `hyper`,
  `hyper-util`, `multer`, `percent-encoding`, `pin-project-lite`, `proc-macro2`, `quote`, `rustls`
  (the `ring` provider), `rustls-pki-types`, `serde_urlencoded` (now unused, T13), `socket2` (for
  `FD_CLOEXEC` and the socket-type check), `syn`, `tokio-rustls`. `ulo-macros` takes `proc-macro2`,
  `quote` and `syn` from the workspace.
- `ulo-http-axum`: `hyper-util` gains `server-auto`, `tokio` gains `time`; `tower` and
  `http-body-util` are removed from the crate (the workspace entries stay for `ulo-http`).
  `ulo-http` drops `serde_urlencoded`.

## 6. Applied by the orchestrating session, and the open compile note

Applied after the areas finished:

- `ulo_net::Activation::count(&self, name: &ListenerName) -> usize`, the untaken descriptors a
  name answers for, so `prepare` refuses a name listed more often than it was inherited (P's N1)
  [N, P 10]. Additive.
- `HttpCx::head`'s doc: the head as the pre-dispatch stage left it, while the seeded
  `Dep<RequestHead>` is the head as the client sent it (P's H1) [H, P 4]. Doc only.

Cross-area requests the logs close themselves: H's `HEAD` 204/304 exception → P 8; HM's
`Request::upgrade` doc → H 18; HM's `Backend::drain` doc → P 15; H's `ScopePattern::parse` → already
so in `pre_dispatch.rs`; T's `Retry-After` rounding → H 15; HM's pattern parity → H 3. Still open:
CX R1 (T12), N R1 and H req 5 (T13), P H2 (T14), H req 4 (T25), MX 7's fix (T5).

Open at the first compile:

- `Cargo.lock` carries an uncommitted modification no log reports making. No area ran cargo (plan
  rule 1), and three manifests changed by hand: `ulo-http` drops `serde_urlencoded`;
  `ulo-http-axum` adds `server-auto` and `time` and drops `tower` and `http-body-util`. The
  lockfile is behind the manifests whatever the change is. Let the first `cargo build` rewrite it
  and review that diff rather than keeping the unexplained one.
- T25: whether `recover`, `dispatch_late` and `MountedHandler<Http>` satisfy the `Send` bounds the
  SSE body and `AppService::call` assume.
- T5 and T13 are recommended before the compile; T12 and T14 after it.
