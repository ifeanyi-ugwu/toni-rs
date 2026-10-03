# Divergences: race 2a, P (the HTTP pipeline and server)

Every place `ulo-http`'s pipeline and server depart from `transports/DESIGN.md` §2.7, §2.8, §3.3,
§3.4, §3.6, §3.7 and §10, or fill what they leave open, beyond the spine's entries 47–58. Each entry
gives what the design says, what was written, and why. Requests for other areas follow the entries.

Files: `crates/ulo-http/src/{middleware,pre_dispatch,cors,tower_bridge,service,server,backend}.rs`.
`upgrade.rs` had nothing to fill and is unchanged. No public signature changed. One crate-private
constructor was added, `middleware::Next::new`, whose closure bound lets a `'static` continuation
coerce into a `Next` borrowing the middleware's `&self`.

## Entries

### 1. The continuation through a tower layer (the point left to P)

- **Design:** §3.4: layers are composed once, in `prepare`, and run inside the stage.
- **Written:** the request's extensions carry it, as the spine assumed. Before a layer runs, the
  stage converts its `Request` into an `http::Request<HttpBody>` and inserts a `Continuation`
  holding the rest of the chain together with the `ConnInfo` and upgrade future, which an
  `http::Request` cannot carry. `ulo_http::Service::call` removes it, boxes the body back into an
  `HttpBody` and runs it. `http::Extensions` stores only `Clone` values, so the continuation is a
  take-once slot every clone shares.
- **User-visible:** a layer that calls its inner service twice for one request, a retry layer for
  example, or that builds a new request without the original's extensions, gets a bare 500 from
  `Service` and an `error` log line. The rest of the chain is one-shot: the body has been read and
  the middleware chain consumed.
- **tower's `util` feature:** not needed, and no request is made. Each request drives its own clone
  of the layered service through `std::future::poll_fn` over `poll_ready`, then `call`, which is
  what `oneshot` does.

### 2. The route timeout (the point left to P)

- **Design:** §3.6: "A route timeout (`#[meta(Timeout(..))]`) fires `Deadline` and renders 504."
  `limits.rs`: "an answer not yet started renders 504."
- **Written:** armed after routing, on `ServiceInner::timer`, around the scoped sub-step and
  `dispatch`; the unscoped entries run before the route is known and are outside it. The pipeline
  is raced against `Timer::sleep`.
  - The sleep wins: the pipeline is dropped at its current await, the execution is cancelled with
    `Deadline`, and `render::timed_out()` is rendered as problem details.
  - The pipeline wins: the pending sleep moves into the response body, polled with it. When it
    passes, the execution is cancelled with `Deadline` and the body is left to end as it ends; a
    streaming body observes the cancellation through `cancelled()`.
- **Gap filled:** the 504 does not pass through the error handlers. The design keeps the deadline
  out of the error (§2.4: the reason lives on the execution), and running error handlers after the
  deadline would be unbounded by it.

### 3. Routing misses and the error handlers

- **Design:** §3.2: 404 on no match, 405 with `Allow` on a wrong method, 204 with `Allow` for
  `OPTIONS`. §3.3: "on a miss only the global error handlers apply". The spine's `AppService` doc
  sends the misses through `ulo::recover(None, ..)`.
- **Written:**
  - 404: offered to the global error handlers as `CallError::new(NotFound, "no route matches this
    path")`; unclaimed, rendered by `render::render_error`.
  - 405: offered as a private `MethodNotAllowed` error carrying `Allow`; unclaimed, rendered as
    problem details with status 405 and the `Allow` header, built in `service.rs`.
  - `OPTIONS` with no handler: 204 with `Allow`, answered directly. A success is not offered to
    error handlers.
- **Why:** no `ErrorKind` is 405, so the 405 cannot be a `CallError`. An error handler sees an
  opaque error, and one that calls `CallError::from_boxed` on it gets `Internal`. A representation
  for 405, a kind or a public error type, needs the user's decision.

### 4. The context `dispatch` and the error handlers read

- **Design:** §3.1: `HttpCx` holds an `Arc<RequestHead>`; §2.10: `RequestHead` is an execution
  input seeded by the transport. Silent on whether the two are the same head when the pre-dispatch
  stage changes the request.
- **Written:** the `RequestHead` input is seeded at `Execution::open` with the head as the client
  sent it, as `RequestHead::path`'s doc states. The `HttpCx` that `dispatch` receives is built from
  the request as the stage leaves it: a rewritten path, a header a middleware set, and a header a
  tower layer changed (tower-http's decompression removes `Content-Encoding`) reach the extractors.
- **Stage failures:** the context offered with an entry's failure is built from the head as that
  sub-step received it, with no body and no upgrade, since the request is the failing entry's. In
  the scoped sub-step it carries the matched route and the handler's tiers run before the global
  ones; in the unscoped sub-step only the global ones run, even when a route matches later, as
  `middleware.rs` documents.

### 5. What reaches an entry's `catch_panic`

- **Design:** §3.3: an entry's panic reaches the error handlers as `PanicRecovered`.
- **Written:** each entry runs inside its own `AppHandle::catch_panic` with
  `DispatchStage::PreDispatch`, its `next` included. Every entry after it is caught by its own
  catch first, and `dispatch` catches its own stages, so what reaches an entry's catch is its own
  panic, before or after `next` returns.
- **Limitation:** the code between the stage and `dispatch` (upgrade matching, routing, building
  the context, rendering) is not an entry. A panic there reaches the innermost unscoped entry's
  catch and is attributed to it, or reaches the backend when no unscoped entry exists. An
  `UpgradeHandler::upgrade` panic takes the same path.

### 6. The in-flight permit and the execution last as long as the response body

- **Design:** §2.8, §3.6: over the in-flight limit a request gets 503; silent on when a request
  stops counting.
- **Written:** the `Permit` and an `ExecutionRef` sit in the outermost response body and drop when
  the backend drops it. A streaming response, SSE included, counts against `max_inflight` for as
  long as it streams, and its execution stays open, as the core's execution doc has it: a
  streaming reply keeps its instances exactly as long as it runs.

### 7. When `Disconnected` fires

- **Design:** §2.6: the backend observed the peer close the connection or reset the stream,
  through a dropped response body; the server dropping an unread request body is not one.
- **Written:** the outermost response body fires `cancel_with(Disconnected)` when it drops before
  its end. Its end is a `None` frame, a frame after which `is_end_stream()` holds, or an error
  frame, which is the server cutting the body rather than the peer leaving. A body the backend
  never writes counts as ended from the start: the answer to a `HEAD`, and a 1xx, 204 or 304
  answer.

### 8. `HEAD` answered by a `GET` handler (H's point, applied in step 8)

- **Design:** §3.2: `HEAD` is answered from the `GET` handler with the body omitted.
- **Written:** after `dispatch`, a response for `Routed::Found { head_from_get: true }` has its body
  replaced by an empty one. When the response carries no `Content-Length` and the dropped body's
  size hint is exact, that size is set as `Content-Length`, as RFC 9110 has a `HEAD` answer carry
  the `GET` answer's headers. A streaming `GET` body is dropped unread, and a `Tracked` stream
  inside reports `CutOff(None)`.
- **Statuses that gain no length:** 1xx and 204, for which RFC 9110 §8.6 forbids `Content-Length`,
  and 304 (H's request). A 304 may carry only the length a 200 would have had, and the dropped
  body does not establish it, so a 304 is skipped too. A `Content-Length` the handler set itself is
  left as written.

### 9. The upgrade hand-off at runtime

- **Design:** §3.5: when a gateway's path matches a request carrying `Upgrade: websocket`, the
  router hands it to `ulo-ws`.
- **Written:** after the unscoped entries, any request carrying an `Upgrade` header whose path
  matches a registered upgrade path goes to that `UpgradeHandler` before routing, whatever the
  header's value; the handler decides what it accepts. No `route_to` runs, since the gateway's
  module is not known to the HTTP transport, and the scoped entries do not run.
- **In `prepare`:** each upgrade path is parsed as a route pattern. Two paths that would match the
  same requests are a `Configure` failure, whether one handler or two registered them. Any upgrade
  path on a backend declaring `upgrades: false` is a `Configure` failure naming the backend.

### 10. What `prepare` checks

- **Design:** §2.7, §3.7, §12: route table, TLS, CORS, endpoints, inherited sockets and backend
  limits, every failure reported together.
- **Written:** one `PrepareError` listing every failure, which `listen()` wraps as this transport's
  `ConfigureError`. Beyond the design's list:
  - A failed pre-dispatch stage does not stop the route table being built over an empty stage, so
    route conflicts are reported beside the stage's failures.
  - Each `BackendLimits` entry is checked: `upgrades` (entry 9), `h2c` against `.h2c(true)`, `tls`
    against `.tls(..)`, `inherited_sockets` against an inherited endpoint, `port_zero` against an
    address with port 0.
  - An address endpoint listed twice is refused, except with port 0, where two endpoints are two
    listeners on ports the OS chooses.
  - `Activation::get()` is read only when an endpoint is inherited, as N's note P1 asks, and once
    per `prepare`.
  - Each inherited name is checked once against `Activation::count`: a name no untaken descriptor
    answers to fails as `ActivationError::Missing`, and a name this server lists more often than
    descriptors answer to it fails naming the name, how often it is listed and how many answer
    (N's note P2). An index answers for one descriptor at most, so an index listed twice fails the
    same way. Without the check the shortfall would fail at `bind`, where each listing takes the
    next descriptor.
  - TLS is loaded with ALPN `h2` then `http/1.1`.
  - `bind` without a prior `prepare` answers an error rather than preparing.

### 11. `PreDispatch` refusals the spine left without a body

- **Design:** silent. The spine's doc on `exclude` says `prepare` reports a call with no entry
  before it; its body discarded the call.
- **Written:**
  - `exclude` gains `#[track_caller]` and records a stray call's location in a crate-private field
    of `PreDispatch`; `prepare` reports it.
  - `apply_for` and `layer_for` with an empty pattern list are a `Configure` failure: a scoped
    entry naming no pattern covers no route. `Entry` gains a crate-private `scoped` flag, since an
    empty `scope` no longer tells the two apart.
  - A scope or exclusion that does not parse is a `Configure` failure naming the entry's location.
  - On an unscoped entry, `exclude` is matched against the request path as that entry receives it,
    through `ScopePattern::covers_path`, since no route is known yet.

### 12. `Stage`'s fields

- **Design:** not a design item; the build plan names `Stage` in P's area and lists
  `Stage::scoped_for` and `ScopedStage` as contracts.
- **Written:** `Stage::unscoped` is removed: nothing read it, and the runnable list the stage keeps
  for the unscoped sub-step replaces it. `ScopedStage::steps` is kept and is what `scoped_for`
  compares, by `Arc::ptr_eq` on the `PreDispatch` and the entry index, so routes with the same set
  of entries share one stage, as the doc on `Stage::shared` states. `scoped_for`'s signature is
  unchanged.

### 13. The call span's fields

- **Design:** §2.9: the span is named `GET /users/{id}`, with `http.request.method`, `http.route`,
  `url.path`, `url.scheme`, `http.response.status_code`, `ulo.transport` and `ulo.handler`.
- **Written:** `span::call` is created at `Execution::open` with the method alone as `otel.name`,
  the route being unknown. After routing, `otel.name` is recorded again as `GET /users/{id}`,
  `http.route` as the pattern and `ulo.handler` as `Controller::method`. `url.scheme` is `https`
  when the connection carries TLS information and `http` otherwise. `url.path` is the path as the
  client sent it. The status is recorded after the pipeline answers.

### 14. CORS behaviour the design leaves open

- **Design:** §3.4: preflights answered with 204 and the `Access-Control-Allow-*` headers,
  `Vary: Origin` added, `*` never echoed with credentials.
- **Written:**
  - A preflight is `OPTIONS` carrying `Origin` and `Access-Control-Request-Method`. One from an
    origin not allowed is still answered 204, with `Vary` and no allow headers, and the browser
    fails it.
  - A preflight's `Vary` lists `Origin`, `Access-Control-Request-Method` and
    `Access-Control-Request-Headers`. Every other response passing through gets `Vary: Origin`,
    requests without `Origin` included, so a cache never hands a response lacking the allow headers
    to a cross-origin request. A `Vary` already listing the name, or `*`, is left alone.
  - Origins are compared byte for byte, as the Fetch specification compares them.
  - `Access-Control-Allow-Methods` is sent only when methods are configured. `allow_any_header`
    reflects `Access-Control-Request-Headers`.
  - A `Cors` allowing any origin with credentials that escaped `prepare`, wrapped inside another
    middleware for example, sends no allow headers at all rather than `*` or a reflected origin.

### 15. `Backend::drain`'s doc

- **Design:** §3.7: `drain`: GOAWAY, close idle keep-alives.
- **Written:** the doc adds that a backend may return only once its connections have ended, and
  that the core drops a drain still running at its drain deadline. HM's axum backend does this, so
  `close` does not cut a response still being written. The core's shutdown was read to confirm the
  deadline drops the drain future.

## Requests

- **N1 (`ulo-net/src/activation.rs`, N), applied:** `Activation::count(&self, name: &ListenerName)
  -> usize`, the untaken descriptors answering to `name`. `prepare` uses it (entry 10).
- **H1 (`ulo-http/src/cx.rs`, H):** say on `HttpCx::head` that it is the head as it reaches
  `dispatch`, after the pre-dispatch stage, while `Dep<RequestHead>` is the head as the client sent
  it; `RequestHead::path`'s doc is true of the input only (entry 4).
- **H2 (`ulo-http/src/render.rs`, H), optional:** a crate-private `render::method_not_allowed(allow,
  config)` or an exposed `document(error, status, config)`, so the 405 document is written beside
  the other problem documents and `service.rs` drops its own (entry 3).
- **User decision:** how an error handler recognises a 405 (entry 3).
