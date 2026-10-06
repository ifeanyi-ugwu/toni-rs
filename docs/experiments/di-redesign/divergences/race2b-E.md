# Divergences: race 2b, area E (the five embedding adapters and the HTTP conformance suite)

Every place area E departs from `transports/DESIGN.md` §3.8, the responses, `REVIEW_2B.md` or the
spine's frozen surface, fills a point they leave open, or needs a change in a file it does not own.
Each entry gives what the design says, what was written, and why. All await the user's sign-off.

No cargo was run. Host APIs were read from `~/.cargo/registry`: axum 0.8.9, salvo_core 0.92.2,
poem 3.1.12, actix-web 4.14.0 with actix-http 3.18.12 and actix-server 2.9.7, rocket 0.5.1 with
rocket_http 0.5.1, ubyte 0.10.4.

Files written: `crates/ulo-http-axum/{Cargo.toml, src/lib.rs, src/layer.rs, src/run.rs}`,
`crates/ulo-http-salvo/{Cargo.toml, src/lib.rs, src/handler.rs, src/run.rs}`,
`crates/ulo-http-poem/{Cargo.toml, src/lib.rs, src/endpoint.rs, src/run.rs}`,
`crates/ulo-http-actix/{Cargo.toml, src/lib.rs, src/service.rs, src/pump.rs, src/run.rs}`,
`crates/ulo-http-rocket/{Cargo.toml, src/lib.rs, src/handler.rs, src/convert.rs, src/upgrade.rs,
src/run.rs}`, `crates/ulo-http-conformance/{Cargo.toml, src/lib.rs, src/app.rs, src/wire.rs,
src/reference.rs, src/rocket_fairing.rs, src/cases/*.rs}`. `src/app.rs` and `src/wire.rs` are new
files in E's own crate.

## Requests for `ulo-http` (S's files)

The five adapters compile only once these three land. Each is additive.

### R1. `Embed::STRIPS_PREFIX`, and `Service::respond` stripping `.nested_at`

- **Design:** §3.8 Prefix: salvo, actix and rocket hand the full path, "so those three adapters
  strip `.nested_at` before building the app's `Request`"; a request outside the prefix "is
  answered as the app's 404 with `Routing::NotFound` and logged at `warn` once".
- **Gap:** an adapter cannot do either. The normalized prefix lives in `Embedded::mount` and
  `ServiceInner::mount`, neither reachable from `Handle` or `Service`, and the app's `NoRoute` 404
  is rendered only inside the app.
- **Request:** in `crates/ulo-http/src/embed.rs`, `trait Embed` gains
  `const STRIPS_PREFIX: bool = true;` ("whether the host removes the mount prefix before its
  handler sees the path"). When it is `false`, `Service::respond` strips the normalized
  `.nested_at` from `req.head.uri` (keeping the query) before answering, and answers a path outside
  the prefix as the app's `NoRoute` 404 with `Routing::NotFound`, logging at `warn` on the first
  such request.
- **Written:** axum and poem declare `true`, salvo, actix and rocket `false`. The rule then holds by
  construction: no native adapter can reach the app without the strip, as no adapter can skip the
  `forward` copies (ninth response, entry 34).

### R2. `Embed::builtin_forwards`, called by `Embedded::new()`

- **Design:** tenth response: an adapter's built-in values are `forward` registrations "inside
  `ulo_http_rocket::Embedded::new()`"; the forward is the insertion.
- **Gap:** each adapter's `Embedded` is an alias of `ulo_http::embed::Embedded<A>`, so
  `ulo_http_rocket::Embedded::new()` is `ulo-http`'s constructor and an adapter crate cannot add to
  it. The spine's doc on the rocket alias promises a registration nothing can make.
- **Request:** `trait Embed` gains
  `fn builtin_forwards(embedded: Embedded<Self>) -> Embedded<Self> where Self: Sized { embedded }`,
  and `Embedded::new()` returns `A::builtin_forwards(embedded)`.
- **Written:** salvo (`req.request.uri()`), poem (`req.original_uri()`), actix (`req.path()`) and
  rocket (`req.uri().path()`) each register `OriginalPath` there. axum keeps the layer, its value
  arriving as a host extension under `host_extensions: true`, as §3.8 states.

### R3. `Handle::body_limit()`

- **Design:** twentieth response, answer 3: rocket buffers "under the embedding's own
  `body_limit`".
- **Gap:** the value is in `Shared::state`'s `HttpConfig`, private to `ulo-http`.
- **Request:** `impl<A: Embed> Handle<A> { pub fn body_limit(&self) -> u64 }`, reading
  `self.shared.state().config().body_limit`.
- **Written:** the rocket handler buffers under it.

## Adapters

### 1. salvo's `run` stops through `ServerHandle::stop_graceful`

- **Design:** §3.8 and `REVIEW_2B.md` R3: `serve_with_graceful_shutdown(service, handle.stopping(),
  Some(drain))`.
- **Written:** salvo 0.92 has no such method. `run` takes `server.handle()`, serves with
  `server.try_serve(service)`, and calls `stop_graceful(drain)` on the handle once `stopping()`
  resolves, the serve future polled throughout since salvo reads the stop command inside its loop.
  `salvo` gains the `server-handle` feature. `run<A>` is bounded `A: Acceptor + Send + 'static`.
- **Why:** the call the design names does not exist; `stop_graceful(Option<Duration>)` is the
  documented graceful stop with a bound (`salvo_core-0.92.2/src/server.rs:90, 186, 252`).

### 2. The `run` bounds (the spine's open point, entry 41)

- axum: the bounds axum puts on awaiting `WithGracefulShutdown` (`L: Listener`, `L::Addr: Debug`,
  the `M` and `S` service bounds).
- poem: `L: Listener + 'static, L::Acceptor: 'static, A: Acceptor + 'static,
  E: IntoEndpoint + Send + 'static, E::Endpoint: 'static`; `E: Send` because the host future is.
- actix: the spine's where-clause gains `'static` on `S`, `S::Error`, `S::Response`, `S::Service`,
  `<S::Service as Service<Request>>::Future` and `B`, the bounds of the `impl` block holding
  `disable_signals` and `shutdown_timeout`; `HttpServer::new` already requires them.
- rocket: none.

### 3. Every `run` closes the app when `Handle::host` refuses the host

- **Design:** silent.
- **Written:** on `Err` from `host`, `run` calls `app.handle().close(..)` before returning it.
- **Why:** the app is bound by then, and returning without serving would leave it listening.

### 4. rocket's `run`: `shutdown.signals` emptied, and an ignite failure becomes the host's error

- **Design:** `shutdown.ctrlc = false` and `shutdown.grace` before `ignite()`.
- **Written:** also `shutdown.signals = []`: on Unix rocket otherwise handles SIGTERM, a second owner
  of the trigger. `mercy` is left at rocket's default. A rocket that fails to ignite is installed
  as a host future failing at once, so the app shuts down naming the error. Errors cross as their
  `Display`, which also marks rocket's error handled; its `Drop` panics otherwise.
- **Open point answered:** `Rocket<Ignite>::shutdown()` exists (`rocket-0.5.1/src/rkt.rs:638`), so
  the handle is taken before `launch()`.

### 5. rocket reads the body only when the app first polls it

- **Design:** `request_body: Buffered(body_limit)`; `Miss::Forward` hands back "the request's
  original `Data`, unread".
- **Written:** the app's body asks for the read on its first poll; the handler then reads
  `body_limit + 1` bytes of `Data` while awaiting the answer and hands them over as one frame. A
  body the app never polls leaves `Data` unread, which a `Forwardable` answer returns as
  `Outcome::Forward((data, Status::NotFound))`. A body longer than `body_limit` arrives as
  `body_limit + 1` bytes, which a default-limit route refuses with the same 413 as everywhere;
  under a route whose own limit is higher, an error frame follows, so the app never takes the
  truncated body for the whole one.
- **Why:** buffering before the call consumes the `Data` the forward needs; both rules hold this
  way.

### 6. rocket: `Rocket::limits().request_body` reports the default `body_limit`

- **Design:** "`RequestBody::Buffered` reports that value", the embedding's own.
- **Written:** `Buffered(HttpConfig::default().body_limit)`, 2 MiB.
- **Why:** `Embed::limits()` is an associated function, with no instance to read.

### 7. rocket: catch-alls at rank 100, every method, `Routing` as `Option<Routing>`

- **Design:** "one catch-all route per method", ranked below rocket's own; `Routing` in the local
  cache.
- **Written:** `/<path..>` for all nine rocket methods at rank 100, above rocket's default ranks
  (-12 to -1). The cache holds `Option<Routing>`, read with `req.local_cache(|| None::<Routing>)`;
  nothing is cached for a forwarded miss, which the host answers. The version is HTTP/1.1, since
  rocket's `Request` exposes none; `ConnInfo::local` is unset for the same reason.

### 8. rocket upgrades: the protocol is whatever the app's 101 names

- **Design:** rocket hands the `IoStream` over when "the app's 101 names `websocket`".
- **Written:** the response carries the hand-off under the protocol its `Upgrade` header names;
  rocket matches it against the request's. An upgrade future is created only for a request with an
  `Upgrade` header.

### 9. A rocket or salvo body of known length

- **Written:** rocket collects a body whose size hint is exact and hands it over sized, so rocket
  writes `Content-Length`; any other body streams through an `AsyncRead`. salvo and poem wrap the
  body in a mutex-guarded `Sync` body keeping its size hint, which their boxed bodies require.
  salvo always gets a body, an empty one included, since salvo replaces an error status with no
  body by its catcher's page.

### 10. actix: no response pump

- **Design:** "on actix the drop is the worker-local pump's"; the spine's `pump.rs` doc pumps the
  response too.
- **Written:** only the request payload is pumped. The response is a `MessageBody` over the app's
  `Send` body, polled on the worker, `BodySize::Sized` where the size is exact; actix drops it at
  the next failed write, the same moment `AtNextWrite` declares.
- **Why:** the pump existed on `master` to carry actix's `!Send` body into a `Send` one; the app's
  body is already `Send`.

### 11. actix: `ActixScope` is also a `ServiceFactory`, and `ActixService` is exported

- **Design:** `App::service(scope("/api", embedded))` or `default_service`.
- **Written:** `register` builds `web::scope(path).default_service(self)`; `ActixScope` implements
  `ServiceFactory<ServiceRequest>` so `App::default_service(scope("", &embedded))` mounts it as the
  fallback. `ActixService`, the factory's service type, is exported. Of the app's response
  extensions only `Routing` is copied into actix's own store.

### 12. axum: `HostLayer` wraps the service, and the `NestedPath` check is not written

- **Design:** the layer sits ahead of the service; "the axum adapter reads the host's `NestedPath`
  on each request and logs a mismatch against `.nested_at`".
- **Written:** the documented placement is `nest_service("/api", HostLayer.layer(embedded.service()))`.
  `Router::layer` wraps each route outside axum's `StripPrefix` and `SetNestedPath`, where
  `NestedPath` is not yet set. The layer inserts `ConnInfo` (peer from `ConnectInfo<SocketAddr>`,
  an existing one kept) and `OriginalPath` from `OriginalUri`. No mismatch is logged.
- **Why:** `HostLayer` is a unit struct and its future is `S::Future` (both frozen), so it holds no
  prefix and sees no response; `ulo-http` exposes none either.
- **Request (S, optional):** an `embed::HostPrefix` extension a tower layer inserts from the host's
  record, which `Service`'s tower `call` compares with the normalized prefix and warns about once.

### 13. poem's `take_upgrade` error crosses as text

- **Written:** `UpgradeError` becomes a `BoxError` from its `Display`, which drops its source.

## The conformance suite

### 14. Public items added

- **Spine:** `Host`, `Mode`, `PREFIX`, `HyperHost`, `app()`, the macro, `RoutingFairing`.
- **Written, added:** `app_for(EmbedLimits)` (the app without its upgrade handler where `upgrades`
  is `false`, which `prepare` refuses otherwise), `ROUTING_HEADER`, `routing_label(&Routing)`,
  `HostValue`, `HOST_VALUE_HEADER`, `ORIGIN`. `app()` is `app_for(EmbedLimits::NONE)`.
- **Why:** the client sees no response extensions and no host store, so each adapter's `Host` and
  the suite need a shared contract for both, which `Host`'s doc states: the host writes
  `ROUTING_HEADER` from the response's `Routing`, and puts `HostValue` from the header in its own
  store (or copies it with `forward` where `host_extensions: false`).

### 15. The reference host is recognised by `TypeId`

- **Written:** where the backend cannot do what a host does (strip a prefix, write a host store,
  read `Routing`), the scenario checks `TypeId::of::<H>() == TypeId::of::<HyperHost>()` and runs the
  half that applies. `HyperHost` serves the app at its root in both modes.
- **Why:** the frozen `Host` trait has no way to say it; adding one is a trait change.

### 16. Byte-identical means status, the app's headers and the body

- **Written:** each scenario that depends on no limit compares against a `HyperHost` started in the
  same mode: status, nine headers the app writes (content type, `Allow`, CORS, cache, retry,
  challenge, `Vary`) and the body bytes. Framing and host headers (`date`, `server`, rocket's Shield
  headers) are left out.

### 17. `lifecycle::unavailable` cannot reach the 503 before `listen()`

- **Design:** "the 503 before `listen()` and after `close`".
- **Written:** after `AppHandle::close` the scenario accepts no listener, or a 503 with
  `Connection: close` and, where readable, `Routing::Unrouted`.
- **Why:** `Host::start` returns once the app listens and every `run` serves the host only after
  `listen()`, so no request can arrive in `Unbound` through `Host`. Reaching it needs a host
  started before `listen()`, a trait change; and after `close` the host has stopped with the app.

### 18. The drain's two shapes

- HTTP/1.1: half a request head is written before `close`, the rest once `draining()` resolves; the
  answer must be 503 with `Connection: close`, then the connection closes.
- HTTP/2: run where the host answers an h2c prior-knowledge request, skipped otherwise; a stream held
  open across the drain, then a new stream on the connection must not be served 200. GOAWAY itself
  is not observed: reqwest does not expose it.

### 19. The disconnect scenario's clock

- **Written:** `GET /endless` writes one event, idles 600 ms, then writes every 100 ms. The client
  leaves after the first event; `AtClose` must observe the disconnect by 300 ms, `AtNextWrite`
  must not by then and must by 3 s. The probe records `StreamOutcome::CutOff` with
  `CancelReason::Disconnected`, from the outcome or from the execution's reason read in the
  `on_stream_end` callback.

### 20. The upgrade scenario echoes raw bytes

- **Written:** the echo handler answers 101 naming `websocket` and writes back raw bytes; no
  WebSocket framing is spoken. On a host declaring `upgrades: false` the same request must not get
  a 101. `tokio-tungstenite` was dropped from the suite's dependencies.

### 21. `host_extensions: false` is not checked in its negative direction

- **Written:** `present_and_absent` returns at once on a host declaring `false`, and `forward_copy`
  on one declaring `true`.
- **Why:** under `false`, a `Host<HostValue>` read with no copy is refused at `prepare`, so the
  suite's app cannot run on such a host without the copy the contract has `start` register.

## Not verified

- Nothing compiled. In particular: the `Send`-ness of poem's `run_with_graceful_shutdown` future and
  rocket's `launch` future for `Handle::host`; the let-chains in rocket's handler under NLL; salvo's
  catcher not replacing a boxed empty body; rocket writing `Content-Length` for a sized body and
  `figment`'s dotted keys in `("shutdown.ctrlc", false)`; `#[meta(BodyLimit(16))]` and
  `Valid<Json<Item>>` in the suite's controller; a module-level `AnyErrorHandler<Http>` enhancer
  acting as a global error handler.
- The adapters call R1–R3, which do not exist yet.
- The disconnect and drain timings are observations the suite makes, not guarantees read from the
  hosts' code.
