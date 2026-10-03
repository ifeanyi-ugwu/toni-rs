# Divergences: the race 2a spine

Every place the race 2a skeleton departs from `transports/DESIGN.md` or `DESIGN.md`, or fills in a
public shape they leave unspecified. Each entry gives what the design says, what the spine writes,
and why. All await the user's sign-off.

The `fw` → `ulo` rename is applied throughout without an entry each: crate names, `__FW_KEY_<name>`
as `__ULO_KEY_<name>`, `__FW_KEYS_CHECK_<key>` as `__ULO_KEYS_CHECK_<key>`, and the span attributes
`fw.transport`, `fw.handler` and `rpc.system = "fw"` as `ulo.transport`, `ulo.handler` and
`rpc.system = "ulo"`.

## Core

### 1. `Metadata`, `MetaTier`, `Shape` and `HandlerInfo` live in the core

- **Design:** §1 lists `Metadata` in `fw-transport`; X3 gives `HandlerSpec::meta(Metadata)` and
  `.shape(Shape)` on a core type.
- **Spine:** all four are in `ulo` (`transport/{metadata,handler}.rs`); `ulo-transport`
  re-exports `Metadata` and `MetaTier`.
- **Why:** the core's `HandlerSpec` and `HandlerInfo` carry them, and the core cannot depend on
  `ulo-transport`.

### 2. One `Metadata` value carries both tiers

- **Design:** `.meta(Metadata)`; `info.meta::<T>()` answers the method's declaration over the
  controller's; it does not say how two tiers fit one argument.
- **Spine:** `Metadata::{controller(v), method(v)}`, `get` (most specific, first written within a
  tier), `get_all` (method first), `entries() -> (type name, MetaTier)`.
- **Why:** keeps `.meta(..)` one call while `HandlerInfo` still tells the tiers apart.

### 3. `Shape` records the four call shapes and no binary flag

- **Design:** §5.2 refuses a binary payload on a text-codec link "from the shape recorded on its
  `HandlerSpec`".
- **Spine:** `#[non_exhaustive] enum Shape { Unary, ServerStreaming, ClientStreaming, Bidi }`,
  `Unary` by default.
- **Why:** the binary payload belongs to RPC, which is race 2b; `#[non_exhaustive]` leaves room for
  the field or variant 2b adds.

### 4. The `.at(prefix)` prefix is stored, not joined

- **Design:** X3: "a runtime value applied to every route and gateway path of that controller".
- **Spine:** `ModuleDef::controller::<C>()` returns `ControllerHandle<'_>` with
  `at(impl Into<Cow<'static, str>>)`; the core records the prefix on each `HandlerInfo`
  (`prefix()`, beside `route()`), and the transport joins the two by its own path rules.
- **Why:** joining is a path rule the core does not own; an RPC pattern takes no prefix.

### 5. `Mount::once`'s shape

- **Design:** X15 names `Mount::once::<K>(..)`, idempotent per controller type.
- **Spine:** `once<K: ?Sized + 'static>(&mut self, f: impl FnOnce(&mut Mount<'_>))`, keyed by
  `TypeId::of::<K>()` within one controller's mount.

### 6. `Server::prepare` has a default body; `bind` keeps its parameter

- **Design:** X6 adds `prepare(&mut self, Mounted<'_, T>)`; it does not say whether `bind` still
  receives the handlers.
- **Spine:** `prepare`'s default answers `Ok(())`; `bind(&mut self, Mounted<'_, T>)` is unchanged.
- **Why:** a server with nothing to check needs no body, and an unchanged `bind` keeps the rest of
  the SPI as built.

### 7. `BoundAddr` lives in the core

- **Design:** §1 puts bound-address reporting in `fw-net`; X6 gives `Server::bound -> Vec<BoundAddr>`.
- **Spine:** `#[non_exhaustive] struct BoundAddr { transport, addr: SocketAddr, tls }` with
  `BoundAddr::new(transport, addr).tls(..)` in `ulo`; `ulo-net` re-exports it.
- **Why:** the core trait's signature names it.

### 8. `ConfigureErrors` and `ConfigureError`

- **Design:** `StartupError::Configure(ConfigureErrors)`, one entry per failure carrying
  `{ transport, source: Redacted }`, mirroring `WiringErrors`.
- **Spine:** `ConfigureErrors` with `iter`, `len`, `is_empty` and a report `Display`;
  `#[non_exhaustive] struct ConfigureError { transport, source }`. Each server's `prepare` error is
  one entry.
- **Why:** the design's shape; the method set is `WiringErrors`'.

### 9. `listen()` refuses a missing `Timer` before any `prepare`

- **Design:** silent on the order of the `TimerMissing` check and `prepare`.
- **Spine:** `TimerMissing` first (as `StartupError::Bind`), then every `prepare`, then every
  `bind`.
- **Why:** with no `Timer`, `prepare` would receive no clock (`Mounted::timer`).

### 10. `StreamOutcome::CutOff` carries an `Option<CancelReason>`

- **Design:** `Completed` or `CutOff(reason)`.
- **Spine:** `#[non_exhaustive] enum StreamOutcome { Completed, CutOff(Option<CancelReason>) }`.
- **Why:** a stream cut off by an error item, or dropped by a transport that recorded nothing, has
  no cancellation reason.

### 11. `ExecutionRef::report_stream_end` is public

- **Design:** `on_stream_end` is "a callback the core runs synchronously when the reply stream
  finishes", driven by `Tracked<S>` in `fw-transport`.
- **Spine:** `ExecutionRef::report_stream_end(outcome)` runs the callbacks once; a later report
  changes nothing; a callback registered after the end runs at once with the recorded outcome.
- **Why:** `Tracked` is outside the core, so its entry point must be public.

### 12. `dispatch_late` takes the execution

- **Design:** `fw::dispatch_late(handler, cx, err) -> LateOutcome`.
- **Spine:** `dispatch_late(handler, exec: &ExecutionRef, cx, err)`, as `dispatch` takes it.
- **Why:** the core cannot reach the execution through an opaque `T::Cx`, and it sets `is_late` on
  it.

### 13. `LateOutcome::{Render, Ignored, End}`

- **Design:** an `Err(e2)` renders `e2`; an `Ok` "is ignored and logged, and the original error
  renders canonically"; `Err(EndStream)` ends the stream.
- **Spine:** `Render(BoxError)` (the original unclaimed, or the reshaped error), `Ignored`, `End`.
  The transport logs `Ignored` and renders the canonical form it kept before the call, through
  `CallError::summary()`.
- **Why:** an error handler takes the error by value, so after an `Ok` the core no longer holds the
  original; and the core has no logger.

### 14. `ulo::recover` is a new core function

- **Design:** §3.3: both pre-dispatch sub-steps run inside the error chain, and on a miss only the
  global error handlers apply. No SPI item is named for it.
- **Spine:** `recover(handler: Option<&MountedHandler<T>>, exec, cx, err) -> Result<T::Reply, BoxError>`:
  the handler's tiers then the global ones when a route matched, the global ones alone when none
  did.
- **Why:** the stage runs outside `dispatch`, and only the core walks the error handlers.

### 15. `AppHandle::catch_panic` and `DispatchStage::PreDispatch`

- **Design:** a pre-dispatch entry's panic reaches the error handlers as `PanicRecovered`.
- **Spine:** `AppHandle::catch_panic(stage, fut)` turns a panic into `PanicRecovered { stage, .. }`
  with the message redacted; `DispatchStage` gains `PreDispatch`.
- **Why:** `PanicRecovered` is constructed by the core alone.

### 16. `AppHandle::root()`

- **Design:** X8: the execution opens with root visibility and switches at `route_to`.
- **Spine:** `AppHandle::root() -> ModuleRef`.
- **Why:** no public route to the root module existed besides naming its type.

### 17. `Execution::route_to` returns nothing

- **Design:** "callable once, by the holder only, before `dispatch`".
- **Spine:** `route_to(&self, &ModuleRef)`; the first call wins, and a later call or a module of
  another app changes nothing.
- **Why:** the core does not panic, and a `Result` for a holder's own misuse would burden every
  transport's call site.

### 18. `Phase`

- **Design:** X13: `AppHandle::phase()` (Running, Stopping, Draining, Destroying, Closed).
- **Spine:** `#[non_exhaustive] pub enum Phase` with those five, ordered; the phases before a
  handle exists read `Running`. The core's internal phase enum keeps its own name inside the crate.

### 19. `Transport::inputs` and the input's origin

- **Design:** `fn inputs(d: &mut Inputs) {}`; a transport origin prints "declared by transport
  `Http`".
- **Spine:** `Inputs::input::<T>() -> &mut Self`, recording the call's location; the freeze records
  each key with `InputOrigin::Transport { name, at }` in place of the declaring module.

### 20. Impl-level values travel in `ulo::__private::Shared<(Arc<V0>, ..)>`

- **Design:** X2: a generated `__FwShared<V0, V1, ..>` struct.
- **Spine:** a core-provided `pub struct Shared<T>(pub T)`, `T` a tuple of `Arc<Vi>`, `Shared(())`
  with no value; the per-handler mount function takes `&Shared<(Arc<__UloV0>, ..)>`.
- **Why:** a struct `#[routes]` generates needs a name unique per module, and the transport
  attribute sitting on a method cannot learn the controller's type name to spell it.

### 21. The checks constant: a protocol obligation the design does not name

- **Design:** §2.2: the transport attribute emits one free `const _` assertion per parameter pair.
- **Spine:** the transport attribute emits `const __ULO_CHECKS_<name>: () = { .. };` holding the
  pairwise assertions, and `#[routes]` emits `const _: () = <Ctrl>::__ULO_CHECKS_<name>;` per
  handler (read from `mount` for a generic controller).
- **Why:** an attribute on an impl item can emit only impl items, where a constant needs a name and
  is evaluated only where read; `#[routes]` is outside the impl and can force it, as it does for X1.

### 22. `__enhancer_specs!` takes the shared-value parameter

- **Design:** silent on how the transport attribute's spec builder reaches the impl-level values.
- **Spine:** `__enhancer_specs!(<Transport>, "<key>", <shared ident>, <__handler tokens>)`; a
  controller-tier `value` entry becomes `spec.<role>_arc(Arc::clone(&shared.0.N))`, its role
  checked by the mount function's bound.

### 23. `ulo-macros` depends on `ulo-handler-codegen`

- **Design:** §1: every transport's attribute macro calls `fw-handler-codegen`.
- **Spine:** `#[routes]` calls it too, and the `__handler` grammar (`EnhancerAttr`, `Entry`, `Form`,
  `Role`, `ScopeArg`, `HandlerTokens`) moved there from `ulo-macros`.
- **Why:** one parser of the protocol for its writer and its readers.

### 24. Every providers-list closure is wrapped as an `into` entry's

- **Design:** §4: `with = closure` and the bare closure are `Auto`; an `into` entry's closure is
  wrapped (a synchronous body in `async move`).
- **Spine:** `with`, `with(scope)`, `K: with` and the bare closure all lower through
  `factory::{FallibleBinding, PlainBinding}` with `wrap_async`, so a synchronous-bodied closure
  that failed to compile before now compiles. `FallibleBinding`/`PlainBinding`/`ProviderScope`
  replace `Fallible`/`Plain::register_singleton`.
- **Why:** "the same closure means the same thing in every position" (§3.3).

### 25. `K: with = ..` refuses a role key

- **Design:** silent.
- **Spine:** a `K` spelled as a role key is a span error, as `X as <role key>` is: "a role key takes
  contributions, and `K: with = ..` binds a single instance; a global enhancer is written
  `into AnyGuard<Http>: [with = ..]`".

### 26. `EndStream` is not `#[non_exhaustive]`

- **Design:** DESIGN §10.2 makes every public error type `#[non_exhaustive]`.
- **Spine:** `pub struct EndStream;` with `Default`.
- **Why:** an error handler constructs it (`Err(EndStream)`), which `#[non_exhaustive]` forbids
  outside the core.

### 27. `key_in`'s signature

- **Design:** X11: a const fn comparing bytes.
- **Spine:** `pub const fn key_in(key: &str, keys: &[&str]) -> bool`.

## `ulo-transport`

### 28. Parameters are read through marker inference, and the core's injection points implement no `FromCall`

- **Design:** §2.2: autoref ranks, `P: FromCall<T>` first and `S: FromContainer` as `Injected<S>`
  second; `fw-transport` implements `FromCall<T>` for `Dep`, `Many`, `Ext`, `ModuleRef` and
  `ExecutionRef`; the pairwise assertion reads `<P as FromCall<T>>::CONSUMES_BODY`.
- **Spine:** `__private::Param<T, M>`, implemented for every `FromCall<T>` type (`ViaCall`) and
  every `FromContainer` type (`ViaContainer`); generated code names `<P as Param<T, _>>` for the
  dependencies, the extraction and the assertion. The core's injection points reach a handler
  through `FromContainer` alone. `Injected<S>: FromCall<T>` stays for generic code.
- **Why:** the assertion sits in a constant, where autoref ranking does not run, and
  `<UserType as FromCall<T>>` does not compile for a bare container type. Marker inference works
  in a constant and needs no type to implement both traits. `Option<Dep<U>>` behaves as designed
  through the core's `Option<S>`.

### 29. `ExtractError::Dependency`

- **Design:** five variants: `Missing`, `Malformed`, `UnsupportedMediaType`, `TooLarge`, `Invalid`.
- **Spine:** a sixth, `Dependency { param, source: LookupError }`, `param` the container type's
  name; it classifies as `CallError::from_boxed` maps its lookup error.
- **Why:** a container parameter's failure is a `LookupError`, `FromCall`'s error is
  `ExtractError`, and a constructor's "tenant not found" must still render 404.

### 30. The reply probe's shape and path

- **Design:** `(&&&::fw::__private::IntoReplyProbe::new(out)).into_reply::<Http>(&cx)`.
- **Spine:** `(&&&::ulo_transport::__private::IntoReplyProbe::<Http, _>::new(out)).into_reply(&cx)`,
  reached through the transport crate's re-export.
- **Why:** the arms need `V: IntoReply<T>` among their impl's where-clauses, which lookup checks
  where it never checks a method's; and the arms name `CallError` and `IntoReply`, which the core
  cannot.

### 31. `CallError`'s accessors and `summary`

- **Design:** `new`, `unauthorized`, `from_boxed`, `with_details`, `kind`, `source_as`.
- **Spine:** also `with_source`, `message`, `details`, `challenge`, and `summary()` (a copy without
  the source, for the late path, entry 13). `CallError::grpc_code` (§13.2) is left to 2b.

### 32. `ErrorKind::as_str`, `Details`, `FieldViolation`, `Link`

- **Design:** the kinds' envelope strings; `Details` as a list; the `Detail` variants.
- **Spine:** `ErrorKind::as_str()` gives `"bad_request"` and kin; `Details::{new, push, with, iter,
  len, is_empty, retry_after}`; `FieldViolation { field, description }`; `Link { description, url }`;
  the JSON form an array of objects tagged by `"type"`, `retry_after` in whole seconds.

### 33. `IntoReplyError::new` and its public message

- **Design:** classified `Internal`, holding the cause as its source.
- **Spine:** `IntoReplyError::new(impl Into<BoxError>)`; its public message is "internal error".

### 34. A transport's `Cx` implements `AsRef<ExecutionRef>`

- **Design:** generic code reads `cx.exec()`, which `Transport::Cx` does not declare.
- **Spine:** the impls in `ulo-transport` bound `T::Cx: AsRef<ExecutionRef>`; `HttpCx` implements
  it beside its inherent `exec()`.
- **Why:** std's conversion trait, rather than a new one.

### 35. `Valid<P>` requires `P: Validate`

- **Design:** `Valid<P>` runs `Validate::validate` after `P` extracts; `Validate` is derived on the
  inner type (`NewUser`).
- **Spine:** `impl<T, P: FromCall<T> + Validate> FromCall<T> for Valid<P>`; `ulo-http` implements
  `Validate` for `Json`, `Form`, `Query` and `Path` by delegating to the value inside. `Invalid`'s
  `param` names `P`.

### 36. The `validator` bridge

- **Design:** "a bridge for the `validator` crate sits behind a feature flag".
- **Spine:** the `validator` feature exposes `validator_bridge::violations(&ValidationErrors)`.

### 37. The derives name `::ulo_transport`

- **Design:** silent on the path generated code takes.
- **Spine:** `#[derive(Classify)]` and `#[derive(Validate)]` emit `::ulo_transport::..`, so a crate
  deriving either depends on `ulo-transport`; what the handler attributes generate reaches it
  through the transport crate's `__private::transport` re-export instead.

### 38. `#[derive(Validate)]` and `#[derive(Classify)]` grammars

- **Design:** shows `length(min, max)`, `email`, `range(min)`; `#[classify(not_found)]`.
- **Spine:** those three rules, either bound optional, `email` a simple shape check rather than
  RFC 5322; `#[classify(..)]` per variant or on the enum as a default, a variant left without a
  kind a span error.

### 39. `Admission`'s API

- **Design:** a runtime-free semaphore on `async-lock`, one counter per server and one per
  connection.
- **Spine:** `Admission::new(Option<usize>).retry_after(d)`, `try_admit() -> Option<Permit>`,
  `connection(Option<usize>) -> ConnectionAdmission`; over a limit the call is refused at once.

### 40. `span::call`

- **Design:** `fw_transport::span::call(..)`; names and attributes per OpenTelemetry.
- **Spine:** `call(transport, name, handler) -> Span`, declaring every transport's fields empty;
  the display name is the `otel.name` field, since a `tracing` span's name is static.

### 41. `Tracked::new(inner, exec)` and `Option<P>`'s `None`

- **Spine:** `Completed` when the inner stream returned `None` before the wrapper drops; the
  transport drops it after writing the stream's end. `Option<P>` is `None` exactly on
  `ExtractError::Missing`.

## `ulo-net` and `ulo-tokio`

### 42. `Tls::from_pem_files` returns a `Tls`, parsed by `load` in `prepare`

- **Design:** §0.6 cites `Tls::from_pem_files` as a fallible conversion returning `Result`; §2.7
  parses it in `prepare`, so an unreadable file fails as `Configure`.
- **Spine:** `from_pem_files(cert, key) -> Tls` and `from_pem(cert, key) -> Tls` record their
  source; `Tls::load(alpn) -> Result<TlsAcceptor, TlsError>` reads and parses.
- **Why:** §2.7's behaviour, which a `Result` in `main` would contradict.

### 43. `EndpointSpec`

- **Design:** `Endpoint::parse` returns `Result`; DESIGN §9.4 writes `Server::new("0.0.0.0:8080")`;
  §12 reports a bad endpoint at `prepare`.
- **Spine:** server builders take `impl Into<EndpointSpec>` (text, an `Endpoint`, a `SocketAddr`);
  text is parsed by `EndpointSpec::resolve` in `prepare`.

### 44. Socket activation and binding

- **Spine:** `Activation::get()` reads the environment once per process; `contains` serves
  `prepare`, `take` serves `bind`; `ActivationError` is `Clone` (`Io(Arc<io::Error>)`), with
  `PidMismatch`, `Malformed`, `Missing`, `NotListening`, `Unsupported` off Unix. `BoundListener`,
  `bind_all` (all-or-nothing) and `BindError` carry binding.

### 45. Dependencies beyond the design

- **Spine:** rustls with the `ring` provider; `rustls-pki-types` for PEM; `socket2` for
  `FD_CLOEXEC` and the socket-type check, which std does not expose.

### 46. `ulo-tokio`'s shapes

- **Spine:** `Timer` a unit struct; `shutdown_signal()` resolves to `Signal::new("SIGTERM")`,
  `"SIGINT"`, `"CTRL_C"` or `"CTRL_CLOSE"`; `spawn_in` returns `JoinHandle<Option<T>>`, `None` when
  the execution was cancelled first.

## `ulo-http`

### 47. `Server<B>` and the backend's `Server` alias

- **Design:** DESIGN §9.4 writes `fw_http::Server::new("0.0.0.0:8080")`, naming no backend; §3.7:
  `fw_http::Server<B: Backend>`.
- **Spine:** `Server::<B>::new(endpoint)` requires `B: Default`; `Server::with_backend(endpoint, b)`;
  each backend crate exports `type Server = ulo_http::Server<ItsBackend>`, so the call reads
  `ulo_http_axum::Server::new("0.0.0.0:8080")`.
- **Why:** `ulo-http` cannot default to a backend that depends on it.

### 48. The server builder and `HttpConfig`

- **Spine:** `endpoint`, `tls`, `body_limit`, `max_inflight`, `max_concurrent_streams`,
  `shed_retry_after`, `challenge`, `h2c`; `#[non_exhaustive] HttpConfig` with those fields, the
  body limit 2 MiB unset.

### 49. `KB`, `MB`, `BodyLimit`, `Timeout`

- **Design:** writes `2 * MB`, `#[meta(BodyLimit(..))]`, `#[meta(Timeout(..))]`.
- **Spine:** `KB`, `MB` as `u64` constants; `BodyLimit(pub u64)`, `Timeout(pub Duration)`.

### 50. `HttpBody`, `Body`, `Response`

- **Design:** names `HttpBody` (request body, tower service) and `Body::stream(s)` (responses).
- **Spine:** one type, `HttpBody`, with `Body` as an alias; `Response` is
  `http::Response<HttpBody>`.

### 51. `HttpCx`'s methods

- **Design:** `head`, `method`, `uri`, `headers`, `route`, `exec`, `ext::<T>()`, `take_body`,
  `response_headers`, `client_addr`.
- **Spine:** also `param`, `params`, `extensions`, `take_upgrade`, `conn`, `app`; `ext` returns
  `Result<Ext<T>, LookupError>`; `response_headers` returns the `MutexGuard`.

### 52. `Request`, `ConnInfo`, `TlsInfo`, `OnUpgrade`, `Upgraded`

- **Spine:** `Request::set_path` returns `Result<(), http::Error>`; `ConnInfo` and `TlsInfo` are
  `#[non_exhaustive]` with constructors for backends; `OnUpgrade` resolves to `Upgraded`, an
  `AsyncRead + AsyncWrite` box.

### 53. `BackendLimits`

- **Design:** an `upgrades` entry; the table's limits (h2c, inherited sockets, port 0).
- **Spine:** `upgrades`, `h2c`, `inherited_sockets`, `port_zero`, `tls`; `BackendLimits::NONE` and
  const setters.

### 54. Middleware answers a `Response`; only layers have an `Err`

- **Design:** §3.3 gives `Middleware::handle -> Response` and says "an entry's `Err` reaches the
  error handlers".
- **Spine:** as designed: a middleware fails only by panicking; an `Err` comes from a tower layer.
  `middleware::Next::run(self, req) -> BoxFuture<'a, Response>`.

### 55. Pre-dispatch scopes and `exclude`

- **Spine:** a scope is a route pattern whose last segment may be `*` (one or more segments),
  matched against route patterns in `prepare`; `exclude` applies to the entry written before
  it.

### 56. `ulo_http::Service`

- **Design:** "`Service<http::Request<HttpBody>, Response = http::Response<HttpBody>>`".
- **Spine:** it accepts any `Bytes` body and its error is `Infallible`; a layer's service may change
  the response body type and its error converts into `BoxError`.
- **Why:** a body-wrapping layer such as `RequestBodyLimitLayer` hands its inner service a wrapped
  body.

### 57. `Cors`'s API

- **Spine:** `new` (allows nothing), `allow_origin` (repeatable), `allow_any_origin`,
  `allow_methods`, `allow_headers`, `allow_any_header`, `expose_headers`, `allow_credentials`,
  `max_age`; `CorsError` for the refusal in `prepare`.

### 58. The WebSocket hand-off point

- **Design:** §3.5: the router hands an upgrade request to `fw-ws` through the backend's upgrade
  future; no type is named.
- **Spine:** `trait UpgradeHandler { paths(&self, &AppHandle); upgrade(&self, Request) }` and
  `Upgrades` module metadata through which a transport registers one.

### 59. Replies

- **Spine:** `Created::at(location, body)`; `NoContent`; `WithHeaders::new(r).header(n, v)`;
  `String` as `text/plain; charset=utf-8`, `Bytes` as `application/octet-stream`, `&'static str`.

### 60. SSE

- **Design:** `Sse::end_event("end")`; `EventId::new(..)?`; item errors take the late path.
- **Spine:** `end_event(&'static str)`, validated when the reply converts, before anything is
  written; `Event::{json, comment}` added; the error type `EventFieldError`; items through
  `SseItem`, implemented for `Event` and `Result<Event, E: Into<CallError>>`. The mid-stream error
  form, which §3 leaves unstated, is an event named `error` carrying the envelope, after which the
  stream ends.

### 61. `Header<H>` and `Multipart`

- **Spine:** `Header<H>` absent is `Missing` naming the header, so `Option<Header<H>>` is `None`;
  `Multipart::next_field` answers `multer`'s field and error types.

### 62. The pattern grammar is checked twice

- **Spine:** `ulo-http-macros` checks a literal at compile time with its own parser of the same
  grammar `ulo-http`'s router parses at `prepare`.
- **Why:** the macro crate cannot depend on the crate that re-exports it.

## `ulo-http-axum`

### 63. `Axum`

- **Spine:** `pub struct Axum` (the backend, `Default`), `type Server = ulo_http::Server<Axum>`; no
  documented limit.

## Workspace

### 64. Dependencies added

- **Spine:** workspace entries for `async-lock`, `axum`, `futures-core`, `headers`, `http-body`,
  `hyper`, `hyper-util`, `multer`, `percent-encoding`, `pin-project-lite`, `proc-macro2`, `quote`,
  `rustls`, `rustls-pki-types`, `serde_urlencoded`, `socket2`, `syn`, `tokio-rustls`. `ulo-macros`
  takes `proc-macro2`, `quote` and `syn` from the workspace.
